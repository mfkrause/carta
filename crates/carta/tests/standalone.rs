//! Standalone-output integration tests: the context builder, metadata precedence, and target-aware
//! interpolation, driven through the public `convert` entry point. A single inline template is
//! rendered to two formats so the same metadata produces format-specific markup. Fully offline.

#![cfg(all(feature = "standalone", feature = "read-commonmark"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use carta::ast::MetaValue;
use carta::{ReaderOptions, WriterOptions, convert};

/// Document with a mix of metadata kinds: an inline title (markup), a boolean, a list, and two keys
/// that also appear in higher precedence layers.
const INPUT: &str = "\
---
title: Hello *World*
flag: true
tags:
  - red
  - blue
mover: from-doc
over: from-doc
---
Body text.
";

/// A one-line template touching every context source: rendered metadata, the derived plain-text
/// page title, a conditional, a loop, the precedence-layered keys, and the body.
const TEMPLATE: &str = "T=$title$|PT=$pagetitle$|F=$if(flag)$yes$else$no$endif$|\
G=$for(tags)$$tags$$sep$,$endfor$|O=$over$|M=$mover$|VM=$vm$|D=$deflt$|B=$body$";

/// Build the standalone options shared by both targets: the inline template plus the three extra
/// metadata layers that exercise precedence.
///
/// - `over` lives in the document and in `-V`; the raw `-V` value wins (and stays unescaped).
/// - `mover` lives in the document and in `-M`; the `-M` value wins.
/// - `vm` lives in `-M` and `-V`; the `-V` value wins (`-V` is the very top).
/// - `deflt` lives only in the metadata-file defaults, below the document.
fn options() -> WriterOptions {
    let mut options = WriterOptions::default();
    options.standalone = true;
    options.template = Some(TEMPLATE.to_owned());
    options.variables = vec![
        ("over".to_owned(), "from-V & raw".to_owned()),
        ("vm".to_owned(), "from-V-vm".to_owned()),
    ];
    options.metadata = BTreeMap::from([
        ("mover".to_owned(), MetaValue::MetaString("from-M".into())),
        ("vm".to_owned(), MetaValue::MetaString("from-M-vm".into())),
    ]);
    options.metadata_defaults = BTreeMap::from([(
        "deflt".to_owned(),
        MetaValue::MetaString("default-val".into()),
    )]);
    options
}

#[cfg(feature = "write-html")]
#[test]
fn standalone_html_context_and_precedence() {
    let output = convert(
        "markdown",
        "html",
        INPUT,
        &ReaderOptions::default(),
        &options(),
    )
    .unwrap();
    assert_eq!(
        output,
        "T=Hello <em>World</em>|PT=Hello World|F=yes|G=red,blue|\
O=from-V & raw|M=from-M|VM=from-V-vm|D=default-val|B=<p>Body text.</p>"
    );
}

#[cfg(feature = "write-latex")]
#[test]
fn standalone_latex_context_and_precedence() {
    let output = convert(
        "markdown",
        "latex",
        INPUT,
        &ReaderOptions::default(),
        &options(),
    )
    .unwrap();
    // `pagetitle` is an HTML-family page-`<title>` fallback, so it is absent for LaTeX: `$pagetitle$`
    // renders empty here even though `title` is set.
    assert_eq!(
        output,
        "T=Hello \\emph{World}|PT=|F=yes|G=red,blue|\
O=from-V & raw|M=from-M|VM=from-V-vm|D=default-val|B=Body text."
    );
}

/// A document carrying the inputs the plain-text identity variables are built from: a title with
/// markup, an author list, and a date.
const IDENTITY_INPUT: &str = "\
---
title: A *Grand* Report
author:
  - Ada Lovelace
  - Alan Turing
date: 2026-06-20
---
Body.
";

/// Dumps every identity variable, with `author-meta` exercised both flat and as a loop so a list
/// value is distinguishable from a single joined string.
const IDENTITY_TEMPLATE: &str = "TM=[$title-meta$]|AM=[$author-meta$]|DM=[$date-meta$]|\
PT=[$pagetitle$]|AML=[$for(author-meta)$<$author-meta$>$sep$,$endfor$]";

fn identity_options() -> WriterOptions {
    let mut options = WriterOptions::default();
    options.standalone = true;
    options.template = Some(IDENTITY_TEMPLATE.to_owned());
    options
}

#[cfg(feature = "write-html")]
#[test]
fn web_identity_variables_expose_pagetitle_date_and_author_list() {
    let output = convert(
        "markdown",
        "html",
        IDENTITY_INPUT,
        &ReaderOptions::default(),
        &identity_options(),
    )
    .unwrap();
    // A web head exposes `pagetitle`, `date-meta`, and `author-meta` as a list (one entry per
    // author, so a flat interpolation concatenates them); `title-meta` is PDF-only and stays empty.
    assert_eq!(
        output,
        "TM=[]|AM=[Ada LovelaceAlan Turing]|DM=[2026-06-20]|PT=[A Grand Report]|\
AML=[<Ada Lovelace>,<Alan Turing>]"
    );
}

#[cfg(feature = "write-latex")]
#[test]
fn pdf_identity_variables_expose_title_meta_and_joined_authors() {
    let output = convert(
        "markdown",
        "latex",
        IDENTITY_INPUT,
        &ReaderOptions::default(),
        &identity_options(),
    )
    .unwrap();
    // A PDF document exposes `title-meta` and `author-meta` joined into one `; `-separated string (a
    // loop sees a single value); `pagetitle` and `date-meta` are web-only and stay empty.
    assert_eq!(
        output,
        "TM=[A Grand Report]|AM=[Ada Lovelace; Alan Turing]|DM=[]|PT=[]|\
AML=[<Ada Lovelace; Alan Turing>]"
    );
}

#[cfg(feature = "write-plain")]
#[test]
fn plain_title_block_shows_author_and_date_without_a_title() {
    let mut options = WriterOptions::default();
    options.standalone = true;
    let output = convert(
        "markdown",
        "plain",
        "---\nauthor: Ada Lovelace\ndate: 2026-06-20\n---\n\nBody text.\n",
        &ReaderOptions::default(),
        &options,
    )
    .unwrap();
    // The author and date head the document even though no title is set; a blank line separates the
    // block from the body.
    assert_eq!(output, "Ada Lovelace\n2026-06-20\n\nBody text.\n");
}

/// A title mixing markup with a quotation: the identity variables strip the markup but keep the
/// quotation so the target's quote glyphs survive into a `<title>` or PDF property.
const QUOTED_TITLE_INPUT: &str = "\
---
title: An *emphatic* \"Report\"
---
Body.
";

#[cfg(feature = "write-html")]
#[test]
fn web_pagetitle_strips_markup_but_keeps_quote_glyphs() {
    let mut options = WriterOptions::default();
    options.standalone = true;
    options.template = Some("[$pagetitle$]".to_owned());
    let output = convert(
        "markdown",
        "html",
        QUOTED_TITLE_INPUT,
        &ReaderOptions::default(),
        &options,
    )
    .unwrap();
    // `emphatic` loses its emphasis; the quotation around `Report` renders as the format's curly
    // quotes rather than being dropped.
    assert_eq!(output, "[An emphatic \u{201c}Report\u{201d}]");
}

#[cfg(feature = "write-latex")]
#[test]
fn pdf_title_meta_strips_markup_but_keeps_quote_glyphs() {
    let mut options = WriterOptions::default();
    options.standalone = true;
    options.template = Some("[$title-meta$]".to_owned());
    let output = convert(
        "markdown",
        "latex",
        QUOTED_TITLE_INPUT,
        &ReaderOptions::default(),
        &options,
    )
    .unwrap();
    // The quotation renders as TeX quote ligatures; the emphasis is gone.
    assert_eq!(output, "[An emphatic ``Report'']");
}

#[cfg(feature = "write-latex")]
#[test]
fn pdf_identity_variables_are_defined_even_without_metadata() {
    let mut options = WriterOptions::default();
    options.standalone = true;
    options.template = Some(
        "AML=[$for(author-meta)$<$author-meta$>$endfor$]|\
TML=[$for(title-meta)$<$title-meta$>$endfor$]"
            .to_owned(),
    );
    let output = convert(
        "markdown",
        "latex",
        "Body only.\n",
        &ReaderOptions::default(),
        &options,
    )
    .unwrap();
    // `title-meta` and `author-meta` are always defined, so a loop iterates once over the empty
    // string even when the document carries no title or author.
    assert_eq!(output, "AML=[<>]|TML=[<>]");
}

/// A block-scalar title: its YAML value is a literal block, so the title parses to block metadata
/// rather than inline. A lone paragraph still has an inline form, so its soft line break folds into a
/// space and the text flows into a single-line metadata field.
const BLOCK_SCALAR_TITLE: &str = "\
---
title: |
  Multi-line
  Title Block
---
Body.
";

/// A block-scalar title spanning two paragraphs: several blocks have no single-paragraph inline form,
/// so any variable that requires inline text comes out empty.
const TWO_PARAGRAPH_TITLE: &str = "\
---
title: |
  First Para

  Second Para
---
Body.
";

#[cfg(feature = "write-html")]
#[test]
fn web_pagetitle_flattens_a_lone_paragraph_title() {
    let mut options = WriterOptions::default();
    options.standalone = true;
    options.template = Some("[$pagetitle$]".to_owned());
    let output = convert(
        "markdown",
        "html",
        BLOCK_SCALAR_TITLE,
        &ReaderOptions::default(),
        &options,
    )
    .unwrap();
    // The block-scalar title's lone paragraph supplies its inline text; the soft break becomes a space.
    assert_eq!(output, "[Multi-line Title Block]");
}

#[cfg(feature = "write-latex")]
#[test]
fn pdf_title_meta_flattens_a_lone_paragraph_title() {
    let mut options = WriterOptions::default();
    options.standalone = true;
    options.template = Some("[$title-meta$]".to_owned());
    let output = convert(
        "markdown",
        "latex",
        BLOCK_SCALAR_TITLE,
        &ReaderOptions::default(),
        &options,
    )
    .unwrap();
    assert_eq!(output, "[Multi-line Title Block]");
}

#[cfg(feature = "write-latex")]
#[test]
fn pdf_title_meta_is_empty_for_a_multi_paragraph_title() {
    let mut options = WriterOptions::default();
    options.standalone = true;
    options.template = Some("[$title-meta$]".to_owned());
    let output = convert(
        "markdown",
        "latex",
        TWO_PARAGRAPH_TITLE,
        &ReaderOptions::default(),
        &options,
    )
    .unwrap();
    // Several blocks have no single-paragraph inline form, so the inline metadata variable is empty.
    assert_eq!(output, "[]");
}

#[cfg(feature = "write-man")]
#[test]
fn man_flattens_block_metadata_into_a_header_field() {
    let mut options = WriterOptions::default();
    options.standalone = true;
    options.template = Some("[$title$]".to_owned());

    let single = convert(
        "markdown",
        "man",
        BLOCK_SCALAR_TITLE,
        &ReaderOptions::default(),
        &options,
    )
    .unwrap();
    // The lone paragraph flattens to inline roff — no paragraph macro leaks into the header field.
    assert_eq!(single, "[Multi\\-line Title Block]");

    let multi = convert(
        "markdown",
        "man",
        TWO_PARAGRAPH_TITLE,
        &ReaderOptions::default(),
        &options,
    )
    .unwrap();
    assert_eq!(multi, "[]");
}

#[cfg(feature = "write-rst")]
#[test]
fn rst_builds_a_title_block_from_a_block_scalar_title() {
    let mut options = WriterOptions::default();
    options.standalone = true;
    options.template = Some("[$titleblock$]".to_owned());
    let output = convert(
        "markdown",
        "rst",
        BLOCK_SCALAR_TITLE,
        &ReaderOptions::default(),
        &options,
    )
    .unwrap();
    // The flattened title heads an over- and underlined title block sized to its display width.
    assert_eq!(
        output,
        "[======================\nMulti-line Title Block\n======================]"
    );
}

#[cfg(feature = "write-typst")]
#[test]
fn typst_default_template_renders_a_structured_author_name() {
    let mut options = WriterOptions::default();
    options.standalone = true;
    let output = convert(
        "markdown",
        "typst",
        "---\nauthor:\n  - name: Grace Hopper\ntitle: Hi\n---\n\nBody.\n",
        &ReaderOptions::default(),
        &options,
    )
    .unwrap();
    // A structured author exposes its `name`; it must reach both the document metadata and the title
    // block as text, never collapsing to a boolean from a non-empty map being interpolated directly.
    assert!(
        output.contains("author: ([Grace Hopper])"),
        "structured author name should reach #set document: {output}"
    );
    assert!(
        output.contains("#align(center)[Grace Hopper]"),
        "structured author should render in the title block: {output}"
    );
    assert!(
        !output.contains("[true]"),
        "a structured author must not collapse to a boolean: {output}"
    );
}
