//! Block and inline helpers shared by the Markdown-family writer engines. Both the `CommonMark`
//! engine and the multi-dialect Markdown engine render these constructs identically, so the layout
//! and escaping logic lives here once.

use carta_ast::{Attr, Block, Format, Inline, Target};

use crate::common::{Piece, is_uri, label_matches_url};

/// Whether an HTML comment must separate two consecutive blocks so the second is not absorbed into
/// the first: two lists of the same kind would merge into one, and an indented code block following
/// a list would read as a continuation of the final item.
pub(crate) fn needs_separator(previous: &Block, current: &Block) -> bool {
    match (previous, current) {
        (Block::BulletList(_), Block::BulletList(_))
        | (Block::OrderedList(..), Block::OrderedList(..)) => true,
        (Block::BulletList(_) | Block::OrderedList(..), Block::CodeBlock(attr, _)) => {
            attr_is_empty(attr)
        }
        _ => false,
    }
}

/// A list item whose first block is a horizontal rule cannot place the rule on the marker line,
/// where it would read as part of the marker; the rule is pushed onto its own line below an empty
/// marker line by prefixing the rendered body with a blank line.
pub(crate) fn offset_horizontal_rule(item: &[Block], body: String) -> String {
    if matches!(item.first(), Some(Block::HorizontalRule)) {
        format!("\n\n{body}")
    } else {
        body
    }
}

/// The deepest heading an ATX marker can express: `######`.
const MAX_ATX_HEADING_LEVEL: i32 = 6;

/// The `#` run opening an ATX heading. The level is clamped into the marker's expressible range,
/// which also bounds the allocation against an absurd level in the document model.
pub(crate) fn atx_heading_marker(level: i32) -> String {
    "#".repeat(usize::try_from(level.clamp(1, MAX_ATX_HEADING_LEVEL)).unwrap_or(1))
}

/// Prefix every line of a blockquote body with `> ` (a bare `>` on an otherwise empty line).
pub(crate) fn quote_block(body: &str) -> String {
    if body.is_empty() {
        return "> ".to_owned();
    }
    let mut out = String::new();
    for (index, line) in body.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if line.is_empty() {
            out.push('>');
        } else {
            out.push_str("> ");
            out.push_str(line);
        }
    }
    out
}

/// Whether a raw node targets HTML and should pass its content through verbatim.
pub(crate) fn is_html_format(format: &Format) -> bool {
    matches!(format.0.as_str(), "html" | "html4" | "html5")
}

/// An indented code block: every non-blank line is prefixed with four spaces, blank lines stay
/// empty, and trailing blank lines are dropped. Empty content yields no output.
pub(crate) fn indent_code(text: &str) -> String {
    let body = text.trim_end_matches('\n');
    let mut out = String::new();
    for (index, line) in body.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if !line.is_empty() {
            out.push_str("    ");
            out.push_str(line);
        }
    }
    out
}

pub(crate) fn longest_backtick_run(text: &str) -> usize {
    let mut longest = 0;
    let mut current = 0;
    for ch in text.chars() {
        if ch == '`' {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    longest
}

pub(crate) fn attr_is_empty(attr: &Attr) -> bool {
    attr.id.is_empty() && attr.classes.is_empty() && attr.attributes.is_empty()
}

/// Whether a link's attributes consist solely of the `uri` or `email` class that marks it as an
/// autolink: with no id and no further attributes, such a link is written in angle-bracket form.
pub(crate) fn is_autolink_class(attr: &Attr) -> bool {
    attr.id.is_empty()
        && attr.attributes.is_empty()
        && matches!(attr.classes.as_slice(), [class] if class == "uri" || class == "email")
}

/// An inline code span, delimited by a backtick run one longer than the longest run it contains
/// (at least one). A single space pads each side exactly when the content holds a backtick, so the
/// delimiters and the embedded backtick stay distinct; content that merely has leading or trailing
/// spaces (or is entirely spaces) is wrapped without extra padding.
pub(crate) fn code_span(text: &str) -> String {
    let max_run = longest_backtick_run(text);
    let fence = "`".repeat((max_run + 1).max(1));
    if max_run > 0 {
        format!("{fence} {text} {fence}")
    } else {
        format!("{fence}{text}{fence}")
    }
}

/// The `(url "title")` destination tail of a link or image, with the title omitted when empty.
pub(crate) fn destination(target: &Target) -> String {
    if target.title.is_empty() {
        target.url.to_string()
    } else {
        format!("{} \"{}\"", target.url, target.title)
    }
}

/// The angle-bracket autolink form when a link's single-`Str` text is the visible form of its URL —
/// the URL itself or its percent-decoded form, for a genuine URI — or the address of a `mailto:`
/// URL, else `None`. The angle-bracket form carries the encoded URL, not the decoded text.
pub(crate) fn autolink(inlines: &[Inline], target: &Target) -> Option<String> {
    let [Inline::Str(text)] = inlines else {
        return None;
    };
    if is_uri(&target.url) && label_matches_url(text, &target.url) {
        return Some(format!("<{}>", target.url));
    }
    if target.url == format!("mailto:{text}") {
        return Some(format!("<{text}>"));
    }
    None
}

/// Append a raw-HTML run to the fill pieces. When `breakable`, the spaces separating a tag's
/// attributes become wrap points so a long tag may fold across lines, while a space inside a quoted
/// attribute value belongs to the value and stays put. Otherwise the run is one unbreakable piece.
/// Either way a non-space run abutting the next piece (a tag's `>` and the text after it) stays one
/// word.
pub(crate) fn push_html(out: &mut Vec<Piece>, html: &str, breakable: bool) {
    if !breakable {
        out.push(Piece::text(html.to_owned()));
        return;
    }
    let mut tokens: Vec<String> = vec![String::new()];
    let mut in_quote = false;
    for ch in html.chars() {
        match ch {
            '"' => {
                in_quote = !in_quote;
                if let Some(last) = tokens.last_mut() {
                    last.push(ch);
                }
            }
            ' ' if !in_quote => tokens.push(String::new()),
            _ => {
                if let Some(last) = tokens.last_mut() {
                    last.push(ch);
                }
            }
        }
    }
    for (index, part) in tokens.iter().enumerate() {
        if index > 0 {
            out.push(Piece::Space);
        }
        if !part.is_empty() {
            out.push(Piece::text(part.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn str_inlines(text: &str) -> Vec<Inline> {
        vec![Inline::Str(text.to_owned().into())]
    }

    #[test]
    fn code_span_pads_only_when_backtick_bearing() {
        // Backtick-free content is wrapped with no padding, whatever its spacing.
        assert_eq!(code_span(""), "``");
        assert_eq!(code_span("plain"), "`plain`");
        assert_eq!(code_span("   "), "`   `");
        assert_eq!(code_span(" x "), "` x `");
        assert_eq!(code_span(" and "), "` and `");
        assert_eq!(code_span(" x"), "` x`");
        assert_eq!(code_span("x "), "`x `");
        // A backtick anywhere forces a single space of padding and a longer fence.
        assert_eq!(code_span("`x"), "`` `x ``");
        assert_eq!(code_span("x`"), "`` x` ``");
        assert_eq!(code_span("`x`"), "`` `x` ``");
        assert_eq!(code_span("a`b"), "`` a`b ``");
        assert_eq!(code_span("a``b"), "``` a``b ```");
        assert_eq!(code_span("`"), "`` ` ``");
        assert_eq!(longest_backtick_run("a``b`c"), 2);
    }

    #[test]
    fn destination_writes_title_verbatim() {
        assert_eq!(
            destination(&Target {
                url: "/p".into(),
                title: String::new().into()
            }),
            "/p"
        );
        assert_eq!(
            destination(&Target {
                url: "/p".into(),
                title: "T".into()
            }),
            "/p \"T\""
        );
        // A quote in the title passes through unescaped between the delimiting quotes.
        assert_eq!(
            destination(&Target {
                url: "/p".into(),
                title: "a\"b".into()
            }),
            "/p \"a\"b\""
        );
    }

    #[test]
    fn autolink_for_uri_and_mailto() {
        let uri = Target {
            url: "http://example.com".into(),
            title: String::new().into(),
        };
        assert_eq!(
            autolink(&str_inlines("http://example.com"), &uri),
            Some("<http://example.com>".to_owned())
        );
        let mail = Target {
            url: "mailto:a@b.com".into(),
            title: String::new().into(),
        };
        assert_eq!(
            autolink(&str_inlines("a@b.com"), &mail),
            Some("<a@b.com>".to_owned())
        );
        let plain = Target {
            url: "http://other".into(),
            title: String::new().into(),
        };
        assert_eq!(autolink(&str_inlines("text"), &plain), None);
    }

    #[test]
    fn autolink_accepts_percent_decoded_label() {
        let target = Target {
            url: "http://e.com/a%20b".into(),
            title: String::new().into(),
        };
        // The percent-decoded visible form of the URL still yields the angle-bracket form, which
        // carries the encoded URL.
        assert_eq!(
            autolink(&str_inlines("http://e.com/a b"), &target),
            Some("<http://e.com/a%20b>".to_owned())
        );
    }
}
