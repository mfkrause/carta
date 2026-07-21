//! ASCII-transliteration folds shared by the readers that build identifiers under an
//! `ascii_identifiers` extension.
//!
//! The four folds here have **different semantics by design**, so each reader's identifier output
//! stays byte-stable — they are co-located, not unified:
//!
//! - [`fold_to_ascii`] (the broad Latin `match`): folds an accented Latin letter to its base,
//!   covering the Latin-Extended-Additional block, and drops every character with no single-letter
//!   ASCII base (including whole non-Latin scripts).
//! - [`rst_asciify`] (the narrow Latin `match`): the same mechanism with a smaller table — it omits
//!   the Latin-Extended-Additional block and several accent rows, so its output differs from
//!   [`fold_to_ascii`] for the characters it does not list.
//! - [`transliterate_ascii`] (the code-point table): binary-searches a per-code-point base and drops
//!   any character absent from the table.
//! - [`dokuwiki_asciify`] (the decomposition strip): folds via canonical decomposition and keeps
//!   only the ASCII characters, so a combining mark is dropped in any script.
//!
//! Any future unification must be a deliberate change, verified per format, because these differences
//! are observable in committed identifier output.

#[cfg(feature = "dokuwiki")]
use unicode_normalization::UnicodeNormalization;

/// Transliterates header text to ASCII for the `ascii_identifiers` extension: each accented letter is
/// folded to its unaccented base, plain ASCII is kept, and every other character (a letter with no
/// ASCII base, or a non-Latin script) is dropped. The result is then slugged as usual.
#[cfg(any(feature = "man", feature = "org"))]
pub(crate) fn fold_to_ascii(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c);
        } else if let Some(base) = ascii_base(c) {
            out.push(base);
        }
    }
    out
}

/// The unaccented ASCII letter underlying a Latin letter with a diacritic, or `None` for a character
/// with no single-letter ASCII base (so the caller drops it).
#[cfg(any(feature = "man", feature = "org"))]
#[allow(clippy::match_same_arms)]
fn ascii_base(c: char) -> Option<char> {
    let base = match c {
        'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' | 'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'Ā' | 'ā' | 'Ă'
        | 'ă' | 'Ą' | 'ą' | 'Ǎ' | 'ǎ' | 'Ǟ' | 'ǟ' | 'Ǡ' | 'ǡ' | 'Ǻ' | 'ǻ' | 'Ȁ' | 'ȁ' | 'Ȃ'
        | 'ȃ' | 'Ȧ' | 'ȧ' => 'a',
        'Ç' | 'ç' | 'Ć' | 'ć' | 'Ĉ' | 'ĉ' | 'Ċ' | 'ċ' | 'Č' | 'č' => 'c',
        'Ď' | 'ď' => 'd',
        'È' | 'É' | 'Ê' | 'Ë' | 'è' | 'é' | 'ê' | 'ë' | 'Ē' | 'ē' | 'Ĕ' | 'ĕ' | 'Ė' | 'ė' | 'Ę'
        | 'ę' | 'Ě' | 'ě' | 'Ȅ' | 'ȅ' | 'Ȇ' | 'ȇ' | 'Ȩ' | 'ȩ' => 'e',
        'Ĝ' | 'ĝ' | 'Ğ' | 'ğ' | 'Ġ' | 'ġ' | 'Ģ' | 'ģ' | 'Ǧ' | 'ǧ' | 'Ǵ' | 'ǵ' => 'g',
        'Ĥ' | 'ĥ' | 'Ȟ' | 'ȟ' => 'h',
        'Ì' | 'Í' | 'Î' | 'Ï' | 'ì' | 'í' | 'î' | 'ï' | 'Ĩ' | 'ĩ' | 'Ī' | 'ī' | 'Ĭ' | 'ĭ' | 'Į'
        | 'į' | 'İ' | 'ı' | 'Ǐ' | 'ǐ' | 'Ȉ' | 'ȉ' | 'Ȋ' | 'ȋ' => 'i',
        'Ĵ' | 'ĵ' | 'ǰ' => 'j',
        'Ķ' | 'ķ' | 'Ǩ' | 'ǩ' => 'k',
        'Ĺ' | 'ĺ' | 'Ļ' | 'ļ' | 'Ľ' | 'ľ' => 'l',
        'Ñ' | 'ñ' | 'Ń' | 'ń' | 'Ņ' | 'ņ' | 'Ň' | 'ň' | 'Ǹ' | 'ǹ' => 'n',
        'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'Ō' | 'ō' | 'Ŏ' | 'ŏ' | 'Ő'
        | 'ő' | 'Ơ' | 'ơ' | 'Ǒ' | 'ǒ' | 'Ǫ' | 'ǫ' | 'Ǭ' | 'ǭ' | 'Ȍ' | 'ȍ' | 'Ȏ' | 'ȏ' | 'Ȫ'
        | 'ȫ' | 'Ȭ' | 'ȭ' | 'Ȯ' | 'ȯ' | 'Ȱ' | 'ȱ' => 'o',
        'Ŕ' | 'ŕ' | 'Ŗ' | 'ŗ' | 'Ř' | 'ř' | 'Ȑ' | 'ȑ' | 'Ȓ' | 'ȓ' => 'r',
        'Ś' | 'ś' | 'Ŝ' | 'ŝ' | 'Ş' | 'ş' | 'Š' | 'š' | 'Ș' | 'ș' => 's',
        'Ţ' | 'ţ' | 'Ť' | 'ť' | 'Ț' | 'ț' => 't',
        'Ù' | 'Ú' | 'Û' | 'Ü' | 'ù' | 'ú' | 'û' | 'ü' | 'Ũ' | 'ũ' | 'Ū' | 'ū' | 'Ŭ' | 'ŭ' | 'Ů'
        | 'ů' | 'Ű' | 'ű' | 'Ų' | 'ų' | 'Ư' | 'ư' | 'Ǔ' | 'ǔ' | 'Ǖ' | 'ǖ' | 'Ǘ' | 'ǘ' | 'Ǚ'
        | 'ǚ' | 'Ǜ' | 'ǜ' | 'Ȕ' | 'ȕ' | 'Ȗ' | 'ȗ' => 'u',
        'Ŵ' | 'ŵ' => 'w',
        'Ý' | 'ý' | 'ÿ' | 'Ŷ' | 'ŷ' | 'Ÿ' | 'Ȳ' | 'ȳ' => 'y',
        'Ź' | 'ź' | 'Ż' | 'ż' | 'Ž' | 'ž' => 'z',
        '\u{1e00}' | '\u{1e01}' | '\u{1ea0}' | '\u{1ea1}' | '\u{1ea2}' | '\u{1ea3}'
        | '\u{1ea4}' | '\u{1ea5}' | '\u{1ea6}' | '\u{1ea7}' | '\u{1ea8}' | '\u{1ea9}'
        | '\u{1eaa}' | '\u{1eab}' | '\u{1eac}' | '\u{1ead}' | '\u{1eae}' | '\u{1eaf}'
        | '\u{1eb0}' | '\u{1eb1}' | '\u{1eb2}' | '\u{1eb3}' | '\u{1eb4}' | '\u{1eb5}'
        | '\u{1eb6}' | '\u{1eb7}' => 'a',
        '\u{1e02}' | '\u{1e03}' | '\u{1e04}' | '\u{1e05}' | '\u{1e06}' | '\u{1e07}' => 'b',
        '\u{1e08}' | '\u{1e09}' => 'c',
        '\u{1e0a}' | '\u{1e0b}' | '\u{1e0c}' | '\u{1e0d}' | '\u{1e0e}' | '\u{1e0f}'
        | '\u{1e10}' | '\u{1e11}' | '\u{1e12}' | '\u{1e13}' => 'd',
        '\u{1e14}' | '\u{1e15}' | '\u{1e16}' | '\u{1e17}' | '\u{1e18}' | '\u{1e19}'
        | '\u{1e1a}' | '\u{1e1b}' | '\u{1e1c}' | '\u{1e1d}' | '\u{1eb8}' | '\u{1eb9}'
        | '\u{1eba}' | '\u{1ebb}' | '\u{1ebc}' | '\u{1ebd}' | '\u{1ebe}' | '\u{1ebf}'
        | '\u{1ec0}' | '\u{1ec1}' | '\u{1ec2}' | '\u{1ec3}' | '\u{1ec4}' | '\u{1ec5}'
        | '\u{1ec6}' | '\u{1ec7}' => 'e',
        '\u{1e1e}' | '\u{1e1f}' => 'f',
        '\u{1e20}' | '\u{1e21}' => 'g',
        '\u{1e22}' | '\u{1e23}' | '\u{1e24}' | '\u{1e25}' | '\u{1e26}' | '\u{1e27}'
        | '\u{1e28}' | '\u{1e29}' | '\u{1e2a}' | '\u{1e2b}' | '\u{1e96}' => 'h',
        '\u{1e2c}' | '\u{1e2d}' | '\u{1e2e}' | '\u{1e2f}' | '\u{1ec8}' | '\u{1ec9}'
        | '\u{1eca}' | '\u{1ecb}' => 'i',
        '\u{1e30}' | '\u{1e31}' | '\u{1e32}' | '\u{1e33}' | '\u{1e34}' | '\u{1e35}' => 'k',
        '\u{1e36}' | '\u{1e37}' | '\u{1e38}' | '\u{1e39}' | '\u{1e3a}' | '\u{1e3b}'
        | '\u{1e3c}' | '\u{1e3d}' => 'l',
        '\u{1e3e}' | '\u{1e3f}' | '\u{1e40}' | '\u{1e41}' | '\u{1e42}' | '\u{1e43}' => 'm',
        '\u{1e44}' | '\u{1e45}' | '\u{1e46}' | '\u{1e47}' | '\u{1e48}' | '\u{1e49}'
        | '\u{1e4a}' | '\u{1e4b}' => 'n',
        '\u{1e4c}' | '\u{1e4d}' | '\u{1e4e}' | '\u{1e4f}' | '\u{1e50}' | '\u{1e51}'
        | '\u{1e52}' | '\u{1e53}' | '\u{1ecc}' | '\u{1ecd}' | '\u{1ece}' | '\u{1ecf}'
        | '\u{1ed0}' | '\u{1ed1}' | '\u{1ed2}' | '\u{1ed3}' | '\u{1ed4}' | '\u{1ed5}'
        | '\u{1ed6}' | '\u{1ed7}' | '\u{1ed8}' | '\u{1ed9}' | '\u{1eda}' | '\u{1edb}'
        | '\u{1edc}' | '\u{1edd}' | '\u{1ede}' | '\u{1edf}' | '\u{1ee0}' | '\u{1ee1}'
        | '\u{1ee2}' | '\u{1ee3}' => 'o',
        '\u{1e54}' | '\u{1e55}' | '\u{1e56}' | '\u{1e57}' => 'p',
        '\u{1e58}' | '\u{1e59}' | '\u{1e5a}' | '\u{1e5b}' | '\u{1e5c}' | '\u{1e5d}'
        | '\u{1e5e}' | '\u{1e5f}' => 'r',
        '\u{1e60}' | '\u{1e61}' | '\u{1e62}' | '\u{1e63}' | '\u{1e64}' | '\u{1e65}'
        | '\u{1e66}' | '\u{1e67}' | '\u{1e68}' | '\u{1e69}' => 's',
        '\u{1e6a}' | '\u{1e6b}' | '\u{1e6c}' | '\u{1e6d}' | '\u{1e6e}' | '\u{1e6f}'
        | '\u{1e70}' | '\u{1e71}' | '\u{1e97}' => 't',
        '\u{1e72}' | '\u{1e73}' | '\u{1e74}' | '\u{1e75}' | '\u{1e76}' | '\u{1e77}'
        | '\u{1e78}' | '\u{1e79}' | '\u{1e7a}' | '\u{1e7b}' | '\u{1ee4}' | '\u{1ee5}'
        | '\u{1ee6}' | '\u{1ee7}' | '\u{1ee8}' | '\u{1ee9}' | '\u{1eea}' | '\u{1eeb}'
        | '\u{1eec}' | '\u{1eed}' | '\u{1eee}' | '\u{1eef}' | '\u{1ef0}' | '\u{1ef1}' => 'u',
        '\u{1e7c}' | '\u{1e7d}' | '\u{1e7e}' | '\u{1e7f}' => 'v',
        '\u{1e80}' | '\u{1e81}' | '\u{1e82}' | '\u{1e83}' | '\u{1e84}' | '\u{1e85}'
        | '\u{1e86}' | '\u{1e87}' | '\u{1e88}' | '\u{1e89}' | '\u{1e98}' => 'w',
        '\u{1e8a}' | '\u{1e8b}' | '\u{1e8c}' | '\u{1e8d}' => 'x',
        '\u{1e8e}' | '\u{1e8f}' | '\u{1e99}' | '\u{1ef2}' | '\u{1ef3}' | '\u{1ef4}'
        | '\u{1ef5}' | '\u{1ef6}' | '\u{1ef7}' | '\u{1ef8}' | '\u{1ef9}' => 'y',
        '\u{1e90}' | '\u{1e91}' | '\u{1e92}' | '\u{1e93}' | '\u{1e94}' | '\u{1e95}' => 'z',
        _ => return None,
    };
    Some(base)
}

/// Reduce text to ASCII for identifier derivation: an accented Latin letter maps to its base letter,
/// any remaining non-ASCII character is dropped, and ASCII characters pass through unchanged. The
/// caller's slug step then keeps only the identifier-valid characters.
#[cfg(feature = "rst")]
pub(crate) fn rst_asciify(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii() {
            out.push(ch);
        } else if let Some(base) = rst_ascii_base(ch) {
            out.push(base);
        }
    }
    out
}

/// The base ASCII letter an accented Latin letter reduces to, or `None` when the character has no
/// such base (ligatures, stroked letters, and non-Latin scripts are dropped).
// Laid out as parallel uppercase and lowercase blocks, each alphabetical by base letter, so the
// mapping stays auditable; an uppercase and a lowercase accent reducing to the same base letter are
// kept on separate lines rather than merged.
#[cfg(feature = "rst")]
#[allow(clippy::match_same_arms)]
fn rst_ascii_base(ch: char) -> Option<char> {
    let base = match ch {
        'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' | 'Ā' | 'Ă' | 'Ą' => 'a',
        'Ç' | 'Ć' | 'Č' | 'Ĉ' | 'Ċ' => 'c',
        'Ď' | 'Ḋ' => 'd',
        'È' | 'É' | 'Ê' | 'Ë' | 'Ē' | 'Ĕ' | 'Ė' | 'Ę' | 'Ě' => 'e',
        'Ĝ' | 'Ğ' | 'Ġ' | 'Ģ' => 'g',
        'Ĥ' => 'h',
        'Ì' | 'Í' | 'Î' | 'Ï' | 'Ĩ' | 'Ī' | 'Ĭ' | 'Į' | 'İ' => 'i',
        'Ĵ' => 'j',
        'Ķ' => 'k',
        'Ĺ' | 'Ļ' | 'Ľ' => 'l',
        'Ñ' | 'Ń' | 'Ņ' | 'Ň' => 'n',
        'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ō' | 'Ŏ' | 'Ő' => 'o',
        'Ŕ' | 'Ŗ' | 'Ř' => 'r',
        'Ś' | 'Ŝ' | 'Ş' | 'Š' => 's',
        'Ţ' | 'Ť' => 't',
        'Ù' | 'Ú' | 'Û' | 'Ü' | 'Ũ' | 'Ū' | 'Ŭ' | 'Ů' | 'Ű' | 'Ų' => 'u',
        'Ŵ' => 'w',
        'Ý' | 'Ŷ' | 'Ÿ' => 'y',
        'Ź' | 'Ż' | 'Ž' => 'z',
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'č' | 'ĉ' | 'ċ' => 'c',
        'ď' | 'ḋ' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
        'ĥ' => 'h',
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => 'i',
        'ĵ' => 'j',
        'ķ' => 'k',
        'ĺ' | 'ļ' | 'ľ' => 'l',
        'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ō' | 'ŏ' | 'ő' => 'o',
        'ŕ' | 'ŗ' | 'ř' => 'r',
        'ś' | 'ŝ' | 'ş' | 'š' => 's',
        'ţ' | 'ť' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'ŵ' => 'w',
        'ý' | 'ŷ' | 'ÿ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        _ => return None,
    };
    Some(base)
}

/// Fold text to ASCII by canonical decomposition, dropping every character that is not ASCII so a
/// letter carrying a diacritic keeps its base letter.
#[cfg(feature = "dokuwiki")]
pub(crate) fn dokuwiki_asciify(text: &str) -> String {
    text.nfd().filter(char::is_ascii).collect()
}

/// Transliterates text to ASCII for `ascii_identifiers`: an ASCII character is kept as is, a
/// character whose canonical decomposition begins with an ASCII letter or digit folds to that
/// character (so `é` becomes `e`), and any other non-ASCII character is dropped (so `Œ`, `ß`, and
/// `½` vanish). The result is then slugged like any other identifier.
#[cfg(feature = "mediawiki")]
pub(crate) fn transliterate_ascii(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii() {
            out.push(ch);
        } else if let Ok(index) = ASCII_FOLD.binary_search_by(|&(cp, _)| cp.cmp(&(ch as u32)))
            && let Some(&(_, byte)) = ASCII_FOLD.get(index)
        {
            out.push(byte as char);
        }
    }
    out
}

/// The ASCII fold for `ascii_identifiers`, keyed by Unicode code point and kept sorted for binary
/// search. Each entry maps a precomposed character to the ASCII letter or digit its canonical
/// decomposition begins with; characters with no ASCII base are absent and are dropped instead.
#[cfg(feature = "mediawiki")]
const ASCII_FOLD: &[(u32, u8)] = &[
    (0x00C0, b'a'),
    (0x00C1, b'a'),
    (0x00C2, b'a'),
    (0x00C3, b'a'),
    (0x00C4, b'a'),
    (0x00C5, b'a'),
    (0x00C7, b'c'),
    (0x00C8, b'e'),
    (0x00C9, b'e'),
    (0x00CA, b'e'),
    (0x00CB, b'e'),
    (0x00CC, b'i'),
    (0x00CD, b'i'),
    (0x00CE, b'i'),
    (0x00CF, b'i'),
    (0x00D1, b'n'),
    (0x00D2, b'o'),
    (0x00D3, b'o'),
    (0x00D4, b'o'),
    (0x00D5, b'o'),
    (0x00D6, b'o'),
    (0x00D9, b'u'),
    (0x00DA, b'u'),
    (0x00DB, b'u'),
    (0x00DC, b'u'),
    (0x00DD, b'y'),
    (0x00E0, b'a'),
    (0x00E1, b'a'),
    (0x00E2, b'a'),
    (0x00E3, b'a'),
    (0x00E4, b'a'),
    (0x00E5, b'a'),
    (0x00E7, b'c'),
    (0x00E8, b'e'),
    (0x00E9, b'e'),
    (0x00EA, b'e'),
    (0x00EB, b'e'),
    (0x00EC, b'i'),
    (0x00ED, b'i'),
    (0x00EE, b'i'),
    (0x00EF, b'i'),
    (0x00F1, b'n'),
    (0x00F2, b'o'),
    (0x00F3, b'o'),
    (0x00F4, b'o'),
    (0x00F5, b'o'),
    (0x00F6, b'o'),
    (0x00F9, b'u'),
    (0x00FA, b'u'),
    (0x00FB, b'u'),
    (0x00FC, b'u'),
    (0x00FD, b'y'),
    (0x00FF, b'y'),
    (0x0100, b'a'),
    (0x0101, b'a'),
    (0x0102, b'a'),
    (0x0103, b'a'),
    (0x0104, b'a'),
    (0x0105, b'a'),
    (0x0106, b'c'),
    (0x0107, b'c'),
    (0x0108, b'c'),
    (0x0109, b'c'),
    (0x010A, b'c'),
    (0x010B, b'c'),
    (0x010C, b'c'),
    (0x010D, b'c'),
    (0x010E, b'd'),
    (0x010F, b'd'),
    (0x0112, b'e'),
    (0x0113, b'e'),
    (0x0114, b'e'),
    (0x0115, b'e'),
    (0x0116, b'e'),
    (0x0117, b'e'),
    (0x0118, b'e'),
    (0x0119, b'e'),
    (0x011A, b'e'),
    (0x011B, b'e'),
    (0x011C, b'g'),
    (0x011D, b'g'),
    (0x011E, b'g'),
    (0x011F, b'g'),
    (0x0120, b'g'),
    (0x0121, b'g'),
    (0x0122, b'g'),
    (0x0123, b'g'),
    (0x0124, b'h'),
    (0x0125, b'h'),
    (0x0128, b'i'),
    (0x0129, b'i'),
    (0x012A, b'i'),
    (0x012B, b'i'),
    (0x012C, b'i'),
    (0x012D, b'i'),
    (0x012E, b'i'),
    (0x012F, b'i'),
    (0x0130, b'i'),
    (0x0134, b'j'),
    (0x0135, b'j'),
    (0x0136, b'k'),
    (0x0137, b'k'),
    (0x0139, b'l'),
    (0x013A, b'l'),
    (0x013B, b'l'),
    (0x013C, b'l'),
    (0x013D, b'l'),
    (0x013E, b'l'),
    (0x0143, b'n'),
    (0x0144, b'n'),
    (0x0145, b'n'),
    (0x0146, b'n'),
    (0x0147, b'n'),
    (0x0148, b'n'),
    (0x014C, b'o'),
    (0x014D, b'o'),
    (0x014E, b'o'),
    (0x014F, b'o'),
    (0x0150, b'o'),
    (0x0151, b'o'),
    (0x0154, b'r'),
    (0x0155, b'r'),
    (0x0156, b'r'),
    (0x0157, b'r'),
    (0x0158, b'r'),
    (0x0159, b'r'),
    (0x015A, b's'),
    (0x015B, b's'),
    (0x015C, b's'),
    (0x015D, b's'),
    (0x015E, b's'),
    (0x015F, b's'),
    (0x0160, b's'),
    (0x0161, b's'),
    (0x0162, b't'),
    (0x0163, b't'),
    (0x0164, b't'),
    (0x0165, b't'),
    (0x0168, b'u'),
    (0x0169, b'u'),
    (0x016A, b'u'),
    (0x016B, b'u'),
    (0x016C, b'u'),
    (0x016D, b'u'),
    (0x016E, b'u'),
    (0x016F, b'u'),
    (0x0170, b'u'),
    (0x0171, b'u'),
    (0x0172, b'u'),
    (0x0173, b'u'),
    (0x0174, b'w'),
    (0x0175, b'w'),
    (0x0176, b'y'),
    (0x0177, b'y'),
    (0x0178, b'y'),
    (0x0179, b'z'),
    (0x017A, b'z'),
    (0x017B, b'z'),
    (0x017C, b'z'),
    (0x017D, b'z'),
    (0x017E, b'z'),
    (0x01A0, b'o'),
    (0x01A1, b'o'),
    (0x01AF, b'u'),
    (0x01B0, b'u'),
    (0x01CD, b'a'),
    (0x01CE, b'a'),
    (0x01CF, b'i'),
    (0x01D0, b'i'),
    (0x01D1, b'o'),
    (0x01D2, b'o'),
    (0x01D3, b'u'),
    (0x01D4, b'u'),
    (0x01D5, b'u'),
    (0x01D6, b'u'),
    (0x01D7, b'u'),
    (0x01D8, b'u'),
    (0x01D9, b'u'),
    (0x01DA, b'u'),
    (0x01DB, b'u'),
    (0x01DC, b'u'),
    (0x01DE, b'a'),
    (0x01DF, b'a'),
    (0x01E0, b'a'),
    (0x01E1, b'a'),
    (0x01E6, b'g'),
    (0x01E7, b'g'),
    (0x01E8, b'k'),
    (0x01E9, b'k'),
    (0x01EA, b'o'),
    (0x01EB, b'o'),
    (0x01EC, b'o'),
    (0x01ED, b'o'),
    (0x01F0, b'j'),
    (0x01F4, b'g'),
    (0x01F5, b'g'),
    (0x01F8, b'n'),
    (0x01F9, b'n'),
    (0x01FA, b'a'),
    (0x01FB, b'a'),
    (0x0200, b'a'),
    (0x0201, b'a'),
    (0x0202, b'a'),
    (0x0203, b'a'),
    (0x0204, b'e'),
    (0x0205, b'e'),
    (0x0206, b'e'),
    (0x0207, b'e'),
    (0x0208, b'i'),
    (0x0209, b'i'),
    (0x020A, b'i'),
    (0x020B, b'i'),
    (0x020C, b'o'),
    (0x020D, b'o'),
    (0x020E, b'o'),
    (0x020F, b'o'),
    (0x0210, b'r'),
    (0x0211, b'r'),
    (0x0212, b'r'),
    (0x0213, b'r'),
    (0x0214, b'u'),
    (0x0215, b'u'),
    (0x0216, b'u'),
    (0x0217, b'u'),
    (0x0218, b's'),
    (0x0219, b's'),
    (0x021A, b't'),
    (0x021B, b't'),
    (0x021E, b'h'),
    (0x021F, b'h'),
    (0x0226, b'a'),
    (0x0227, b'a'),
    (0x0228, b'e'),
    (0x0229, b'e'),
    (0x022A, b'o'),
    (0x022B, b'o'),
    (0x022C, b'o'),
    (0x022D, b'o'),
    (0x022E, b'o'),
    (0x022F, b'o'),
    (0x0230, b'o'),
    (0x0231, b'o'),
    (0x0232, b'y'),
    (0x0233, b'y'),
    (0x1E00, b'a'),
    (0x1E01, b'a'),
    (0x1E02, b'b'),
    (0x1E03, b'b'),
    (0x1E04, b'b'),
    (0x1E05, b'b'),
    (0x1E06, b'b'),
    (0x1E07, b'b'),
    (0x1E08, b'c'),
    (0x1E09, b'c'),
    (0x1E0A, b'd'),
    (0x1E0B, b'd'),
    (0x1E0C, b'd'),
    (0x1E0D, b'd'),
    (0x1E0E, b'd'),
    (0x1E0F, b'd'),
    (0x1E10, b'd'),
    (0x1E11, b'd'),
    (0x1E12, b'd'),
    (0x1E13, b'd'),
    (0x1E14, b'e'),
    (0x1E15, b'e'),
    (0x1E16, b'e'),
    (0x1E17, b'e'),
    (0x1E18, b'e'),
    (0x1E19, b'e'),
    (0x1E1A, b'e'),
    (0x1E1B, b'e'),
    (0x1E1C, b'e'),
    (0x1E1D, b'e'),
    (0x1E1E, b'f'),
    (0x1E1F, b'f'),
    (0x1E20, b'g'),
    (0x1E21, b'g'),
    (0x1E22, b'h'),
    (0x1E23, b'h'),
    (0x1E24, b'h'),
    (0x1E25, b'h'),
    (0x1E26, b'h'),
    (0x1E27, b'h'),
    (0x1E28, b'h'),
    (0x1E29, b'h'),
    (0x1E2A, b'h'),
    (0x1E2B, b'h'),
    (0x1E2C, b'i'),
    (0x1E2D, b'i'),
    (0x1E2E, b'i'),
    (0x1E2F, b'i'),
    (0x1E30, b'k'),
    (0x1E31, b'k'),
    (0x1E32, b'k'),
    (0x1E33, b'k'),
    (0x1E34, b'k'),
    (0x1E35, b'k'),
    (0x1E36, b'l'),
    (0x1E37, b'l'),
    (0x1E38, b'l'),
    (0x1E39, b'l'),
    (0x1E3A, b'l'),
    (0x1E3B, b'l'),
    (0x1E3C, b'l'),
    (0x1E3D, b'l'),
    (0x1E3E, b'm'),
    (0x1E3F, b'm'),
    (0x1E40, b'm'),
    (0x1E41, b'm'),
    (0x1E42, b'm'),
    (0x1E43, b'm'),
    (0x1E44, b'n'),
    (0x1E45, b'n'),
    (0x1E46, b'n'),
    (0x1E47, b'n'),
    (0x1E48, b'n'),
    (0x1E49, b'n'),
    (0x1E4A, b'n'),
    (0x1E4B, b'n'),
    (0x1E4C, b'o'),
    (0x1E4D, b'o'),
    (0x1E4E, b'o'),
    (0x1E4F, b'o'),
    (0x1E50, b'o'),
    (0x1E51, b'o'),
    (0x1E52, b'o'),
    (0x1E53, b'o'),
    (0x1E54, b'p'),
    (0x1E55, b'p'),
    (0x1E56, b'p'),
    (0x1E57, b'p'),
    (0x1E58, b'r'),
    (0x1E59, b'r'),
    (0x1E5A, b'r'),
    (0x1E5B, b'r'),
    (0x1E5C, b'r'),
    (0x1E5D, b'r'),
    (0x1E5E, b'r'),
    (0x1E5F, b'r'),
    (0x1E60, b's'),
    (0x1E61, b's'),
    (0x1E62, b's'),
    (0x1E63, b's'),
    (0x1E64, b's'),
    (0x1E65, b's'),
    (0x1E66, b's'),
    (0x1E67, b's'),
    (0x1E68, b's'),
    (0x1E69, b's'),
    (0x1E6A, b't'),
    (0x1E6B, b't'),
    (0x1E6C, b't'),
    (0x1E6D, b't'),
    (0x1E6E, b't'),
    (0x1E6F, b't'),
    (0x1E70, b't'),
    (0x1E71, b't'),
    (0x1E72, b'u'),
    (0x1E73, b'u'),
    (0x1E74, b'u'),
    (0x1E75, b'u'),
    (0x1E76, b'u'),
    (0x1E77, b'u'),
    (0x1E78, b'u'),
    (0x1E79, b'u'),
    (0x1E7A, b'u'),
    (0x1E7B, b'u'),
    (0x1E7C, b'v'),
    (0x1E7D, b'v'),
    (0x1E7E, b'v'),
    (0x1E7F, b'v'),
    (0x1E80, b'w'),
    (0x1E81, b'w'),
    (0x1E82, b'w'),
    (0x1E83, b'w'),
    (0x1E84, b'w'),
    (0x1E85, b'w'),
    (0x1E86, b'w'),
    (0x1E87, b'w'),
    (0x1E88, b'w'),
    (0x1E89, b'w'),
    (0x1E8A, b'x'),
    (0x1E8B, b'x'),
    (0x1E8C, b'x'),
    (0x1E8D, b'x'),
    (0x1E8E, b'y'),
    (0x1E8F, b'y'),
    (0x1E90, b'z'),
    (0x1E91, b'z'),
    (0x1E92, b'z'),
    (0x1E93, b'z'),
    (0x1E94, b'z'),
    (0x1E95, b'z'),
    (0x1E96, b'h'),
    (0x1E97, b't'),
    (0x1E98, b'w'),
    (0x1E99, b'y'),
    (0x1EA0, b'a'),
    (0x1EA1, b'a'),
    (0x1EA2, b'a'),
    (0x1EA3, b'a'),
    (0x1EA4, b'a'),
    (0x1EA5, b'a'),
    (0x1EA6, b'a'),
    (0x1EA7, b'a'),
    (0x1EA8, b'a'),
    (0x1EA9, b'a'),
    (0x1EAA, b'a'),
    (0x1EAB, b'a'),
    (0x1EAC, b'a'),
    (0x1EAD, b'a'),
    (0x1EAE, b'a'),
    (0x1EAF, b'a'),
    (0x1EB0, b'a'),
    (0x1EB1, b'a'),
    (0x1EB2, b'a'),
    (0x1EB3, b'a'),
    (0x1EB4, b'a'),
    (0x1EB5, b'a'),
    (0x1EB6, b'a'),
    (0x1EB7, b'a'),
    (0x1EB8, b'e'),
    (0x1EB9, b'e'),
    (0x1EBA, b'e'),
    (0x1EBB, b'e'),
    (0x1EBC, b'e'),
    (0x1EBD, b'e'),
    (0x1EBE, b'e'),
    (0x1EBF, b'e'),
    (0x1EC0, b'e'),
    (0x1EC1, b'e'),
    (0x1EC2, b'e'),
    (0x1EC3, b'e'),
    (0x1EC4, b'e'),
    (0x1EC5, b'e'),
    (0x1EC6, b'e'),
    (0x1EC7, b'e'),
    (0x1EC8, b'i'),
    (0x1EC9, b'i'),
    (0x1ECA, b'i'),
    (0x1ECB, b'i'),
    (0x1ECC, b'o'),
    (0x1ECD, b'o'),
    (0x1ECE, b'o'),
    (0x1ECF, b'o'),
    (0x1ED0, b'o'),
    (0x1ED1, b'o'),
    (0x1ED2, b'o'),
    (0x1ED3, b'o'),
    (0x1ED4, b'o'),
    (0x1ED5, b'o'),
    (0x1ED6, b'o'),
    (0x1ED7, b'o'),
    (0x1ED8, b'o'),
    (0x1ED9, b'o'),
    (0x1EDA, b'o'),
    (0x1EDB, b'o'),
    (0x1EDC, b'o'),
    (0x1EDD, b'o'),
    (0x1EDE, b'o'),
    (0x1EDF, b'o'),
    (0x1EE0, b'o'),
    (0x1EE1, b'o'),
    (0x1EE2, b'o'),
    (0x1EE3, b'o'),
    (0x1EE4, b'u'),
    (0x1EE5, b'u'),
    (0x1EE6, b'u'),
    (0x1EE7, b'u'),
    (0x1EE8, b'u'),
    (0x1EE9, b'u'),
    (0x1EEA, b'u'),
    (0x1EEB, b'u'),
    (0x1EEC, b'u'),
    (0x1EED, b'u'),
    (0x1EEE, b'u'),
    (0x1EEF, b'u'),
    (0x1EF0, b'u'),
    (0x1EF1, b'u'),
    (0x1EF2, b'y'),
    (0x1EF3, b'y'),
    (0x1EF4, b'y'),
    (0x1EF5, b'y'),
    (0x1EF6, b'y'),
    (0x1EF7, b'y'),
    (0x1EF8, b'y'),
    (0x1EF9, b'y'),
    (0x212A, b'k'),
    (0x212B, b'a'),
];

#[cfg(all(test, any(feature = "man", feature = "org")))]
mod tests {
    use super::*;

    #[cfg(feature = "man")]
    #[test]
    fn ascii_fold_keeps_base_letters_and_drops_the_rest() {
        assert_eq!(fold_to_ascii("Café Münch"), "Cafe Munch");
        // A letter with no single-ASCII base is dropped, not transliterated.
        assert_eq!(fold_to_ascii("Straße"), "Strae");
        // A non-Latin script leaves nothing behind.
        assert_eq!(fold_to_ascii("Λόγος"), "");
        // Latin Extended Additional letters fold to their base; an accented letter folds to the
        // lowercase base (bare ASCII keeps its case), so the dotted capitals here become h and z.
        assert_eq!(fold_to_ascii("Việt Ḩ Ẓ"), "Viet h z");
        // A letter in that block with no single-ASCII base is dropped.
        assert_eq!(fold_to_ascii("ẞ"), "");
    }
}
