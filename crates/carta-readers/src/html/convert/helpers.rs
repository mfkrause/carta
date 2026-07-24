//! Inline run assembly, attribute extraction, tag serialization, and block-flow helpers.

use carta_ast::{
    Attr, Block, Inline, ListAttributes, ListNumberDelim, ListNumberStyle, MathType, Target,
};

use super::super::tree::{Element, Node, attr_value, style_property};

/// Merge a finished inline into a run, joining adjacent strings and collapsing adjacent breaks the
/// way [`push_text`] does, so a smart pass over one text node fuses cleanly with its neighbors.
pub(super) fn absorb(out: &mut Vec<Inline>, inline: Inline) {
    match inline {
        Inline::Str(text) => push_str(out, &text),
        Inline::Space => push_break(out, false),
        Inline::SoftBreak => push_break(out, true),
        other => out.push(other),
    }
}

/// Push a formatting inline, fusing it with an identical formatting element directly before it.
/// Adjacent runs of the same emphasis, strength, strike, underline, super-, or subscript (with no
/// intervening text or break) carry one meaning, so their children are concatenated into a single
/// element. Quotation and small-caps stay separate; any other inline is appended as is.
pub(super) fn merge_adjacent_formatting(out: &mut Vec<Inline>, inline: Inline) {
    let mergeable = matches!(
        (out.last(), &inline),
        (Some(Inline::Emph(_)), Inline::Emph(_))
            | (Some(Inline::Strong(_)), Inline::Strong(_))
            | (Some(Inline::Strikeout(_)), Inline::Strikeout(_))
            | (Some(Inline::Underline(_)), Inline::Underline(_))
            | (Some(Inline::Superscript(_)), Inline::Superscript(_))
            | (Some(Inline::Subscript(_)), Inline::Subscript(_))
    );
    if !mergeable {
        out.push(inline);
        return;
    }
    // Both sides are the same formatting variant, so the children concatenate onto the previous.
    let next = match inline {
        Inline::Emph(children)
        | Inline::Strong(children)
        | Inline::Strikeout(children)
        | Inline::Underline(children)
        | Inline::Superscript(children)
        | Inline::Subscript(children) => children,
        other => {
            out.push(other);
            return;
        }
    };
    if let Some(
        Inline::Emph(prev)
        | Inline::Strong(prev)
        | Inline::Strikeout(prev)
        | Inline::Underline(prev)
        | Inline::Superscript(prev)
        | Inline::Subscript(prev),
    ) = out.last_mut()
    {
        prev.extend(next);
    }
}

/// Percent-escape a URL reference so characters that are unsafe or structural in a URL survive as a
/// valid reference. Whitespace and the delimiters `<>|"{}[]^` and a backtick are encoded per UTF-8
/// byte as uppercase `%XX`; every other character, including an existing `%`, a backslash, a tilde,
/// and any non-ASCII character, is kept verbatim.
pub(crate) fn escape_uri(uri: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(uri.len());
    let mut buf = [0u8; 4];
    for c in uri.chars() {
        if c.is_whitespace()
            || matches!(c, '<' | '>' | '|' | '"' | '{' | '}' | '[' | ']' | '^' | '`')
        {
            for &byte in c.encode_utf8(&mut buf).as_bytes() {
                let _ = write!(out, "%{byte:02X}");
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Collapse every run of ASCII whitespace in an inline code span to a single space, without trimming
/// the edges: an all-whitespace span becomes a single space, and leading and trailing whitespace each
/// survive as one space.
pub(super) fn collapse_ws(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_space = false;
    for ch in text.chars() {
        if ch.is_ascii_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out
}

/// Whether a `<span>` requests small-caps rendering, either through the `smallcaps` class or a
/// `font-variant: small-caps` style declaration.
pub(super) fn is_small_caps(e: &Element) -> bool {
    if e.attrs
        .iter()
        .any(|(key, value)| key == "class" && value.split_whitespace().any(|c| c == "smallcaps"))
    {
        return true;
    }
    attr_value(e, "style")
        .and_then(|style| style_property(&style, "font-variant"))
        .is_some_and(|value| value.eq_ignore_ascii_case("small-caps"))
}

/// Split an anchor's content into the whitespace at its edges and the trimmed middle. Leading and
/// trailing breaks belong outside the anchor in HTML rendering, so they are returned to the caller to
/// place around it.
pub(super) fn hoist_edge_whitespace(
    mut inlines: Vec<Inline>,
) -> (Option<Inline>, Vec<Inline>, Option<Inline>) {
    let leading = if matches!(inlines.first(), Some(Inline::Space | Inline::SoftBreak)) {
        Some(inlines.remove(0))
    } else {
        None
    };
    let trailing = if matches!(inlines.last(), Some(Inline::Space | Inline::SoftBreak)) {
        inlines.pop()
    } else {
        None
    };
    (leading, inlines, trailing)
}

/// Serialize an element's start tag, e.g. `<cite id="1">`. Attribute names are emitted in source
/// order; a value-less attribute is written bare, and special characters in values are escaped.
pub(super) fn open_tag(e: &Element) -> String {
    let mut out = String::from("<");
    out.push_str(&e.name);
    for (key, value) in &e.attrs {
        out.push(' ');
        out.push_str(key);
        if !value.is_empty() {
            out.push_str("=\"");
            push_escaped_attr_value(&mut out, value);
            out.push('"');
        }
    }
    out.push('>');
    out
}

/// Serialize an element's end tag, e.g. `</cite>`.
pub(super) fn close_tag(name: &str) -> String {
    let mut out = String::from("</");
    out.push_str(name);
    out.push('>');
    out
}

fn push_escaped_attr_value(out: &mut String, value: &str) {
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
}

pub(super) fn image(e: &Element) -> Inline {
    let url = attr_value(e, "src").unwrap_or_default();
    let title = attr_value(e, "title").unwrap_or_default();
    let alt = attr_value(e, "alt").unwrap_or_default();
    let attr = extract_attr(e, &["src", "title", "alt"]);
    Inline::Image(
        Box::new(attr),
        inlines_from_text(&alt),
        Box::new(Target {
            url: escape_uri(&url).into(),
            title: title.into(),
        }),
    )
}

pub(super) fn extract_attr(e: &Element, exclude: &[&str]) -> Attr {
    let mut attr = Attr::default();
    for (key, value) in &e.attrs {
        if exclude.contains(&key.as_str()) {
            continue;
        }
        match key.as_str() {
            "id" => attr.id = value.as_str().into(),
            "class" => attr.classes = value.split_whitespace().map(Into::into).collect(),
            _ => {
                let name = key.strip_prefix("data-").unwrap_or(key).to_string();
                attr.attributes.push((name.into(), value.clone().into()));
            }
        }
    }
    attr
}

pub(super) fn is_line_block_div(e: &Element) -> bool {
    let attr = extract_attr(e, &[]);
    attr.id.is_empty() && attr.attributes.is_empty() && attr.classes == ["line-block"]
}

pub(super) fn div_attr(e: &Element, sectioning: bool) -> Attr {
    let mut attr = extract_attr(e, &[]);
    if sectioning {
        attr.classes.insert(0, e.name.clone().into());
    }
    attr
}

pub(super) fn merge_attr(base: &mut Attr, other: Attr) {
    if base.id.is_empty() {
        base.id = other.id;
    }
    base.classes.extend(other.classes);
    base.attributes.extend(other.attributes);
}

pub(super) fn list_attributes(e: &Element) -> ListAttributes {
    let start = attr_value(e, "start")
        .and_then(|s| s.trim().parse::<i32>().ok())
        .unwrap_or(1);
    let style = match attr_value(e, "type").as_deref() {
        Some("1") => ListNumberStyle::Decimal,
        Some("a") => ListNumberStyle::LowerAlpha,
        Some("A") => ListNumberStyle::UpperAlpha,
        Some("i") => ListNumberStyle::LowerRoman,
        Some("I") => ListNumberStyle::UpperRoman,
        _ => ListNumberStyle::DefaultStyle,
    };
    ListAttributes {
        start,
        style,
        delim: ListNumberDelim::DefaultDelim,
    }
}

pub(super) fn inlines_from_text(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    push_text(&mut out, text);
    out
}

/// Append `text` to an inline run, collapsing each whitespace span to a single break: a span that
/// spans a line is a soft break, otherwise a space. Non-whitespace merges into the trailing string.
pub(super) fn push_text(out: &mut Vec<Inline>, text: &str) {
    // Boundaries are ASCII whitespace, so byte scanning is exact and each non-whitespace span
    // appends in one step.
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(&byte) = bytes.get(i) {
        if byte.is_ascii_whitespace() {
            let mut newline = false;
            while let Some(&w) = bytes.get(i) {
                if !w.is_ascii_whitespace() {
                    break;
                }
                newline |= w == b'\n';
                i += 1;
            }
            push_break(out, newline);
        } else {
            let start = i;
            while bytes.get(i).is_some_and(|&b| !b.is_ascii_whitespace()) {
                i += 1;
            }
            if let Some(span) = text.get(start..i) {
                push_str(out, span);
            }
        }
    }
}

pub(super) fn push_str(out: &mut Vec<Inline>, word: &str) {
    if let Some(Inline::Str(existing)) = out.last_mut() {
        existing.push_str(word);
    } else {
        out.push(Inline::Str(word.into()));
    }
}

fn push_break(out: &mut Vec<Inline>, newline: bool) {
    match out.last() {
        Some(Inline::SoftBreak) => {}
        Some(Inline::Space) => {
            if newline {
                out.pop();
                out.push(Inline::SoftBreak);
            }
        }
        _ => out.push(if newline {
            Inline::SoftBreak
        } else {
            Inline::Space
        }),
    }
}

/// Whether a `<tr>` is a header row: it carries at least one cell and every cell is a `<th>`. Such a
/// leading row stands in for a missing `<thead>`.
pub(super) fn is_header_row(tr: &Element) -> bool {
    let mut saw_header = false;
    for node in &tr.children {
        if let Node::Element(cell) = node {
            match cell.name.as_str() {
                "td" => return false,
                "th" => saw_header = true,
                _ => {}
            }
        }
    }
    saw_header
}

/// The break to place beside a formatting element when the given edge inline is whitespace: a
/// [`Inline::Space`] for a space, an [`Inline::SoftBreak`] for a soft break, nothing otherwise.
pub(super) fn edge_break(edge: Option<&Inline>) -> Option<Inline> {
    match edge {
        Some(Inline::Space) => Some(Inline::Space),
        Some(Inline::SoftBreak) => Some(Inline::SoftBreak),
        _ => None,
    }
}

pub(super) fn trim_inlines(mut inlines: Vec<Inline>) -> Vec<Inline> {
    while matches!(inlines.first(), Some(Inline::Space | Inline::SoftBreak)) {
        inlines.remove(0);
    }
    while matches!(inlines.last(), Some(Inline::Space | Inline::SoftBreak)) {
        inlines.pop();
    }
    inlines
}

/// Flush a loose inline run as a `Plain` block, dropping it when only whitespace remains.
pub(super) fn flush(pending: &mut Vec<Inline>, out: &mut Vec<Block>) {
    let trimmed = trim_inlines(std::mem::take(pending));
    if !trimmed.is_empty() {
        out.push(Block::Plain(trimmed));
    }
}

/// How a container shapes the loose inline runs directly inside it.
///
/// A run of inline content between block-level siblings is captured as a `Block::Plain`. Whether that
/// `Plain` stays plain or is promoted to a full `Block::Para` depends on the container:
///
/// - `Prose`: a paragraph-carrying flow such as the document body or a blockquote. A `Plain` is
///   promoted whenever any sibling is paragraph-like, and a nested list counts as such a sibling.
/// - `Item`: a list item or definition, where a bare run reads as tight text. Promotion still
///   happens next to a paragraph-like sibling, but a nested list alone does not force it.
/// - `Framed`: a structural wrapper (a `div`, figure, caption, or table cell) that preserves its
///   runs verbatim: a `Plain` is never promoted, keeping tight text tight regardless of its siblings.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flow {
    Prose,
    Item,
    Framed,
}

/// A loose inline run is a `Plain` block until a paragraph-like sibling promotes the whole group to
/// `Para`. The container's [`Flow`] decides whether promotion applies and how nested lists count.
pub(super) fn fix_plains(blocks: Vec<Block>, flow: Flow) -> Vec<Block> {
    if flow == Flow::Framed {
        return blocks;
    }
    let in_list = flow == Flow::Item;
    if !blocks.iter().any(|block| is_paraish(block, in_list)) {
        return blocks;
    }
    blocks
        .into_iter()
        .map(|block| match block {
            Block::Plain(inlines) => Block::Para(inlines),
            other => other,
        })
        .collect()
}

fn is_paraish(block: &Block, in_list: bool) -> bool {
    match block {
        Block::Para(_) | Block::Header(..) | Block::BlockQuote(_) | Block::CodeBlock(..) => true,
        Block::BulletList(_) | Block::OrderedList(..) => !in_list,
        _ => false,
    }
}

/// A `<script type="math/tex">` carries TeX in its body; `mode=display` selects display math.
pub(super) fn math_script_type(e: &Element) -> Option<MathType> {
    let value = attr_value(e, "type")?;
    let value = value.to_ascii_lowercase();
    if !value.starts_with("math/tex") {
        return None;
    }
    if value.contains("mode=display") {
        Some(MathType::DisplayMath)
    } else {
        Some(MathType::InlineMath)
    }
}

pub(super) fn is_math_script(e: &Element) -> bool {
    math_script_type(e).is_some()
}

pub(super) fn is_checkbox(e: &Element) -> bool {
    e.name == "input"
        && attr_value(e, "type").is_some_and(|kind| kind.eq_ignore_ascii_case("checkbox"))
}

pub(super) fn contains_checkbox(e: &Element) -> bool {
    e.children.iter().any(|node| match node {
        Node::Element(child) => is_checkbox(child) || contains_checkbox(child),
        Node::Text(_) | Node::Comment(_) => false,
    })
}
