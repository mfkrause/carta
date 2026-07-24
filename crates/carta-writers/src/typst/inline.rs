//! Inline rendering and line filling for the Typst writer.

use std::fmt::Write as _;

use carta_ast::{Attr, Block, Inline, Target, to_plain_text};
use carta_core::WrapMode;

use crate::common::display_width;

use super::escape::{
    escape_string, escape_text, image_call, inline_code, math, quote_marks, raw_inline_passthrough,
};
use super::{Fragment, blocks, label};

/// Render inline content laid out per the document's wrap mode (paragraph context).
pub(super) fn fill_inlines(items: &[Inline], width: usize, wrap: WrapMode, smart: bool) -> String {
    fill(&fragments(items, width, wrap, smart), width, wrap)
}

/// Render inline content without wrapping, single-spacing the breakable units (nested markup
/// context, where the surrounding construct controls layout). A nested footnote's body still
/// reflows per the document `wrap`.
pub(super) fn inline_run(items: &[Inline], width: usize, wrap: WrapMode, smart: bool) -> String {
    let mut out = String::new();
    for fragment in fragments(items, width, wrap, smart) {
        match fragment {
            Fragment::Text(text) | Fragment::Atom(text) => out.push_str(&text),
            Fragment::Space | Fragment::Soft => out.push(' '),
            Fragment::LineBreak => out.push_str(" \\ "),
        }
    }
    escape_open_marker(&out)
}

/// Build the fragment stream for an inline sequence. A leading `(` on the very first text fragment is
/// escaped here (the only character whose escape depends on opening the whole sequence rather than a
/// physical line).
pub(super) fn fragments(
    items: &[Inline],
    width: usize,
    wrap: WrapMode,
    smart: bool,
) -> Vec<Fragment> {
    let mut out = Vec::new();
    let mut after_space = true;
    let mut first_inline = true;
    for item in items {
        let next_after_space = matches!(item, Inline::Space | Inline::SoftBreak);
        match item {
            Inline::Str(text) => {
                out.push(Fragment::Text(escape_text(
                    text,
                    after_space,
                    first_inline,
                    smart,
                )));
                first_inline = false;
            }
            Inline::Space => out.push(Fragment::Space),
            Inline::SoftBreak => out.push(Fragment::Soft),
            Inline::LineBreak => out.push(Fragment::LineBreak),
            Inline::Emph(inner) => {
                extend_wrapped(&mut out, inner, "#emph[", "]", width, wrap, smart);
                first_inline = false;
            }
            Inline::Strong(inner) => {
                extend_wrapped(&mut out, inner, "#strong[", "]", width, wrap, smart);
                first_inline = false;
            }
            Inline::Strikeout(inner) => {
                extend_wrapped(&mut out, inner, "#strike[", "]", width, wrap, smart);
                first_inline = false;
            }
            Inline::Underline(inner) => {
                extend_wrapped(&mut out, inner, "#underline[", "]", width, wrap, smart);
                first_inline = false;
            }
            Inline::SmallCaps(inner) => {
                extend_wrapped(&mut out, inner, "#smallcaps[", "]", width, wrap, smart);
                first_inline = false;
            }
            Inline::Quoted(kind, inner) => {
                let (open, close) = quote_marks(kind, smart);
                extend_wrapped(
                    &mut out,
                    inner,
                    &open.to_string(),
                    &close.to_string(),
                    width,
                    wrap,
                    smart,
                );
                first_inline = false;
            }
            Inline::Span(attr, inner) if attr.id.is_empty() => {
                let (open, close) = span_wrapper(attr);
                extend_wrapped(&mut out, inner, open, close, width, wrap, smart);
                first_inline = false;
            }
            Inline::Span(attr, inner) => {
                out.push(Fragment::Atom(span(
                    attr,
                    inner,
                    first_inline,
                    width,
                    wrap,
                    smart,
                )));
                first_inline = false;
            }
            other => {
                let rendered = inline(other, width, wrap, smart);
                if !rendered.is_empty() {
                    out.push(Fragment::Atom(rendered));
                    first_inline = false;
                }
            }
        }
        after_space = next_after_space;
    }
    out
}

/// Splice an inline sequence into `out` wrapped in `open`/`close` delimiters, keeping its internal
/// spaces as wrap points: the opening fuses to the first word and the closing to the last, so a long
/// run can break across physical lines with the delimiters staying attached to their boundary words.
/// Both boundary words remain escapable so a leading line-marker character is still guarded when a
/// word opens a physical line.
fn extend_wrapped(
    out: &mut Vec<Fragment>,
    items: &[Inline],
    open: &str,
    close: &str,
    width: usize,
    wrap: WrapMode,
    smart: bool,
) {
    let mut inner = fragments(items, width, wrap, smart);
    let is_textual =
        |fragment: &Fragment| matches!(fragment, Fragment::Text(_) | Fragment::Atom(_));
    match inner.iter().position(is_textual) {
        None => out.push(Fragment::Atom(format!("{open}{close}"))),
        Some(first) => {
            let last = inner.iter().rposition(is_textual).unwrap_or(first);
            if let Some(fragment) = inner.get_mut(first) {
                prepend_fragment(fragment, open);
            }
            if let Some(fragment) = inner.get_mut(last) {
                append_fragment(fragment, close);
            }
            out.append(&mut inner);
        }
    }
}

/// Prepend `prefix` to a textual fragment's text, leaving its variant (and thus its escapability) intact.
fn prepend_fragment(fragment: &mut Fragment, prefix: &str) {
    if let Fragment::Text(text) | Fragment::Atom(text) = fragment {
        *text = format!("{prefix}{text}");
    }
}

/// Append `suffix` to a textual fragment's text, leaving its variant (and thus its escapability) intact.
fn append_fragment(fragment: &mut Fragment, suffix: &str) {
    if let Fragment::Text(text) | Fragment::Atom(text) = fragment {
        text.push_str(suffix);
    }
}

/// The delimiters wrapping a span's content: a semantic class selects a Typst function, otherwise the
/// content renders bare. Spans carrying an id label are handled separately and never reach here.
fn span_wrapper(attr: &Attr) -> (&'static str, &'static str) {
    if attr.classes.iter().any(|class| class == "mark") {
        ("#highlight[", "]")
    } else if attr.classes.iter().any(|class| class == "underline") {
        ("#underline[", "]")
    } else if attr.classes.iter().any(|class| class == "smallcaps") {
        ("#smallcaps[", "]")
    } else {
        ("", "")
    }
}

/// Greedily fill fragments to the fill column at a physical line start (paragraph context), laid out
/// per the document's wrap mode.
fn fill(fragments: &[Fragment], width: usize, wrap: WrapMode) -> String {
    // Only Auto reflows to a width; the other modes split solely on source soft breaks.
    let width = if matches!(wrap, WrapMode::Auto) {
        width.max(1)
    } else {
        usize::MAX
    };
    fill_with(
        fragments,
        width,
        0,
        0,
        matches!(wrap, WrapMode::Preserve),
        0,
    )
}

/// Fill fragments for a table cell whose first line begins at `first` columns in (after the opening
/// bracket) and whose wrapped continuation lines sit at `indent` columns. Continuation lines are
/// emitted at column zero; the caller applies the indent. The cell content is `#table` source rather
/// than a bordered field, so it follows the document wrap mode: only `Auto` reflows to the fill
/// column, while `None` and `Preserve` keep it on physical lines split solely on source soft breaks.
pub(super) fn fill_cell(
    fragments: &[Fragment],
    first: usize,
    indent: usize,
    width: usize,
    wrap: WrapMode,
    glue: usize,
) -> String {
    let width = if matches!(wrap, WrapMode::Auto) {
        width
    } else {
        usize::MAX
    };
    fill_with(
        fragments,
        width,
        first,
        indent,
        matches!(wrap, WrapMode::Preserve),
        glue,
    )
}

/// Lay fragments out into lines no wider than `width` (already resolved to a sentinel when no width
/// wrap is wanted). The first line is laid out as if `first` columns are already consumed; each
/// continuation line reserves `indent` columns. A line-opening `- + = /` is escaped only when it
/// begins a true physical line (`first == 0`). A source soft break forces a fresh physical line when
/// `preserve_softs` is set, and is otherwise inter-word space. `glue` is the width of the non-breaking
/// text that follows the last word on its physical line, so that word's wrap decision keeps it with
/// the trailing run rather than overflowing the line.
fn fill_with(
    fragments: &[Fragment],
    width: usize,
    first: usize,
    indent: usize,
    preserve_softs: bool,
    glue: usize,
) -> String {
    let last_word = fragments
        .iter()
        .rposition(|fragment| matches!(fragment, Fragment::Text(_) | Fragment::Atom(_)));
    let mut out = String::new();
    let mut column = first;
    let mut at_line_start = first == 0;
    let mut physical_line_start = first == 0;
    let mut pending_space = false;
    for (position, fragment) in fragments.iter().enumerate() {
        match fragment {
            Fragment::Soft if preserve_softs => {
                out.push('\n');
                column = indent;
                pending_space = false;
                at_line_start = true;
                physical_line_start = true;
            }
            Fragment::Space | Fragment::Soft => pending_space = true,
            Fragment::LineBreak => {
                out.push_str(" \\ ");
                column += 3;
                pending_space = false;
                at_line_start = false;
                physical_line_start = false;
            }
            Fragment::Text(text) | Fragment::Atom(text) => {
                let escapable = matches!(fragment, Fragment::Text(_));
                let word_width = display_width(text);
                let trailing = if Some(position) == last_word { glue } else { 0 };
                if at_line_start {
                    push_word(&mut out, text, escapable, physical_line_start);
                    column += word_width;
                    at_line_start = false;
                } else if pending_space && column + 1 + word_width + trailing > width {
                    out.push('\n');
                    push_word(&mut out, text, escapable, true);
                    column = indent + word_width;
                    physical_line_start = true;
                } else {
                    if pending_space {
                        out.push(' ');
                        column += 1;
                    }
                    push_word(&mut out, text, escapable, false);
                    column += word_width;
                }
                pending_space = false;
            }
        }
    }
    out
}

/// Append a word; when it opens a physical line, escape a leading `- + = /` that would otherwise be
/// read as a list or line marker.
fn push_word(out: &mut String, word: &str, escapable: bool, line_start: bool) {
    if line_start && escapable {
        out.push_str(&escape_open_marker(word));
    } else {
        out.push_str(word);
    }
}

/// Escape a leading line-marker character (`- + = /`) so it does not open a Typst list or rule.
fn escape_open_marker(word: &str) -> String {
    match word.chars().next() {
        Some('-' | '+' | '=' | '/') => format!("\\{word}"),
        _ => word.to_owned(),
    }
}

fn inline(value: &Inline, width: usize, wrap: WrapMode, smart: bool) -> String {
    match value {
        Inline::Str(text) => escape_text(text, true, false, smart),
        Inline::Emph(items) => format!("#emph[{}]", inline_run(items, width, wrap, smart)),
        Inline::Strong(items) => format!("#strong[{}]", inline_run(items, width, wrap, smart)),
        Inline::Underline(items) => {
            format!("#underline[{}]", inline_run(items, width, wrap, smart))
        }
        Inline::Strikeout(items) => format!("#strike[{}]", inline_run(items, width, wrap, smart)),
        Inline::Superscript(items) => format!("#super[{}]", inline_run(items, width, wrap, smart)),
        Inline::Subscript(items) => format!("#sub[{}]", inline_run(items, width, wrap, smart)),
        Inline::SmallCaps(items) => {
            format!("#smallcaps[{}]", inline_run(items, width, wrap, smart))
        }
        Inline::Quoted(kind, items) => {
            let (open, close) = quote_marks(kind, smart);
            format!("{open}{}{close}", inline_run(items, width, wrap, smart))
        }
        Inline::Cite(citations, _) => cite(citations),
        Inline::Code(_, text) => inline_code(text),
        Inline::Space | Inline::SoftBreak => " ".to_owned(),
        Inline::LineBreak => " \\ ".to_owned(),
        Inline::Math(kind, text) => math(kind, text, smart),
        Inline::RawInline(format, text) => raw_inline_passthrough(format, text),
        Inline::Link(_, items, target) => link(items, target, width, wrap, smart),
        Inline::Image(attr, alt, target) => format!("#box({})", image_call(attr, alt, target)),
        Inline::Note(blocks) => format!("#footnote[{}]", self_blocks(blocks, width, wrap, smart)),
        Inline::Span(attr, items) => span(attr, items, false, width, wrap, smart),
    }
}

fn self_blocks(items: &[Block], width: usize, wrap: WrapMode, smart: bool) -> String {
    blocks(items, width, wrap, smart)
        .trim_end_matches('\n')
        .to_owned()
}

fn cite(citations: &[carta_ast::Citation]) -> String {
    let mut out = String::new();
    for citation in citations {
        let _ = write!(out, "@{}", citation.id);
    }
    out
}

fn link(items: &[Inline], target: &Target, width: usize, wrap: WrapMode, smart: bool) -> String {
    let plain = to_plain_text(items);
    let url = escape_string(&target.url);
    if plain == target.url {
        format!("#link(\"{url}\")")
    } else {
        // Unbounded width: the link is one unit for wrapping, yet `Preserve` keeps inner breaks.
        let label = fill(&fragments(items, width, wrap, smart), usize::MAX, wrap);
        format!("#link(\"{url}\")[{label}]")
    }
}

/// Render a span: its semantic classes select a wrapper, then a trailing id label. A label opening
/// the inline sequence is anchored with a leading zero-width space so it does not attach to the
/// preceding markup.
fn span(
    attr: &Attr,
    items: &[Inline],
    at_start: bool,
    width: usize,
    wrap: WrapMode,
    smart: bool,
) -> String {
    let content = if attr.classes.iter().any(|class| class == "mark") {
        format!("#highlight[{}]", inline_run(items, width, wrap, smart))
    } else if attr.classes.iter().any(|class| class == "underline") {
        format!("#underline[{}]", inline_run(items, width, wrap, smart))
    } else if attr.classes.iter().any(|class| class == "smallcaps") {
        format!("#smallcaps[{}]", inline_run(items, width, wrap, smart))
    } else {
        inline_run(items, width, wrap, smart)
    };
    match label(&attr.id) {
        Some(rendered) if at_start => format!("\u{200b}{content}{rendered}"),
        Some(rendered) => format!("{content}{rendered}"),
        None => content,
    }
}
