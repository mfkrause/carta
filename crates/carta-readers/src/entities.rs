//! Named and numeric character-reference decoding shared by the text readers. The named-entity
//! table is generated at build time from the vendored character-reference data.

include!(concat!(env!("OUT_DIR"), "/entities_table.rs"));

/// Convert a numeric character reference's code point to a character: the null code point and any
/// value outside the Unicode scalar range both map to the replacement character.
pub(crate) fn code_point(code: u32) -> char {
    if code == 0 {
        return '\u{fffd}';
    }
    char::from_u32(code).unwrap_or('\u{fffd}')
}

/// Look up a named character reference (keyed without the leading `&` or trailing `;`), returning its
/// replacement text.
pub(crate) fn lookup_named(name: &str) -> Option<&'static str> {
    ENTITIES
        .binary_search_by(|(candidate, _)| (*candidate).cmp(name))
        .ok()
        .and_then(|index| ENTITIES.get(index))
        .map(|(_, characters)| *characters)
}

/// Reads one character reference starting at the `&` in `chars[i]`, scanning the name or digits no
/// further than `limit`. Named (`&name;`) and decimal (`&#NN;`) forms are always recognized;
/// hexadecimal (`&#xNN;`) only when `allow_hex` is set. Returns the decoded text and the index just
/// past the closing `;`.
#[cfg(any(feature = "dokuwiki", feature = "jira", feature = "mediawiki"))]
pub(crate) fn read_reference(
    chars: &[char],
    i: usize,
    limit: usize,
    allow_hex: bool,
) -> Option<(String, usize)> {
    let mut j = i + 1;
    if chars.get(j) == Some(&'#') {
        j += 1;
        let hex = allow_hex && matches!(chars.get(j), Some('x' | 'X'));
        if hex {
            j += 1;
        }
        let begin = j;
        while j < limit
            && chars.get(j).is_some_and(|c| {
                if hex {
                    c.is_ascii_hexdigit()
                } else {
                    c.is_ascii_digit()
                }
            })
        {
            j += 1;
        }
        if j == begin || chars.get(j) != Some(&';') {
            return None;
        }
        let digits: String = chars.get(begin..j).unwrap_or(&[]).iter().collect();
        let code = u32::from_str_radix(&digits, if hex { 16 } else { 10 }).ok()?;
        Some((code_point(code).to_string(), j + 1))
    } else {
        let begin = j;
        while j < limit && chars.get(j).is_some_and(char::is_ascii_alphanumeric) {
            j += 1;
        }
        if j == begin || chars.get(j) != Some(&';') {
            return None;
        }
        let name: String = chars.get(begin..j).unwrap_or(&[]).iter().collect();
        Some((lookup_named(&name)?.to_string(), j + 1))
    }
}
