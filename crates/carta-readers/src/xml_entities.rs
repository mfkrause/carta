//! Decoding of XML entity references — the five predefined named entities and numeric character
//! references — shared by the readers that scan XML text.

/// Replaces the five predefined XML entities and numeric character references, preserving multi-byte
/// UTF-8. XML defines no other named entities, so a name outside the five — and any malformed or
/// out-of-range numeric reference — is left verbatim.
pub(crate) fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(rest.get(..amp).unwrap_or(""));
        let tail = rest.get(amp..).unwrap_or("");
        if let Some(semi) = tail.find(';')
            && let Some(name) = tail.get(1..semi)
            && let Some(decoded) = decode_reference(name)
        {
            out.push(decoded);
            rest = tail.get(semi + 1..).unwrap_or("");
            continue;
        }
        out.push('&');
        rest = tail.get(1..).unwrap_or("");
    }
    out.push_str(rest);
    out
}

/// Resolves the body of a reference (the text between `&` and `;`) to its character, or `None` when
/// it is neither a predefined name nor a valid numeric reference.
fn decode_reference(body: &str) -> Option<char> {
    match body {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            let digits = body.strip_prefix('#')?;
            let code = match digits.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => digits.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::decode_entities;

    #[test]
    fn decode_entities_leaves_unknown_and_malformed_verbatim() {
        assert_eq!(
            decode_entities("&copy; &amp; &#xZZ; &nosemi"),
            "&copy; & &#xZZ; &nosemi"
        );
        assert_eq!(decode_entities("plain"), "plain");
    }
}
