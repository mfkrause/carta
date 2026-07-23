//! Free helpers around the block phase's state machine: table-caption attachment, raw TeX
//! environment scanning, list-marker classification, fenced div and alert recognition, and
//! leaf-text cleanup.

use carta_ast::{Attr, ListNumberStyle};
use carta_core::{Extension, Extensions};

use super::block::ListInfo;
use super::cursor::ListMarkerParse;
use super::{ExampleMap, IrBlock, attr, texttable};

/// Tighten an HTML element's final content block from `Para` to `Plain` when no blank line
/// separated it from the close tag.
pub(super) fn tighten_last_block(blocks: &mut [IrBlock]) {
    if let Some(block) = blocks.last_mut()
        && let IrBlock::Para(text) = block
    {
        *block = IrBlock::Plain(std::mem::take(text));
    }
}

/// In a tight list, item paragraphs render as `Plain` rather than `Para`.
pub(crate) fn demote_loose_paragraphs(blocks: &mut [IrBlock]) {
    for block in blocks {
        if let IrBlock::Para(text) = block {
            *block = IrBlock::Plain(std::mem::take(text));
        }
    }
}

/// Attach table captions: a paragraph led by `Table:`, `table:`, or `:` becomes the caption of the
/// table — pipe, dash-ruled, or grid — immediately before it, or, failing that, immediately after
/// it. The caption attaches to the nearer uncaptioned table and is removed from the block list; with
/// no such table it stays an ordinary paragraph. Working in document order, a caption above a table
/// is reached first, so it wins over one below. The pass recurses into nested block containers first.
pub(super) fn attach_table_captions(blocks: &mut Vec<IrBlock>, ext: Extensions) {
    for block in blocks.iter_mut() {
        match block {
            IrBlock::Div(_, children) | IrBlock::BlockQuote(children) => {
                attach_table_captions(children, ext);
            }
            IrBlock::BulletList(items) | IrBlock::OrderedList(_, items) => {
                for item in items {
                    attach_table_captions(item, ext);
                }
            }
            IrBlock::DefinitionList(items) => {
                for item in items {
                    for definition in &mut item.definitions {
                        attach_table_captions(definition, ext);
                    }
                }
            }
            _ => {}
        }
    }
    if !ext.contains(Extension::TableCaptions) {
        return;
    }
    let mut i = 0;
    while i < blocks.len() {
        let Some(caption) = caption_text(blocks.get(i)) else {
            i += 1;
            continue;
        };
        let attached = (i >= 1 && set_table_caption(blocks, i - 1, &caption, ext))
            || (i + 1 < blocks.len() && set_table_caption(blocks, i + 1, &caption, ext));
        if attached {
            blocks.remove(i);
        } else {
            i += 1;
        }
    }
}

/// The caption text of a paragraph block led by a `Table:`/`table:`/`:` marker, with the marker
/// stripped; `None` for any other block.
fn caption_text(block: Option<&IrBlock>) -> Option<String> {
    let IrBlock::Para(text) = block? else {
        return None;
    };
    let (first, rest) = match text.split_once('\n') {
        Some((first, rest)) => (first, Some(rest)),
        None => (text.as_str(), None),
    };
    let body = strip_caption_marker(first)?;
    Some(match rest {
        Some(rest) => format!("{body}\n{rest}"),
        None => body.to_owned(),
    })
}

/// Strip a leading `Table:`, `table:`, or `:` caption marker and the spaces after it, returning the
/// remaining first-line text; `None` when no marker is present. Only the marker's first letter may
/// vary in case, so `TABLE:` is not a marker.
pub(super) fn strip_caption_marker(first: &str) -> Option<&str> {
    for marker in ["Table:", "table:"] {
        if let Some(rest) = first.strip_prefix(marker) {
            return Some(rest.trim_start());
        }
    }
    first.strip_prefix(':').map(str::trim_start)
}

/// Set `text` as the caption of the table at `index`, if that block is a pipe, dash-ruled, or grid
/// table that has no caption yet. Returns whether the caption was attached. With
/// [`Extension::TableAttributes`] enabled, a trailing `{…}` attribute block on the caption is split
/// off and applied to the table's outer attributes; the remaining text becomes the caption.
pub(super) fn set_table_caption(
    blocks: &mut [IrBlock],
    index: usize,
    text: &str,
    ext: Extensions,
) -> bool {
    let (caption_slot, attr_slot) = match blocks.get_mut(index) {
        Some(IrBlock::Table { caption, attr, .. }) => (caption, attr),
        Some(IrBlock::TextTable(table)) => (&mut table.caption, &mut table.attr),
        Some(IrBlock::GridTable(table)) => (&mut table.caption, &mut table.attr),
        _ => return false,
    };
    if caption_slot.is_some() {
        return false;
    }
    let (body, parsed) = if ext.contains(Extension::TableAttributes) {
        split_trailing_attr(text)
    } else {
        (text, None)
    };
    *caption_slot = Some(body.to_owned());
    if let Some(parsed) = parsed {
        *attr_slot = parsed;
    }
    true
}

/// Split a trailing `{…}` attribute block off the end of a caption. Returns the caption text with the
/// block and any whitespace before it removed, alongside the parsed attributes. When the text has no
/// well-formed trailing attribute block, the text is returned unchanged with `None`.
pub(super) fn split_trailing_attr(text: &str) -> (&str, Option<Attr>) {
    let trimmed = text.trim_end();
    if !trimmed.ends_with('}') {
        return (text, None);
    }
    // The trailing block opens at some `{`; find the one whose attribute parse consumes exactly to
    // the end. Earlier `{` characters stay part of the caption text.
    for (open, _) in trimmed.char_indices().filter(|&(_, ch)| ch == '{') {
        if let Some(rest) = trimmed.get(open..)
            && let Some((attr, consumed)) = attr::parse_attributes(rest)
            && consumed == rest.len()
        {
            let body = trimmed.get(..open).map_or("", str::trim_end);
            return (body, Some(attr));
        }
    }
    (text, None)
}

pub(super) enum Continue {
    Matched,
    MatchedLeaf,
    NotMatched,
}

/// Math environments are rendered inline rather than as block-level raw TeX, so a `\begin` opening
/// one is not a block environment. The base name is matched exactly; a single trailing `*` (the
/// unnumbered variant) counts as the same environment.
pub(super) fn is_math_environment(name: &str) -> bool {
    const MATH_ENVS: &[&str] = &[
        "equation",
        "align",
        "gather",
        "multline",
        "eqnarray",
        "flalign",
        "alignat",
        "displaymath",
        "math",
        "dmath",
    ];
    let base = name.strip_suffix('*').unwrap_or(name);
    MATH_ENVS.contains(&base)
}

/// If `s` begins with `\<keyword>` (optionally followed by spaces) then a braced `{name}`, return
/// the literal brace content. The leading backslash must not itself be escaped — callers pass the
/// raw slice from a line start, where this holds. The brace content runs to the first `}` and may be
/// empty; it is compared exactly elsewhere, so inner spaces are significant.
pub(super) fn raw_tex_env_name(s: &str, keyword: &[u8]) -> Option<String> {
    let after_backslash = s.strip_prefix('\\')?;
    let after_keyword = after_backslash.strip_prefix(std::str::from_utf8(keyword).ok()?)?;
    let after_spaces = after_keyword.trim_start_matches(' ');
    let body = after_spaces.strip_prefix('{')?;
    let close = body.find('}')?;
    body.get(..close).map(str::to_owned)
}

/// Scan one source `line` of an open environment named `name`, starting at nesting `depth`. Returns
/// the depth after the line and, when the environment's matching `\end{name}` is reached (depth back
/// to zero), the byte offset just past that `\end{...}`. Backslash escapes are honored: a `\\`
/// consumes both characters so an escaped command never counts toward the depth.
pub(super) fn raw_tex_scan(line: &str, name: &str, mut depth: usize) -> (usize, Option<usize>) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes.get(i) != Some(&b'\\') {
            i += 1;
            continue;
        }
        // An escaped backslash consumes both bytes and starts no command.
        if bytes.get(i + 1) == Some(&b'\\') {
            i += 2;
            continue;
        }
        let rest = line.get(i..).unwrap_or("");
        if raw_tex_env_name(rest, b"begin").as_deref() == Some(name) {
            depth += 1;
            i += 1;
            continue;
        }
        if raw_tex_env_name(rest, b"end").as_deref() == Some(name) {
            depth = depth.saturating_sub(1);
            // Advance past this `\end{...}` so the close offset lands just after its brace.
            let end_off = rest.find('}').map_or(line.len(), |brace| i + brace + 1);
            if depth == 0 {
                return (0, Some(end_off));
            }
            i = end_off;
            continue;
        }
        i += 1;
    }
    (depth, None)
}

/// The number for an example item, given its `@label` (or `None` for the anonymous `@`). A new or
/// anonymous item advances the shared counter; a repeated label reuses its first number.
pub(super) fn next_example_number(
    label: Option<String>,
    counter: &mut i32,
    map: &mut ExampleMap,
) -> i32 {
    if let Some(label) = &label
        && let Some(&number) = map.get(label)
    {
        return number;
    }
    *counter += 1;
    if let Some(label) = label {
        map.insert(label, *counter);
    }
    *counter
}

pub(super) fn list_info(parsed: &ListMarkerParse) -> ListInfo {
    ListInfo {
        bullet: parsed.bullet,
        marker: parsed.marker,
        style: parsed.style,
        delim: parsed.delim,
        start: parsed.start,
    }
}

/// Reread a lone roman `i`/`I` (the only roman enumerator whose start is one) as the ninth letter
/// of its alphabet. Any other list info is returned unchanged.
pub(super) fn demote_lone_roman(info: ListInfo) -> ListInfo {
    let style = match info.style {
        ListNumberStyle::LowerRoman if info.start == 1 => ListNumberStyle::LowerAlpha,
        ListNumberStyle::UpperRoman if info.start == 1 => ListNumberStyle::UpperAlpha,
        _ => return info,
    };
    ListInfo {
        style,
        start: 9,
        ..info
    }
}

/// Whether `marker` reads as a continuation of an ordered list whose established style is
/// `list_style` (the delimiter is checked separately). The list's first item fixes the style; each
/// later marker is reread in that style rather than its own:
///
/// - a decimal list takes only decimal markers;
/// - an alphabetic list takes any single letter of its case (so `h. i. j.` is one list, `i` read as
///   the ninth letter);
/// - a roman list takes any roman numeral of its case, plus the single letters whose position is a
///   roman value (`a`, `e`, `j`) — the same letters a roman sequence can reach.
pub(super) fn continues_ordered(list_style: ListNumberStyle, marker: &ListMarkerParse) -> bool {
    use ListNumberStyle::{Decimal, LowerAlpha, LowerRoman, UpperAlpha, UpperRoman};
    let lower = matches!(marker.style, LowerAlpha | LowerRoman);
    let upper = matches!(marker.style, UpperAlpha | UpperRoman);
    match list_style {
        Decimal => matches!(marker.style, Decimal),
        LowerAlpha => lower && marker.single_letter,
        UpperAlpha => upper && marker.single_letter,
        LowerRoman => lower && continues_roman(marker),
        UpperRoman => upper && continues_roman(marker),
        // An example list groups every example marker of the same delimiter, regardless of label.
        ListNumberStyle::Example => matches!(marker.style, ListNumberStyle::Example),
        ListNumberStyle::DefaultStyle => false,
    }
}

/// Whether `marker` reads as a roman numeral continuing a roman list: a multi-letter roman, the lone
/// roman `i`/`I`, or a single letter whose alphabet position is itself a roman digit or a roman
/// value (`a`=1, `e`=5, `j`=10).
fn continues_roman(marker: &ListMarkerParse) -> bool {
    if !marker.single_letter {
        return matches!(
            marker.style,
            ListNumberStyle::LowerRoman | ListNumberStyle::UpperRoman
        );
    }
    matches!(
        marker.style,
        ListNumberStyle::LowerRoman | ListNumberStyle::UpperRoman
    ) || matches!(marker.start, 1 | 3 | 4 | 5 | 9 | 10 | 12 | 13 | 22 | 24)
}

/// If a fence's info string is exactly an attribute block holding only a raw-format marker — a `{`,
/// optional whitespace, `=`, a format name, optional whitespace, then `}` — return that name. The
/// fence's contents are then raw output for that format. A format name is one run of letters,
/// digits, `-`, or `_`; anything else (extra attributes, a space inside the name, a stray symbol)
/// is not a raw marker and the fence stays an ordinary code block.
pub(super) fn raw_block_format(info: &str) -> Option<String> {
    let inner = info.trim().strip_prefix('{')?.strip_suffix('}')?;
    // The `=` immediately precedes the name: `{= html}` (a gap after `=`) is not a raw marker,
    // while surrounding whitespace (`{ =html }`) is allowed.
    let name = inner.trim_start().strip_prefix('=')?.trim_end();
    if name.is_empty() || !name.chars().all(is_format_name_char) {
        return None;
    }
    Some(name.to_owned())
}

pub(super) fn is_format_name_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_')
}

pub(super) fn fence_attr(info: &str, extensions: Extensions) -> Attr {
    let info = info.trim();
    if info.is_empty() {
        return Attr::default();
    }
    // With fenced-code attributes enabled, a `{…}` info string is a full attribute block; the whole
    // info must be the block, else it falls back to the bare-language reading.
    if (extensions.contains(Extension::FencedCodeAttributes)
        || extensions.contains(Extension::Attributes))
        && info.starts_with('{')
        && let Some((parsed, consumed)) = attr::parse_attributes(info)
        && info
            .get(consumed..)
            .is_some_and(|rest| rest.trim().is_empty())
    {
        return parsed;
    }
    let language = info.split_whitespace().next().unwrap_or("");
    Attr {
        id: carta_ast::Text::default(),
        classes: vec![language.into()],
        attributes: Vec::new(),
    }
}

/// If `line` (already past any container markers and leading indent) opens a fenced div — a run of
/// three or more colons followed by a valid attribute spec — return the colon count and the parsed
/// attributes. A bare colon run with no spec is not an opener (it can only close).
pub(super) fn div_open_fence(line: &str) -> Option<(usize, Attr)> {
    let after_colons = line.trim_start_matches(':');
    let count = line.len() - after_colons.len();
    if count < 3 {
        return None;
    }
    let attr = div_open_attr(after_colons.trim())?;
    Some((count, attr))
}

/// The recognized alert kinds: the marker spelling (recognized only in all-uppercase), the lowercased
/// class applied to the wrapping div, and the display title.
pub(super) struct AlertType {
    pub(super) class: &'static str,
    pub(super) title: &'static str,
}

const ALERT_TYPES: &[(&str, AlertType)] = &[
    (
        "note",
        AlertType {
            class: "note",
            title: "Note",
        },
    ),
    (
        "tip",
        AlertType {
            class: "tip",
            title: "Tip",
        },
    ),
    (
        "important",
        AlertType {
            class: "important",
            title: "Important",
        },
    ),
    (
        "warning",
        AlertType {
            class: "warning",
            title: "Warning",
        },
    ),
    (
        "caution",
        AlertType {
            class: "caution",
            title: "Caution",
        },
    ),
];

/// If `line` is exactly an alert marker `[!TYPE]` followed by only trailing whitespace — with no
/// leading whitespace and a recognized `TYPE` — return its kind. The broad Markdown dialect
/// (`uppercase_only`) admits only the all-uppercase spelling `[!NOTE]`; the `CommonMark` engine
/// accepts any casing (`[!note]`, `[!Note]`).
pub(super) fn alert_marker_type(line: &str, uppercase_only: bool) -> Option<&'static AlertType> {
    let inner = line.strip_prefix("[!")?;
    let close = inner.find(']')?;
    let name = inner.get(..close)?;
    // Only whitespace may follow the closing bracket.
    if !inner.get(close + 1..)?.chars().all(char::is_whitespace) {
        return None;
    }
    if uppercase_only && name.bytes().any(|b| b.is_ascii_lowercase()) {
        return None;
    }
    ALERT_TYPES
        .iter()
        .find(|(spelling, _)| name.eq_ignore_ascii_case(spelling))
        .map(|(_, ty)| ty)
}

/// Parse a fenced-div opener's attribute spec (the text after the colons, already trimmed). It is
/// either a single brace block of valid attributes or a single bare word taken verbatim as the sole
/// class; anything else (empty, multiple words, junk after a brace) is not a valid opener.
fn div_open_attr(spec: &str) -> Option<Attr> {
    if spec.is_empty() {
        return None;
    }
    if spec.starts_with('{')
        && let Some((attr, consumed)) = attr::parse_attributes_first_id(spec)
        && attr::is_non_empty(&attr)
        && spec
            .get(consumed..)
            .is_some_and(|rest| rest.trim().is_empty())
    {
        return Some(attr);
    }
    // Bare-word form: a single whitespace-free token becomes the sole class, kept verbatim (a
    // leading dot is not stripped).
    if spec.chars().any(char::is_whitespace) {
        return None;
    }
    Some(Attr {
        id: carta_ast::Text::default(),
        classes: vec![spec.into()],
        attributes: Vec::new(),
    })
}

/// If `line` (already past any container markers) is a closing div fence — up to three spaces of
/// indent, then a run of three or more colons, then only whitespace — return the colon count.
pub(super) fn div_close_fence(line: &str) -> Option<usize> {
    let after_spaces = line.trim_start_matches(' ');
    if line.len() - after_spaces.len() > 3 {
        return None;
    }
    let after_colons = after_spaces.trim_start_matches(':');
    let count = after_spaces.len() - after_colons.len();
    if count < 3 || !after_colons.trim().is_empty() {
        return None;
    }
    Some(count)
}

pub(super) fn strip_one_trailing_newline(text: &str) -> String {
    text.strip_suffix('\n').unwrap_or(text).to_owned()
}

/// A line opens or extends a line block when a `|` sits at its start, followed by a space or the
/// line's end.
pub(super) fn is_line_block_marker(line: &str) -> bool {
    line == "|" || line.starts_with("| ")
}

/// Whether `text` (a node's accumulated lines, each terminated by a newline) holds exactly one
/// non-empty line.
pub(super) fn single_line(text: &str) -> bool {
    let body = text.strip_suffix('\n').unwrap_or(text);
    !body.is_empty() && !body.contains('\n')
}

/// Split an accumulated table leaf's text into its physical lines, dropping the trailing empty piece
/// left by the final newline.
pub(super) fn split_table_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

pub(super) fn owned_lines(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| (*line).to_owned()).collect()
}

/// The last non-blank physical line of an accumulated leaf's text, scanning back from the end so the
/// cost is the length of that line rather than the whole accumulation.
pub(super) fn last_nonempty_line(text: &str) -> &str {
    text.trim_end_matches('\n')
        .rsplit('\n')
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
}

/// Whether a dash-only line is a thematic break: three or more dashes, with spaces allowed between
/// them. Used to settle a dash-ruled table candidate that turned out not to be a table.
pub(super) fn is_thematic_dash_line(line: &str) -> bool {
    texttable::is_dash_line(line) && line.bytes().filter(|byte| *byte == b'-').count() >= 3
}

/// Whether a line block's current (final) entry is empty: its last line is a `|` marker carrying no
/// content. A content-bearing line stays non-empty once written, so checking the final line alone is
/// enough — an empty entry is only ever followed by another marker line, never folded into.
pub(super) fn last_entry_is_empty(text: &str) -> bool {
    let last = text
        .trim_end_matches('\n')
        .rsplit('\n')
        .next()
        .unwrap_or("");
    last.strip_prefix('|')
        .is_some_and(|rest| rest.trim_matches([' ', '\t']).is_empty())
}

/// Split a line block's accumulated raw lines into prepared per-entry strings. A `|`-led line opens
/// a new entry — its `|` and one following space dropped, any remaining leading spaces kept as
/// non-breaking spaces so they survive inline parsing — while any other line continues the previous
/// entry, joined to it by a single space.
pub(super) fn line_block_lines(text: &str) -> Vec<String> {
    let mut entries: Vec<String> = Vec::new();
    for raw in text.lines() {
        if let Some(rest) = raw.strip_prefix('|') {
            let rest = rest.strip_prefix(' ').unwrap_or(rest);
            // Trailing whitespace is dropped first, so an all-space entry collapses to empty
            // rather than to a run of preserved leading spaces.
            entries.push(preserve_leading_spaces(rest.trim_end_matches([' ', '\t'])));
        } else if let Some(last) = entries.last_mut() {
            last.push(' ');
            last.push_str(raw.trim());
        } else {
            entries.push(raw.trim().to_owned());
        }
    }
    // A whitespace-only continuation folds nothing into its entry but leaves a dangling separator
    // space; drop any such trailing run, leaving preserved leading spaces untouched.
    for entry in &mut entries {
        let kept = entry.trim_end_matches([' ', '\t']).len();
        entry.truncate(kept);
    }
    entries
}

/// Replace a run of leading ASCII spaces with non-breaking spaces.
fn preserve_leading_spaces(s: &str) -> String {
    let trimmed = s.trim_start_matches(' ');
    let spaces = s.len() - trimmed.len();
    let mut out = String::with_capacity(s.len() + spaces);
    for _ in 0..spaces {
        out.push('\u{a0}');
    }
    out.push_str(trimmed);
    out
}

/// Drop trailing whitespace-only lines (and the final line ending), keeping interior blank lines.
pub(super) fn strip_trailing_blank_lines(text: &str) -> String {
    let mut lines: Vec<&str> = text.split('\n').collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// Trim an ATX heading's content: drop surrounding spaces/tabs and an optional closing run of `#`
/// (which must be preceded by whitespace or form the whole line, else it belongs to the content).
pub(super) fn strip_atx_closing(content: &str, require_preceding_space: bool) -> String {
    let trimmed = content.trim_matches([' ', '\t']);
    let without_hashes = trimmed.trim_end_matches('#');
    if without_hashes.len() == trimmed.len() {
        return trimmed.to_owned();
    }
    // A closing hash run always terminates the heading when the dialect does not require a space
    // after the opener; otherwise the run must be set off from the content by whitespace.
    if !require_preceding_space
        || without_hashes.is_empty()
        || without_hashes.ends_with([' ', '\t'])
    {
        without_hashes.trim_end_matches([' ', '\t']).to_owned()
    } else {
        trimmed.to_owned()
    }
}
