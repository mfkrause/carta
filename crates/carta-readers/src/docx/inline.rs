//! Inline assembly for the docx reader, nesting run formatting into wrapper inlines.

use std::collections::VecDeque;

use carta_ast::{Inline, Target, Text};

use crate::xml::{Element, local_name};

use super::RunFmt;
use super::helpers::{custom_style_attr, field_link_target, mark_attr};

/// One enclosing inline wrapper. The declaration order is the nesting order applied to a leaf:
/// earlier variants wrap later ones regardless of the order the source turned them on.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Wrapper {
    Custom(Text),
    Emph,
    Strong,
    Mark,
    SmallCaps,
    Strikeout,
    Superscript,
    Subscript,
    Underline,
}

impl Wrapper {
    fn wrap(self, children: Vec<Inline>) -> Inline {
        match self {
            Wrapper::Custom(name) => Inline::Span(Box::new(custom_style_attr(&name)), children),
            Wrapper::Emph => Inline::Emph(children),
            Wrapper::Strong => Inline::Strong(children),
            Wrapper::Mark => Inline::Span(Box::new(mark_attr()), children),
            Wrapper::SmallCaps => Inline::SmallCaps(children),
            Wrapper::Strikeout => Inline::Strikeout(children),
            Wrapper::Superscript => Inline::Superscript(children),
            Wrapper::Subscript => Inline::Subscript(children),
            Wrapper::Underline => Inline::Underline(children),
        }
    }
}

/// Extracts a source-code paragraph's text verbatim from its runs, preserving all whitespace and
/// rendering each line break, carriage return, or tab as the character it stands for so the code
/// block's exact layout survives.
pub(super) fn code_paragraph_text(paragraph: &Element) -> String {
    let mut out = String::new();
    push_code_runs(paragraph, &mut out);
    out
}

fn push_code_runs(element: &Element, out: &mut String) {
    for child in element.elements() {
        match local_name(&child.name) {
            "t" => out.push_str(&child.text()),
            "tab" => out.push('\t'),
            "br" => {
                // Only a text-wrapping break advances the code line; page and column breaks do not.
                if child.attr("type").unwrap_or("textWrapping") == "textWrapping" {
                    out.push('\n');
                }
            }
            "cr" => out.push('\n'),
            "noBreakHyphen" => out.push('\u{2011}'),
            "sym" => {
                if let Some(ch) = child
                    .attr("char")
                    .and_then(|code| u32::from_str_radix(code, 16).ok())
                    .and_then(char::from_u32)
                {
                    out.push(ch);
                }
            }
            // Properties, tracked deletions, and field instructions are not part of the code.
            "rPr" | "pPr" | "del" | "delText" | "instrText" => {}
            _ => push_code_runs(child, out),
        }
    }
}

/// The wrapper path implied by a run's formatting, outermost first.
fn wrappers(fmt: &RunFmt) -> Vec<Wrapper> {
    let mut path = Vec::new();
    // `custom` is stored innermost-first; reversed here so the outermost style opens first.
    for name in fmt.custom.iter().rev() {
        path.push(Wrapper::Custom(name.clone()));
    }
    if fmt.italic {
        path.push(Wrapper::Emph);
    }
    if fmt.bold {
        path.push(Wrapper::Strong);
    }
    if fmt.mark {
        path.push(Wrapper::Mark);
    }
    if fmt.smallcaps {
        path.push(Wrapper::SmallCaps);
    }
    if fmt.strike {
        path.push(Wrapper::Strikeout);
    }
    if fmt.superscript {
        path.push(Wrapper::Superscript);
    }
    if fmt.subscript {
        path.push(Wrapper::Subscript);
    }
    if fmt.underline {
        path.push(Wrapper::Underline);
    }
    path
}

/// A leaf produced within a paragraph, tagged with the formatting active when it was emitted.
struct Leaf {
    fmt: RunFmt,
    inline: Inline,
}

/// One open complex field, tracked while its runs stream by. Its `instr` collects the field code
/// (before the `separate`); `result_start` marks where its displayed result begins in `leaves`.
struct FieldFrame {
    instr: String,
    result_start: Option<usize>,
}

/// Assembles a paragraph's inline content, collapsing whitespace and trimming its edges as prose,
/// then nesting formatting wrappers by shared prefix.
#[derive(Default)]
pub(super) struct InlineBuilder {
    leaves: Vec<Leaf>,
    text: String,
    text_fmt: RunFmt,
    pending_space: Option<RunFmt>,
    has_content: bool,
    fields: Vec<FieldFrame>,
}

impl InlineBuilder {
    /// Whether the builder is inside a complex field's code region, where run content is the field
    /// instruction rather than displayed text and so emits nothing.
    fn in_field_code(&self) -> bool {
        self.fields
            .last()
            .is_some_and(|frame| frame.result_start.is_none())
    }

    /// Opens a complex field. Content up to its `separate` is field code and is suppressed.
    pub(super) fn field_begin(&mut self) {
        self.fields.push(FieldFrame {
            instr: String::new(),
            result_start: None,
        });
    }

    /// Appends a chunk of the current field's instruction text.
    pub(super) fn field_instr(&mut self, text: &str) {
        if let Some(frame) = self.fields.last_mut() {
            frame.instr.push_str(text);
        }
    }

    /// Marks the boundary between a field's code and its displayed result.
    pub(super) fn field_separate(&mut self) {
        self.flush_text();
        self.resolve_space();
        let start = self.leaves.len();
        if let Some(frame) = self.fields.last_mut() {
            frame.result_start = Some(start);
        }
    }

    /// Closes a complex field. A hyperlink or reference field wraps its result in a link; any other
    /// field leaves its result in place as ordinary inlines.
    pub(super) fn field_end(&mut self) {
        self.flush_text();
        let Some(frame) = self.fields.pop() else {
            return;
        };
        let Some(target) = field_link_target(&frame.instr) else {
            return;
        };
        let start = frame.result_start.unwrap_or(self.leaves.len());
        let start = start.min(self.leaves.len());
        let result = self.leaves.split_off(start);
        let content = build_nested(result);
        if content.is_empty() {
            return;
        }
        self.leaves.push(Leaf {
            fmt: RunFmt::default(),
            inline: Inline::Link(
                Box::default(),
                content,
                Box::new(Target {
                    url: target.into(),
                    title: Text::default(),
                }),
            ),
        });
        self.has_content = true;
    }

    pub(super) fn push_text(&mut self, fmt: &RunFmt, text: &str) {
        if self.in_field_code() {
            return;
        }
        for ch in text.chars() {
            if is_break_space(ch) {
                self.flush_text();
                self.pending_space = Some(fmt.clone());
            } else {
                self.resolve_space();
                if !self.text.is_empty() && &self.text_fmt != fmt {
                    self.flush_text();
                }
                if self.text.is_empty() {
                    self.text_fmt = fmt.clone();
                }
                self.text.push(ch);
                self.has_content = true;
            }
        }
    }

    pub(super) fn push_space(&mut self, fmt: &RunFmt) {
        if self.in_field_code() {
            return;
        }
        self.flush_text();
        self.pending_space = Some(fmt.clone());
    }

    pub(super) fn push_break(&mut self, fmt: &RunFmt) {
        if self.in_field_code() {
            return;
        }
        self.flush_text();
        self.pending_space = None;
        self.leaves.push(Leaf {
            fmt: fmt.clone(),
            inline: Inline::LineBreak,
        });
        self.has_content = true;
    }

    pub(super) fn push_node(&mut self, fmt: RunFmt, inline: Inline) {
        if self.in_field_code() {
            return;
        }
        self.flush_text();
        self.resolve_space();
        self.leaves.push(Leaf { fmt, inline });
        self.has_content = true;
    }

    fn flush_text(&mut self) {
        if !self.text.is_empty() {
            let text = std::mem::take(&mut self.text);
            self.leaves.push(Leaf {
                fmt: self.text_fmt.clone(),
                inline: Inline::Str(text.into()),
            });
        }
    }

    fn resolve_space(&mut self) {
        if let Some(fmt) = self.pending_space.take()
            && self.has_content
        {
            self.leaves.push(Leaf {
                fmt,
                inline: Inline::Space,
            });
        }
    }

    pub(super) fn finish(mut self) -> Vec<Inline> {
        self.flush_text();
        // A trailing space is dropped: paragraph edges carry no whitespace.
        self.pending_space = None;
        build_nested(self.leaves)
    }
}

/// Nests a flat, formatting-tagged leaf sequence into wrapper inlines. Adjacent leaves are grouped
/// under whichever of a leaf's formats spans the longest unbroken run, factored outermost; each
/// leaf then keeps its remaining formats inside. So a bold-italic run beside a bold run share one
/// outer emphasis-strong split rather than each carrying its own copy.
fn build_nested(leaves: Vec<Leaf>) -> Vec<Inline> {
    let items = leaves
        .into_iter()
        .map(|leaf| (wrappers(&leaf.fmt), leaf.inline))
        .collect();
    build_grouped(items)
}

/// The number of consecutive leading items whose formatting includes `wrapper`.
fn run_length(items: &VecDeque<(Vec<Wrapper>, Inline)>, wrapper: &Wrapper) -> usize {
    let mut len = 0;
    while items
        .get(len)
        .is_some_and(|(path, _)| path.contains(wrapper))
    {
        len += 1;
    }
    len
}

fn build_grouped(mut items: VecDeque<(Vec<Wrapper>, Inline)>) -> Vec<Inline> {
    let mut out = Vec::new();
    loop {
        // Decide how to open the front leaf without holding a borrow across the mutation below.
        let choice = match items.front() {
            None => break,
            Some((path, _)) => match path.first() {
                None => None,
                Some(first) => {
                    let mut best = first.clone();
                    let mut best_len = run_length(&items, &best);
                    for wrapper in path.iter().skip(1) {
                        let len = run_length(&items, wrapper);
                        if len > best_len {
                            best_len = len;
                            best = wrapper.clone();
                        }
                    }
                    Some((best, best_len))
                }
            },
        };
        match choice {
            // An unformatted leaf contributes its inline directly.
            None => {
                if let Some((_, inline)) = items.pop_front() {
                    out.push(inline);
                }
            }
            // Peel the run this wrapper spans: drop it from each member, nest the rest inside.
            Some((wrapper, len)) => {
                let mut group = VecDeque::new();
                for _ in 0..len {
                    if let Some((mut path, inline)) = items.pop_front() {
                        path.retain(|candidate| *candidate != wrapper);
                        group.push_back((path, inline));
                    }
                }
                out.extend(wrap_factored(wrapper, build_grouped(group)));
            }
        }
    }
    out
}

/// Whether an inline carries no formatting of its own and so may sit outside an enclosing wrapper
/// rather than inside it. Only an inter-word space qualifies; a line break belongs to the span it
/// falls in and stays inside.
fn is_neutral(inline: &Inline) -> bool {
    matches!(inline, Inline::Space)
}

/// Wraps `inner` in `wrapper`, but lifts any leading and trailing formatting-neutral inlines out as
/// siblings so a span never opens or closes on a bare space. An all-neutral body drops the wrapper
/// entirely.
fn wrap_factored(wrapper: Wrapper, mut inner: Vec<Inline>) -> Vec<Inline> {
    let lead = inner.iter().take_while(|item| is_neutral(item)).count();
    let trail = inner
        .iter()
        .rev()
        .take_while(|item| is_neutral(item))
        .count()
        .min(inner.len() - lead);
    let trailing = inner.split_off(inner.len() - trail);
    let mut out: Vec<Inline> = inner.drain(..lead).collect();
    if !inner.is_empty() {
        out.push(wrapper.wrap(inner));
    }
    out.extend(trailing);
    out
}

/// Whether a character is prose whitespace that collapses to a single inter-word space. Only the
/// ASCII space, tab, and the two line-ending characters fold; every other space character (the
/// non-breaking space and the fixed-width Unicode spaces among them) is literal text and is carried
/// through verbatim.
fn is_break_space(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n' | '\r')
}
