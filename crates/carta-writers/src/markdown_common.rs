//! Block and inline helpers shared by the Markdown-family writer engines. Both the `CommonMark`
//! engine and the multi-dialect Markdown engine render these constructs identically, so the layout
//! and escaping logic lives here once.

use carta_ast::{Attr, Block, Format};

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
