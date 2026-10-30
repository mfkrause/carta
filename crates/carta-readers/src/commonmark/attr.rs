//! Parse attribute blocks such as `{#id .class key="value"}`.
//!
//! Bare words invalidate the block. Later identifiers replace earlier ones; other attributes accumulate.

use carta_ast::Attr;

use super::scan::{char_at, is_ascii_punctuation};

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
    parse_attributes_bytes_with(s, 0, IdPolicy::Last, true)
}

/// Like [`parse_attributes`], but a repeated `#id` keeps the first.
pub(crate) fn parse_attributes_first_id(s: &str) -> Option<(Attr, usize)> {
    parse_attributes_bytes_with(s, 0, IdPolicy::First, true)
}

/// Parse an attribute block at byte offset `start` in `text`, which must begin with `{`. Returns the
/// parsed [`Attr`] and the byte offset just past the closing brace, or `None` when the block is not
/// well-formed (a bare-word token, or no closing brace). A repeated `#id` keeps the last.
/// `unescape_values` removes backslashes before ASCII punctuation in attribute values.
pub(crate) fn parse_attributes_bytes(
    text: &str,
    start: usize,
    unescape_values: bool,
) -> Option<(Attr, usize)> {
    parse_attributes_bytes_with(text, start, IdPolicy::Last, unescape_values)
}

fn parse_attributes_bytes_with(
    text: &str,
    start: usize,
    policy: IdPolicy,
    unescape_values: bool,
) -> Option<(Attr, usize)> {
    if char_at(text, start) != Some('{') {
        return None;
    }
    let mut index = start + 1;
    let mut attr = Attr::default();
    loop {
        skip_ws(text, &mut index);
        match char_at(text, index) {
            None => return None,
            Some('}') => {
                index += 1;
                break;
            }
            Some('#') => {
                index += 1;
                let id = read_token(text, &mut index);
                if id.is_empty() || id.contains('\\') {
                    return None;
                }
                if matches!(policy, IdPolicy::Last) || attr.id.is_empty() {
                    attr.id = id.into();
                }
            }
            Some('.') => {
                index += 1;
                let class = read_token(text, &mut index);
                if class.is_empty() || class.contains('\\') {
                    return None;
                }
                attr.classes.push(class.into());
            }
            Some(_) => {
                let key = read_key(text, &mut index);
                if key.is_empty() || key.contains('\\') || char_at(text, index) != Some('=') {
                    return None;
                }
                index += 1;
                let value = read_value(text, &mut index, unescape_values);
                attr.attributes.push((key.into(), value.into()));
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

fn skip_ws(text: &str, index: &mut usize) {
    while matches!(char_at(text, *index), Some(' ' | '\t' | '\n')) {
        *index += 1;
    }
}

/// Read an identifier or class token: everything up to whitespace or the closing brace.
fn read_token(text: &str, index: &mut usize) -> String {
    let mut out = String::new();
    while let Some(ch) = char_at(text, *index) {
        if is_token_end(ch) {
            break;
        }
        out.push(ch);
        *index += ch.len_utf8();
    }
    out
}

/// Read a key: everything up to `=`, whitespace, or the closing brace.
fn read_key(text: &str, index: &mut usize) -> String {
    let mut out = String::new();
    while let Some(ch) = char_at(text, *index) {
        if ch == '=' || is_token_end(ch) {
            break;
        }
        out.push(ch);
        *index += ch.len_utf8();
    }
    out
}

/// Read a quoted or bare value, optionally removing backslashes before ASCII punctuation.
fn read_value(text: &str, index: &mut usize, unescape_values: bool) -> String {
    let quote = char_at(text, *index).filter(|ch| matches!(ch, '"' | '\''));
    if quote.is_some() {
        *index += 1;
    }
    let mut out = String::new();
    while let Some(ch) = char_at(text, *index) {
        if Some(ch) == quote {
            *index += ch.len_utf8();
            break;
        }
        if quote.is_none() && is_token_end(ch) {
            break;
        }
        if unescape_values
            && ch == '\\'
            && let Some(escaped) = char_at(text, *index + 1)
            && is_ascii_punctuation(escaped)
        {
            out.push(escaped);
            *index += 1 + escaped.len_utf8();
            continue;
        }
        out.push(ch);
        *index += ch.len_utf8();
    }
    out
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
                ("key".into(), "val".into()),
                ("k2".into(), "two words".into())
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
        assert_eq!(attr("{key=}").0.attributes, [("key".into(), "".into())]);
    }

    #[test]
    fn single_quoted_and_escaped_double_quote() {
        assert_eq!(
            attr("{title='hi there'}").0.attributes,
            [("title".into(), "hi there".into())]
        );
        assert_eq!(
            attr(r#"{title="a \"q\" b"}"#).0.attributes,
            [("title".into(), r#"a "q" b"#.into())]
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
    fn backslashes_in_names_invalidate_attributes() {
        for source in [r"{#a\b}", r"{.a\b}", r"{k\b=v}", r"{.valid #a\_b}"] {
            assert!(parse_attributes(source).is_none(), "{source}");
        }
    }

    #[test]
    fn quoted_values_keep_backslashes_before_letters() {
        assert_eq!(
            attr(r#"{title="a\qb\_c"}"#).0.attributes,
            [("title".into(), "a\\qb_c".into())]
        );
    }

    #[test]
    fn value_escapes_depend_on_the_dialect() {
        for source in [r"{key=a\*b}", r#"{key="a\*b"}"#] {
            let (escaped, _) = super::parse_attributes_bytes(source, 0, true).unwrap();
            let (literal, _) = super::parse_attributes_bytes(source, 0, false).unwrap();
            assert_eq!(escaped.attributes, [("key".into(), "a*b".into())]);
            assert_eq!(literal.attributes, [("key".into(), "a\\*b".into())]);
        }
        assert!(super::parse_attributes_bytes(r#"{key="a\"b"}"#, 0, false).is_none());
    }

    #[test]
    fn empty_identifier_token_is_invalid() {
        // A bare `#` invalidates the whole attribute block.
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
