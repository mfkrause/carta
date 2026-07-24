use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use carta_ast::{Attr, Block, Citation, CitationMode, Inline, Target};

use super::super::identifiers::HeaderNumbering;
use super::super::resolve::{
    HeaderParseCache, RefContext, gather_headers, heading_content_is_context_independent,
    resolve_block,
};
use super::super::{ExampleMap, IrBlock, LinkDef, RefMap};
use super::parse_inlines;
use carta_core::{Extension, Extensions};

static NO_DEFINED: BTreeSet<String> = BTreeSet::new();
static NO_BY_ID: BTreeMap<String, Vec<Block>> = BTreeMap::new();
static NO_EXAMPLES: ExampleMap = BTreeMap::new();

/// An empty reference context, for tests that exercise inline syntax without footnotes or example
/// references. Each call leaks a fresh citation count so a test starts numbering from zero.
fn no_notes() -> RefContext<'static> {
    RefContext {
        defined: &NO_DEFINED,
        by_id: &NO_BY_ID,
        in_definition: false,
        markdown: false,
        examples: &NO_EXAMPLES,
        cite_count: Box::leak(Box::new(Cell::new(0))),
    }
}

fn no_ext() -> Extensions {
    Extensions::empty()
}

fn exts(list: &[Extension]) -> Extensions {
    Extensions::from_list(list)
}

fn empty_refs() -> RefMap {
    BTreeMap::new()
}

fn ref_map(entries: &[(&str, &str)]) -> RefMap {
    let mut m = BTreeMap::new();
    for (k, v) in entries {
        m.insert(
            k.to_string(),
            LinkDef {
                url: v.to_string(),
                title: String::new(),
            },
        );
    }
    m
}

fn p(text: &str) -> Vec<Inline> {
    parse_inlines(text, &empty_refs(), no_notes(), no_ext())
}

fn pe(text: &str, ext: Extensions) -> Vec<Inline> {
    parse_inlines(text, &empty_refs(), no_notes(), ext)
}

/// A reference context in the markdown dialect, where escaped spaces bind as non-breaking
/// spaces, code spans trim their content, and superscripts and subscripts reject inner
/// whitespace.
fn md_notes() -> RefContext<'static> {
    RefContext {
        markdown: true,
        ..no_notes()
    }
}

fn pm(text: &str, ext: Extensions) -> Vec<Inline> {
    parse_inlines(text, &empty_refs(), md_notes(), ext)
}

fn str(s: &str) -> Inline {
    Inline::Str(s.to_owned().into())
}

fn link(content: Vec<Inline>, url: &str) -> Inline {
    Inline::Link(
        Box::default(),
        content,
        Box::new(Target {
            url: url.to_owned().into(),
            title: carta_ast::Text::default(),
        }),
    )
}

fn image(alt: Vec<Inline>, url: &str) -> Inline {
    Inline::Image(
        Box::default(),
        alt,
        Box::new(Target {
            url: url.to_owned().into(),
            title: carta_ast::Text::default(),
        }),
    )
}

// --- TeX math ---

fn math_inline(content: &str) -> Inline {
    Inline::Math(carta_ast::MathType::InlineMath, content.to_owned().into())
}

fn math_display(content: &str) -> Inline {
    Inline::Math(carta_ast::MathType::DisplayMath, content.to_owned().into())
}

fn math() -> Extensions {
    exts(&[Extension::TexMathDollars])
}

// --- Attributes: spans, inline code, links ---

fn span(attr: Attr, content: Vec<Inline>) -> Inline {
    Inline::Span(Box::new(attr), content)
}

fn attr(id: &str, classes: &[&str], kv: &[(&str, &str)]) -> Attr {
    Attr {
        id: id.to_owned().into(),
        classes: classes.iter().map(|c| (*c).into()).collect(),
        attributes: kv.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect(),
    }
}

fn attrs() -> Extensions {
    exts(&[Extension::Attributes])
}

// --- Inline raw attribute (`{=FORMAT}` on a code span) ---

fn raw(format: &str, text: &str) -> Inline {
    Inline::RawInline(
        carta_ast::Format(format.to_owned().into()),
        text.to_owned().into(),
    )
}

fn code(text: &str) -> Inline {
    Inline::Code(Box::default(), text.to_owned().into())
}

// --- Inline raw TeX and backslash math ---

fn tex(source: &str) -> Inline {
    Inline::RawInline(
        carta_ast::Format("tex".to_owned().into()),
        source.to_owned().into(),
    )
}

fn raw_tex() -> Extensions {
    exts(&[Extension::RawTex])
}

fn single_math() -> Extensions {
    exts(&[Extension::TexMathSingleBackslash])
}

fn double_math() -> Extensions {
    exts(&[Extension::TexMathDoubleBackslash])
}

// --- Native spans (`<span …>` … `</span>`) ---

fn native() -> Extensions {
    exts(&[Extension::NativeSpans])
}

// --- Mark (highlight) ---

fn mark(content: Vec<Inline>) -> Inline {
    span(attr("", &["mark"], &[]), content)
}

// --- Citations ---

fn cites() -> Extensions {
    exts(&[Extension::Citations])
}

fn cite(citations: Vec<Citation>, fallback: Vec<Inline>) -> Inline {
    Inline::Cite(citations, fallback)
}

fn citation(
    id: &str,
    prefix: Vec<Inline>,
    suffix: Vec<Inline>,
    mode: CitationMode,
    note_num: i32,
) -> Citation {
    Citation {
        id: id.to_owned().into(),
        prefix,
        suffix,
        mode,
        note_num,
        hash: 0,
    }
}

#[path = "inline_parse_tests/attributes.rs"]
mod attributes;
#[path = "inline_parse_tests/citations.rs"]
mod citations;
#[path = "inline_parse_tests/emphasis.rs"]
mod emphasis;
#[path = "inline_parse_tests/extensions.rs"]
mod extensions;
#[path = "inline_parse_tests/links.rs"]
mod links;
#[path = "inline_parse_tests/math.rs"]
mod math;
#[path = "inline_parse_tests/native.rs"]
mod native;
