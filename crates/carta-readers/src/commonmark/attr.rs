//! Pandoc-style attribute blocks: `{#id .class key=value key="quoted value"}`.
//!
//! Shared by the header, fenced-code, inline-code, link, and span attribute extensions. A token is an
//! identifier (`#…`), a class (`.…`), or a `key=value` pair; any other bare word makes the whole block
//! invalid, in which case the surrounding `{…}` stays literal text. A later `#id` overrides an earlier
//! one; classes and key/value pairs accumulate in source order.

use carta_ast::Attr;

/// Which identifier survives when an attribute block carries more than one `#id` token.
#[derive(Clone, Copy)]
pub(crate) enum IdPolicy {
    /// Keep the first identifier; later ones are dropped (fenced-div openers).
    First,
    /// Keep the last identifier, overriding earlier ones (headers, code, spans, links).
    Last,
}

/// Parse an attribute block at the start of `s`, which must begin with `{`. Returns the parsed
/// [`Attr`] and the number of bytes consumed (both braces included), or `None` when `s` does not open
/// a well-formed block (a bare-word token, or no closing brace). A repeated `#id` keeps the last.
pub(crate) fn parse_attributes(s: &str) -> Option<(Attr, usize)> {
    parse_attributes_with(s, IdPolicy::Last)
}

/// Like [`parse_attributes`], but a repeated `#id` keeps the first.
pub(crate) fn parse_attributes_first_id(s: &str) -> Option<(Attr, usize)> {
    parse_attributes_with(s, IdPolicy::First)
}

fn parse_attributes_with(s: &str, policy: IdPolicy) -> Option<(Attr, usize)> {
    let chars: Vec<char> = s.chars().collect();
    let (attr, end) = parse_attributes_chars_with(&chars, 0, policy)?;
    let consumed = chars
        .get(..end)
        .map_or(0, |head| head.iter().map(|ch| ch.len_utf8()).sum());
    Some((attr, consumed))
}

/// Parse an attribute block at `chars[start..]`, which must begin with `{`. Returns the parsed
/// [`Attr`] and the char index just past the closing brace, or `None` when the block is not
/// well-formed (a bare-word token, or no closing brace). A repeated `#id` keeps the last.
pub(crate) fn parse_attributes_chars(chars: &[char], start: usize) -> Option<(Attr, usize)> {
    parse_attributes_chars_with(chars, start, IdPolicy::Last)
}

fn parse_attributes_chars_with(
    chars: &[char],
    start: usize,
    policy: IdPolicy,
) -> Option<(Attr, usize)> {
    if chars.get(start).copied() != Some('{') {
        return None;
    }
    let mut index = start + 1;
    let mut attr = Attr::default();
    loop {
        skip_ws(chars, &mut index);
        match chars.get(index).copied() {
            None => return None,
            Some('}') => {
                index += 1;
                break;
            }
            Some('#') => {
                index += 1;
                let id = read_token(chars, &mut index);
                if id.is_empty() {
                    return None;
                }
                if matches!(policy, IdPolicy::Last) || attr.id.is_empty() {
                    attr.id = id;
                }
            }
            Some('.') => {
                index += 1;
                let class = read_token(chars, &mut index);
                if class.is_empty() {
                    return None;
                }
                attr.classes.push(class);
            }
            Some(_) => {
                let key = read_key(chars, &mut index);
                if key.is_empty() || chars.get(index).copied() != Some('=') {
                    return None;
                }
                index += 1;
                let value = read_value(chars, &mut index);
                attr.attributes.push((key, value));
            }
        }
    }
    Some((attr, index))
}

/// Whether `attr` carries any identifier, class, or key/value pair. An attribute block that parses
/// to nothing (`{}`) is not consumed by inline attribute targets.
pub(crate) fn is_non_empty(attr: &Attr) -> bool {
    !attr.id.is_empty() || !attr.classes.is_empty() || !attr.attributes.is_empty()
}

/// Merge `extra` into `into`: the first non-empty identifier is kept, while classes and key/value
/// pairs accumulate in source order. Used when consecutive attribute blocks attach to one target.
pub(crate) fn merge(into: &mut Attr, extra: Attr) {
    if into.id.is_empty() {
        into.id = extra.id;
    }
    into.classes.extend(extra.classes);
    into.attributes.extend(extra.attributes);
}

fn is_token_end(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n' | '}')
}

fn skip_ws(chars: &[char], index: &mut usize) {
    while matches!(chars.get(*index).copied(), Some(' ' | '\t' | '\n')) {
        *index += 1;
    }
}

/// Read an identifier or class token: everything up to whitespace or the closing brace.
fn read_token(chars: &[char], index: &mut usize) -> String {
    let mut out = String::new();
    while let Some(&ch) = chars.get(*index).filter(|&&c| !is_token_end(c)) {
        out.push(ch);
        *index += 1;
    }
    out
}

/// Read a key: everything up to `=`, whitespace, or the closing brace.
fn read_key(chars: &[char], index: &mut usize) -> String {
    let mut out = String::new();
    while let Some(&ch) = chars.get(*index).filter(|&&c| c != '=' && !is_token_end(c)) {
        out.push(ch);
        *index += 1;
    }
    out
}

/// Read a value: a quoted string (with backslash escapes) or a bare run up to whitespace/`}`.
fn read_value(chars: &[char], index: &mut usize) -> String {
    match chars.get(*index).copied() {
        Some(quote @ ('"' | '\'')) => {
            *index += 1;
            let mut out = String::new();
            while let Some(&ch) = chars.get(*index) {
                if ch == '\\'
                    && let Some(&escaped) = chars.get(*index + 1)
                {
                    out.push(escaped);
                    *index += 2;
                    continue;
                }
                *index += 1;
                if ch == quote {
                    break;
                }
                out.push(ch);
            }
            out
        }
        _ => read_token(chars, index),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_attributes;

    fn attr(s: &str) -> (carta_ast::Attr, usize) {
        parse_attributes(s).expect("well-formed attribute block")
    }

    #[test]
    fn id_classes_and_key_values() {
        let (a, consumed) = attr(r#"{#sec .a .b key=val k2="two words"}"#);
        assert_eq!(a.id, "sec");
        assert_eq!(a.classes, ["a", "b"]);
        assert_eq!(
            a.attributes,
            [
                ("key".to_owned(), "val".to_owned()),
                ("k2".to_owned(), "two words".to_owned())
            ]
        );
        assert_eq!(consumed, r#"{#sec .a .b key=val k2="two words"}"#.len());
    }

    #[test]
    fn last_id_wins() {
        assert_eq!(attr("{#one #two}").0.id, "two");
    }

    #[test]
    fn first_id_wins_under_the_first_policy() {
        let (a, _) = super::parse_attributes_first_id("{#one #two}").expect("well-formed block");
        assert_eq!(a.id, "one");
        // Classes and pairs still accumulate; only the identifier precedence differs.
        let (b, _) = super::parse_attributes_first_id("{.a #x .b #y}").expect("well-formed block");
        assert_eq!(b.id, "x");
        assert_eq!(b.classes, ["a", "b"]);
    }

    #[test]
    fn empty_block_is_valid() {
        let (a, consumed) = attr("{}");
        assert!(a.id.is_empty() && a.classes.is_empty() && a.attributes.is_empty());
        assert_eq!(consumed, 2);
    }

    #[test]
    fn empty_value_after_equals() {
        assert_eq!(
            attr("{key=}").0.attributes,
            [("key".to_owned(), String::new())]
        );
    }

    #[test]
    fn single_quoted_and_escaped_double_quote() {
        assert_eq!(
            attr("{title='hi there'}").0.attributes,
            [("title".to_owned(), "hi there".to_owned())]
        );
        assert_eq!(
            attr(r#"{title="a \"q\" b"}"#).0.attributes,
            [("title".to_owned(), r#"a "q" b"#.to_owned())]
        );
    }

    #[test]
    fn dotted_id_and_dashed_class() {
        assert_eq!(attr("{#a.b-c_d}").0.id, "a.b-c_d");
        assert_eq!(attr("{.foo-bar .ns:cls}").0.classes, ["foo-bar", "ns:cls"]);
    }

    #[test]
    fn surrounding_whitespace_ignored() {
        let (a, _) = attr("{  .a   #b  }");
        assert_eq!(a.id, "b");
        assert_eq!(a.classes, ["a"]);
    }

    #[test]
    fn bare_word_is_invalid() {
        assert!(parse_attributes("{foo}").is_none());
        assert!(parse_attributes("{#x foo}").is_none());
        assert!(parse_attributes("{.a !!}").is_none());
    }

    #[test]
    fn empty_identifier_token_is_invalid() {
        // A `#` with no following token is not a valid identifier, so the whole block fails to
        // parse — even when a later token would be well-formed.
        assert!(parse_attributes("{#}").is_none());
        assert!(parse_attributes("{# #b}").is_none());
        assert!(parse_attributes("{#a #}").is_none());
    }

    #[test]
    fn unterminated_block_is_invalid() {
        assert!(parse_attributes("{#x").is_none());
        assert!(parse_attributes("not an attr").is_none());
    }

    #[test]
    fn consumed_length_stops_at_closing_brace() {
        let (_, consumed) = attr("{.a} trailing");
        assert_eq!(consumed, 4);
    }
}
