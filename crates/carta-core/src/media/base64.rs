//! Standard base64, the encoding embedded resources travel in inside text container formats.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// The line width binary payloads are wrapped to inside a notebook's JSON.
const LINE_WIDTH: usize = 76;

/// The standard base64 symbol for a 6-bit value; the mask keeps the index within the alphabet.
fn symbol(value: u32) -> char {
    ALPHABET
        .get((value & 0x3f) as usize)
        .map_or('A', |&byte| byte as char)
}

/// Encodes bytes as standard base64 with `=` padding and no line breaks.
#[must_use]
pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let (b0, b1, b2, len) = match chunk {
            [b0, b1, b2] => (*b0, *b1, *b2, 3),
            [b0, b1] => (*b0, *b1, 0, 2),
            [b0] => (*b0, 0, 0, 1),
            _ => continue,
        };
        let triple = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        out.push(symbol(triple >> 18));
        out.push(symbol(triple >> 12));
        out.push(if len > 1 { symbol(triple >> 6) } else { '=' });
        out.push(if len > 2 { symbol(triple) } else { '=' });
    }
    out
}

/// Encodes bytes as base64 wrapped to 76-character lines, each line — including the last —
/// terminated by a newline. This is the line-broken form a notebook stores binary payloads in. Empty
/// input yields an empty string (no lone newline).
#[must_use]
pub fn encode_mime(data: &[u8]) -> String {
    if data.is_empty() {
        return String::new();
    }
    let raw = encode(data);
    let mut out = String::with_capacity(raw.len() + raw.len() / LINE_WIDTH + 1);
    for (index, byte) in raw.bytes().enumerate() {
        if index != 0 && index % LINE_WIDTH == 0 {
            out.push('\n');
        }
        out.push(byte as char);
    }
    out.push('\n');
    out
}

/// Decodes standard base64, ignoring inner whitespace. Returns `None` when the input — once
/// whitespace is removed — is not well-formed: a length that is not a multiple of four, a symbol
/// outside the alphabet, or padding that does not fall at the very end of the final quartet.
#[must_use]
pub fn decode(input: &str) -> Option<Vec<u8>> {
    let symbols: Vec<u8> = input
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    if symbols.is_empty() {
        return Some(Vec::new());
    }
    if !symbols.len().is_multiple_of(4) {
        return None;
    }
    let group_count = symbols.len() / 4;
    let mut out = Vec::with_capacity(group_count * 3);
    for (index, chunk) in symbols.chunks_exact(4).enumerate() {
        let last = index + 1 == group_count;
        let &[a, b, c, d] = chunk else { return None };
        let v0 = sextet(a)?;
        let v1 = sextet(b)?;
        out.push((v0 << 2) | (v1 >> 4));
        if c == b'=' {
            if !last || d != b'=' {
                return None;
            }
            continue;
        }
        let v2 = sextet(c)?;
        out.push(((v1 & 0x0f) << 4) | (v2 >> 2));
        if d == b'=' {
            if !last {
                return None;
            }
            continue;
        }
        let v3 = sextet(d)?;
        out.push(((v2 & 0x03) << 6) | v3);
    }
    Some(out)
}

/// The 6-bit value of one standard base64 alphabet symbol, or `None` for any other byte.
fn sextet(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{decode, encode, encode_mime};

    #[test]
    fn decodes_and_ignores_whitespace() {
        assert_eq!(decode("aGVsbG8="), Some(b"hello".to_vec()));
        assert_eq!(decode("aGVs\nbG8="), Some(b"hello".to_vec()));
        assert_eq!(decode(""), Some(Vec::new()));
        // A length that is not a multiple of four, a non-alphabet byte, and misplaced padding each
        // fail to decode rather than silently dropping or truncating input.
        assert_eq!(decode("QQ"), None);
        assert_eq!(decode("aGVsbG8@"), None);
        assert_eq!(decode("a=VsbG8="), None);
    }

    #[test]
    fn encodes_with_padding() {
        assert_eq!(encode(b""), "");
        assert_eq!(encode(b"f"), "Zg==");
        assert_eq!(encode(b"fo"), "Zm8=");
        assert_eq!(encode(b"foo"), "Zm9v");
        assert_eq!(encode(b"hello"), "aGVsbG8=");
    }

    #[test]
    fn encode_decode_round_trips_all_byte_values() {
        let data: Vec<u8> = (0..=255).collect();
        assert_eq!(decode(&encode(&data)), Some(data));
    }

    #[test]
    fn mime_form_wraps_at_76_with_trailing_newline() {
        // 300 bytes -> 400 base64 chars -> five 76-char lines plus a 20-char line, each newline-ended.
        let data: Vec<u8> = (0..300u32)
            .map(|index| u8::try_from(index % 251).unwrap_or(0))
            .collect();
        let wrapped = encode_mime(&data);
        let lines: Vec<&str> = wrapped.split('\n').collect();
        // A trailing newline means split leaves an empty final element.
        assert_eq!(lines.last(), Some(&""));
        assert_eq!(lines.len(), 7);
        assert!(lines.iter().take(5).all(|line| line.len() == 76));
        assert_eq!(lines.get(5).map(|line| line.len()), Some(20));
        // The concatenation of the lines decodes back to the input.
        assert_eq!(decode(&wrapped), Some(data));
    }

    #[test]
    fn mime_form_of_empty_is_empty() {
        assert_eq!(encode_mime(b""), "");
    }
}
