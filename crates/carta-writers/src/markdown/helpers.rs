//! Free helpers for block, list, and code-fence rendering in the markdown engine.

use carta_ast::{Attr, Block, Format, Inline};

use crate::markdown_common::attr_is_empty;

use super::attr_braces;

/// Whether a raw-format name denotes TeX, which Markdown dialects with `raw_tex` embed verbatim.
/// `ConTeXt` and other TeX-adjacent formats are excluded: only `tex`/`latex` take the verbatim
/// path; everything else is rendered via the `raw_attribute` fenced form.
pub(super) fn is_tex_format(format: &Format) -> bool {
    matches!(format.0.as_str(), "tex" | "latex")
}

pub(super) fn collapse_trailing_newline(text: &str) -> String {
    text.strip_suffix('\n').unwrap_or(text).to_owned()
}

/// The inline content of a list item that is exactly one paragraph, so its content can be filled
/// directly under the item marker without first rendering the item's block sequence.
pub(super) fn single_paragraph(item: &[Block]) -> Option<&[Inline]> {
    match item {
        [Block::Plain(inlines) | Block::Para(inlines)] => Some(inlines),
        _ => None,
    }
}

/// The escaped form of a word that opens a paragraph and would otherwise start a list, or `None`
/// when the word is no list marker. A bullet (`-`/`+`) is escaped whatever follows it. An ordered
/// marker is escaped only when `break_follows` reports a space or the paragraph end after it: a
/// number (decimal, a single letter, a roman numeral, `#`, or an `@` example label) closed by `.`
/// or `)` or enclosed in parentheses, at most ten characters long. Only a later line of the same
/// paragraph is safe from being read as a list, so only the first word is at risk.
pub(super) fn escaped_leading_marker(text: &str, break_follows: bool) -> Option<String> {
    if text == "-" || text == "+" {
        return Some(format!("\\{text}"));
    }
    if !break_follows || text.chars().count() > 10 {
        return None;
    }
    let (enclosed, body) = match text.strip_prefix('(') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let delimiter = body.chars().last()?;
    let number = body.strip_suffix(delimiter)?;
    let closes = match delimiter {
        ')' => true,
        // A capital before a period reads as an initial ("B. Russell"), not as a marker.
        '.' => !enclosed && !matches!(number.as_bytes(), [letter] if letter.is_ascii_uppercase()),
        _ => false,
    };
    if !closes || !is_list_number(number) {
        return None;
    }
    Some(if enclosed {
        format!("\\({number}\\)")
    } else {
        format!("{number}\\{delimiter}")
    })
}

/// Whether `number` is the numbering part of an ordered-list marker.
fn is_list_number(number: &str) -> bool {
    match number.as_bytes() {
        [] => false,
        [b'#'] => true,
        [b'@', label @ ..] => label
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')),
        [letter] if letter.is_ascii_alphabetic() => true,
        digits => digits.iter().all(u8::is_ascii_digit) || is_roman_numeral(number),
    }
}

/// Whether `text` is a roman numeral in a single letter case: its value groups in descending
/// order, each subtractive pair and each five-valued letter at most once.
fn is_roman_numeral(text: &str) -> bool {
    const GROUPS: [(&str, bool); 13] = [
        ("m", true),
        ("cm", false),
        ("d", false),
        ("cd", false),
        ("c", true),
        ("xc", false),
        ("l", false),
        ("xl", false),
        ("x", true),
        ("ix", false),
        ("v", false),
        ("iv", false),
        ("i", true),
    ];
    let lower = text.to_ascii_lowercase();
    if text != lower && text != text.to_ascii_uppercase() {
        return false;
    }
    let mut rest = lower.as_str();
    for (group, repeats) in GROUPS {
        while let Some(tail) = rest.strip_prefix(group) {
            rest = tail;
            if !repeats {
                break;
            }
        }
    }
    !text.is_empty() && rest.is_empty()
}

/// The info string for a fenced code block when fenced-code attributes are available, or `None` to
/// render it indented (no attributes). A lone class becomes a bare language tag; anything richer
/// uses the attribute block form.
pub(super) fn extended_code_info(attr: &Attr) -> Option<String> {
    if attr_is_empty(attr) {
        return None;
    }
    if attr.id.is_empty()
        && attr.attributes.is_empty()
        && let [class] = attr.classes.as_slice()
    {
        return Some(format!(" {class}"));
    }
    Some(format!(" {}", attr_braces(attr)))
}

/// The info string for a fenced code block when fenced-code attributes are unavailable, or `None`
/// for indented output: only the first class survives, as a bare language tag.
pub(super) fn github_code_info(attr: &Attr) -> Option<String> {
    match attr.classes.first() {
        Some(class) if !class.is_empty() => Some(format!(" {class}")),
        _ if attr_is_empty(attr) => None,
        _ => Some(String::new()),
    }
}

/// The fence length for a fenced code block built from `fence`: longer than the longest leading run
/// of that character already in the body (so the fence cannot close early), and at least three.
pub(super) fn fence_run_len(text: &str, fence: char) -> usize {
    let mut longest = 0;
    for line in text.split('\n') {
        let run = line.chars().take_while(|&c| c == fence).count();
        longest = longest.max(run);
    }
    (longest + 1).max(3)
}

/// The colon-fence length for a fenced div: longer than the longest leading colon run already in the
/// body (so a nested div's fence is strictly shorter), and at least three.
pub(super) fn colon_fence_len(body: &str) -> usize {
    let mut longest = 0;
    for line in body.split('\n') {
        let run = line.chars().take_while(|&c| c == ':').count();
        longest = longest.max(run);
    }
    (longest + 1).max(3)
}

/// The number of divs a block sequence nests through divs alone: its own divs and, in turn, the
/// divs those nest the same way. A fenced div's fence grows by one colon for each.
pub(super) fn nested_div_count(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .map(|block| match block {
            Block::Div(_, children) => 1 + nested_div_count(children),
            _ => 0,
        })
        .sum()
}

/// The text following a fenced-div opener: a bare class when the div carries only a single class and
/// the shorthand is allowed (`braced` is false), otherwise an attribute block.
pub(super) fn div_opener(attr: &Attr, braced: bool) -> String {
    if !braced
        && attr.id.is_empty()
        && attr.attributes.is_empty()
        && let [class] = attr.classes.as_slice()
    {
        return format!(" {class}");
    }
    format!(" {}", attr_braces(attr))
}

/// Flatten a figure caption's blocks into one inline sequence for the implicit-figure form: each
/// paragraph contributes its inlines and successive paragraphs are joined by a line break. An empty
/// caption yields an empty sequence. Returns `None` if any block is not a paragraph, leaving a
/// richly structured caption to fall back to an HTML figure.
pub(super) fn caption_blocks_as_inlines(blocks: &[Block]) -> Option<Vec<Inline>> {
    let mut inlines = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        let (Block::Plain(paragraph) | Block::Para(paragraph)) = block else {
            return None;
        };
        if index > 0 {
            inlines.push(Inline::LineBreak);
        }
        inlines.extend(paragraph.iter().cloned());
    }
    Some(inlines)
}

/// Whether a header's attributes are exactly the identifier a reader would derive from its text, so
/// the explicit `{#id}` block is redundant and can be dropped.
pub(super) fn header_attr_implicit(
    attr: &Attr,
    inlines: &[Inline],
    auto_identifiers: bool,
) -> bool {
    attr.classes.is_empty()
        && attr.attributes.is_empty()
        && (attr.id.is_empty()
            || (auto_identifiers && attr.id == carta_ast::slug(&carta_ast::to_plain_text(inlines))))
}

/// Whether a list item's first block opens with a checkbox glyph, and if so whether it is checked.
/// `None` when the item is not a checkbox item.
pub(super) fn checkbox_state(item: &[Block]) -> Option<bool> {
    let (Block::Plain(inlines) | Block::Para(inlines)) = item.first()? else {
        return None;
    };
    match inlines.first()? {
        Inline::Str(text) if text == "\u{2610}" => Some(false),
        Inline::Str(text) if text == "\u{2612}" => Some(true),
        _ => None,
    }
}

/// Remove the leading checkbox glyph and the space after it from a list item's first block.
pub(super) fn strip_checkbox(item: &[Block]) -> Vec<Block> {
    let mut blocks = item.to_vec();
    if let Some(Block::Plain(inlines) | Block::Para(inlines)) = blocks.first_mut()
        && matches!(inlines.first(), Some(Inline::Str(text)) if text == "\u{2610}" || text == "\u{2612}")
    {
        inlines.remove(0);
        if matches!(inlines.first(), Some(Inline::Space)) {
            inlines.remove(0);
        }
    }
    blocks
}
