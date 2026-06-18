//! Slide-deck writer: renders the document model to frames of a LaTeX presentation.
//!
//! The block sequence is split into slides at a computed slide level (see [`crate::slides`]):
//! headers above it become sectioning commands, headers at it open titled frames, and the rest
//! gathers into frame bodies. Frame content reuses the LaTeX block and inline rendering in its
//! slide dialect. A frame holding any code is marked fragile; a header's recognized presentation
//! classes become frame options. Headers below the slide level nest the body into block
//! environments. The result is a body fragment with no preamble and no trailing newline; this
//! format has no public specification.

use carta_ast::{Attr, Block, Document, Inline};
use carta_core::{Result, Writer, WriterOptions};

use crate::latex::{Dialect, anchor, render_fragment, render_heading, render_titled_open};
use crate::slides::{FrameTitle, Heading, Slide, group_headings, segment, slide_level};

/// Renders a document to a slide-deck fragment.
#[derive(Debug, Default, Clone, Copy)]
pub struct BeamerWriter;

impl Writer for BeamerWriter {
    fn write(&self, document: &Document, _options: &WriterOptions) -> Result<String> {
        if document.blocks.is_empty() {
            return Ok("\\begin{frame}\n\\end{frame}".to_owned());
        }
        let level = slide_level(&document.blocks);
        let units: Vec<String> = segment(&document.blocks, level)
            .iter()
            .map(|slide| render_slide(slide, level))
            .collect();
        Ok(units.join("\n\n"))
    }
}

/// The frame classes recognized as presentation options, in the order they are emitted within a
/// frame's `[…]`. A class not in this set is dropped; `fragile` is added on its own when the frame
/// holds code.
const FRAME_CLASSES: &[&str] = &[
    "allowdisplaybreaks",
    "allowframebreaks",
    "b",
    "c",
    "containsverbatim",
    "environment",
    "fragile",
    "label",
    "noframenumbering",
    "plain",
    "s",
    "shrink",
    "squeeze",
    "standout",
    "t",
];

fn render_slide(slide: &Slide, level: i32) -> String {
    match slide {
        Slide::Section { level, attr, title } => render_heading(*level, attr, title),
        Slide::Frame { title, body } => render_frame(title.as_ref(), body, level),
    }
}

/// Render one frame: the `\begin{frame}` line with options and title, an anchor for the title's
/// identifier, the body grouped into block environments below the slide level, and `\end{frame}`.
fn render_frame(title: Option<&FrameTitle>, body: &[Block], level: i32) -> String {
    let fragile = contains_code(body);
    let options = frame_options(title, fragile);
    let mut lines = Vec::new();
    match title {
        Some(title) => {
            lines.push(render_titled_open(
                &format!("\\begin{{frame}}{options}{{"),
                title.inlines,
                Dialect::SLIDE,
            ));
            if !title.attr.id.is_empty() {
                lines.push(anchor(&title.attr.id));
            }
        }
        None => lines.push(format!("\\begin{{frame}}{options}")),
    }
    let rendered = render_body(body, level + 1);
    if !rendered.is_empty() {
        lines.push(rendered);
    }
    lines.push("\\end{frame}".to_owned());
    lines.join("\n")
}

/// The bracketed option list for a frame: the title's recognized classes (preserving their order),
/// a `label=` from the title's attributes, and `fragile` when the frame holds code. Empty when
/// there are no options.
fn frame_options(title: Option<&FrameTitle>, fragile: bool) -> String {
    let mut options: Vec<String> = Vec::new();
    if let Some(title) = title {
        for class in &title.attr.classes {
            if FRAME_CLASSES.contains(&class.as_str()) {
                options.push(class.clone());
            }
        }
        if let Some((_, value)) = title.attr.attributes.iter().find(|(key, _)| key == "label") {
            options.push(format!("label={value}"));
        }
    }
    if fragile && !options.iter().any(|option| option == "fragile") {
        options.push("fragile".to_owned());
    }
    if options.is_empty() {
        String::new()
    } else {
        format!("[{}]", options.join(","))
    }
}

/// Render a frame body, turning headers at or below `block_level` into nested block environments
/// and rendering everything else through the LaTeX slide dialect. Each block environment's body is
/// grouped one level deeper.
fn render_body(blocks: &[Block], block_level: i32) -> String {
    let Some(group_level) = shallowest_header(blocks) else {
        return render_fragment(blocks, Dialect::SLIDE);
    };
    let group_level = group_level.max(block_level);
    let units: Vec<String> = group_headings(blocks, group_level)
        .iter()
        .map(|heading| match heading {
            Heading::Loose(run) => render_fragment(run, Dialect::SLIDE),
            Heading::Section { attr, title, body } => {
                render_block_env(attr, title, body, group_level)
            }
        })
        .filter(|rendered| !rendered.is_empty())
        .collect();
    units.join("\n\n")
}

/// Render a header below the slide level as a `block` environment: a titled opening, an anchor for
/// its identifier, the recursively grouped body, and the close.
fn render_block_env(attr: &Attr, title: &[Inline], body: &[Block], group_level: i32) -> String {
    let mut lines = vec![render_titled_open("\\begin{block}{", title, Dialect::SLIDE)];
    if !attr.id.is_empty() {
        lines.push(anchor(&attr.id));
    }
    let rendered = render_body(body, group_level + 1);
    if !rendered.is_empty() {
        lines.push(rendered);
    }
    lines.push("\\end{block}".to_owned());
    lines.join("\n")
}

/// The shallowest header level in a block run, or `None` when it holds no headers.
fn shallowest_header(blocks: &[Block]) -> Option<i32> {
    blocks
        .iter()
        .filter_map(|block| match block {
            Block::Header(level, _, _) => Some(*level),
            _ => None,
        })
        .min()
}

/// Whether a block run holds any code, marking its frame fragile. Code may be a code block or an
/// inline code span, nested arbitrarily deep within other blocks.
fn contains_code(blocks: &[Block]) -> bool {
    blocks.iter().any(block_contains_code)
}

fn block_contains_code(block: &Block) -> bool {
    match block {
        Block::CodeBlock(_, _) => true,
        Block::Plain(inlines) | Block::Para(inlines) | Block::Header(_, _, inlines) => {
            inlines.iter().any(inline_contains_code)
        }
        Block::LineBlock(lines) => lines
            .iter()
            .any(|line| line.iter().any(inline_contains_code)),
        Block::BlockQuote(blocks) | Block::Div(_, blocks) | Block::Figure(_, _, blocks) => {
            contains_code(blocks)
        }
        Block::BulletList(items) | Block::OrderedList(_, items) => {
            items.iter().any(|item| contains_code(item))
        }
        Block::DefinitionList(items) => items.iter().any(|(term, definitions)| {
            term.iter().any(inline_contains_code)
                || definitions.iter().any(|blocks| contains_code(blocks))
        }),
        Block::Table(table) => table_contains_code(table),
        Block::RawBlock(_, _) | Block::HorizontalRule => false,
    }
}

fn inline_contains_code(inline: &Inline) -> bool {
    match inline {
        Inline::Code(_, _) => true,
        Inline::Emph(inlines)
        | Inline::Underline(inlines)
        | Inline::Strong(inlines)
        | Inline::Strikeout(inlines)
        | Inline::Superscript(inlines)
        | Inline::Subscript(inlines)
        | Inline::SmallCaps(inlines)
        | Inline::Quoted(_, inlines)
        | Inline::Cite(_, inlines)
        | Inline::Link(_, inlines, _)
        | Inline::Image(_, inlines, _)
        | Inline::Span(_, inlines) => inlines.iter().any(inline_contains_code),
        Inline::Note(blocks) => contains_code(blocks),
        Inline::Str(_)
        | Inline::Space
        | Inline::SoftBreak
        | Inline::LineBreak
        | Inline::Math(_, _)
        | Inline::RawInline(_, _) => false,
    }
}

fn table_contains_code(table: &carta_ast::Table) -> bool {
    let body_rows = table
        .bodies
        .iter()
        .flat_map(|body| body.head.iter().chain(body.body.iter()));
    let rows = table
        .head
        .rows
        .iter()
        .chain(body_rows)
        .chain(table.foot.rows.iter());
    let cell_code = rows
        .flat_map(|row| row.cells.iter())
        .any(|cell| contains_code(&cell.content));
    let caption_code = contains_code(&table.caption.long)
        || table
            .caption
            .short
            .iter()
            .flatten()
            .any(inline_contains_code);
    cell_code || caption_code
}

#[cfg(test)]
mod tests {
    use super::*;
    use carta_ast::Attr;

    fn render(blocks: Vec<Block>) -> String {
        BeamerWriter
            .write(
                &Document {
                    blocks,
                    ..Document::default()
                },
                &WriterOptions::default(),
            )
            .unwrap()
    }

    fn para(text: &str) -> Block {
        Block::Para(vec![Inline::Str(text.to_owned())])
    }

    fn header(level: i32, id: &str, title: &str) -> Block {
        Block::Header(
            level,
            Attr {
                id: id.to_owned(),
                ..Attr::default()
            },
            vec![Inline::Str(title.to_owned())],
        )
    }

    #[test]
    fn bare_content_is_a_titleless_frame() {
        assert_eq!(render(vec![para("x")]), "\\begin{frame}\nx\n\\end{frame}");
    }

    #[test]
    fn horizontal_rule_splits_frames() {
        let out = render(vec![para("a"), Block::HorizontalRule, para("b")]);
        assert_eq!(
            out,
            "\\begin{frame}\na\n\\end{frame}\n\n\\begin{frame}\nb\n\\end{frame}"
        );
    }

    #[test]
    fn slide_level_header_titles_a_frame() {
        let out = render(vec![header(6, "six", "T"), para("y")]);
        assert!(out.starts_with("\\begin{frame}{T}\n\\protect\\phantomsection\\label{six}\ny"));
    }

    #[test]
    fn shallow_header_is_a_section() {
        let out = render(vec![header(1, "a", "A"), header(2, "b", "B"), para("y")]);
        assert!(out.contains("\\section{A}\\label{a}"));
        assert!(out.contains("\\begin{frame}{B}"));
    }

    #[test]
    fn code_block_marks_frame_fragile() {
        let out = render(vec![Block::CodeBlock(Attr::default(), "x\n".to_owned())]);
        assert!(out.starts_with("\\begin{frame}[fragile]"));
    }

    #[test]
    fn inline_code_marks_frame_fragile() {
        let out = render(vec![Block::Para(vec![Inline::Code(
            Attr::default(),
            "x".to_owned(),
        )])]);
        assert!(out.starts_with("\\begin{frame}[fragile]"));
    }

    #[test]
    fn deep_header_becomes_block_environment() {
        let out = render(vec![
            header(1, "a", "A"),
            para("x"),
            header(2, "b", "B"),
            para("y"),
        ]);
        assert!(out.contains("\\begin{block}{B}"));
        assert!(out.contains("\\end{block}"));
    }

    #[test]
    fn recognized_class_becomes_frame_option() {
        let mut attr = Attr {
            id: "six".to_owned(),
            ..Attr::default()
        };
        attr.classes = vec!["allowframebreaks".to_owned(), "unknown".to_owned()];
        let out = render(vec![
            Block::Header(6, attr, vec![Inline::Str("T".to_owned())]),
            para("y"),
        ]);
        assert!(out.starts_with("\\begin{frame}[allowframebreaks]{T}"));
    }

    #[test]
    fn empty_document_is_an_empty_frame() {
        assert_eq!(render(vec![]), "\\begin{frame}\n\\end{frame}");
    }

    fn div(classes: &[&str], blocks: Vec<Block>) -> Block {
        Block::Div(
            Attr {
                classes: classes.iter().map(|class| (*class).to_owned()).collect(),
                ..Attr::default()
            },
            blocks,
        )
    }

    fn bullet(items: Vec<&str>) -> Block {
        Block::BulletList(items.into_iter().map(|item| vec![para(item)]).collect())
    }

    #[test]
    fn incremental_div_adds_overlay_to_list() {
        let out = render(vec![div(&["incremental"], vec![bullet(vec!["a", "b"])])]);
        assert!(out.contains("\\begin{itemize}[<+->]"));
    }

    #[test]
    fn nonincremental_div_keeps_plain_list() {
        let out = render(vec![div(&["nonincremental"], vec![bullet(vec!["a", "b"])])]);
        assert!(out.contains("\\begin{itemize}"));
        assert!(!out.contains("[<+->]"));
    }

    #[test]
    fn columns_div_emits_columns_environment() {
        let column = div(&["column"], vec![para("left")]);
        let out = render(vec![div(&["columns"], vec![column])]);
        assert!(out.contains("\\begin{columns}"));
        assert!(out.contains("\\begin{column}"));
        assert!(out.contains("\\end{column}"));
        assert!(out.contains("\\end{columns}"));
    }

    #[test]
    fn block_quote_of_list_unwraps_to_overlaid_list() {
        let out = render(vec![Block::BlockQuote(vec![bullet(vec!["a", "b"])])]);
        assert!(out.contains("\\begin{itemize}[<+->]"));
        assert!(!out.contains("\\begin{quote}"));
    }

    #[test]
    fn long_frame_title_is_not_wrapped_mid_command() {
        let title = "Lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor";
        let out = render(vec![header(6, "id", title)]);
        assert!(out.starts_with(&format!("\\begin{{frame}}{{{title}}}")));
    }
}
