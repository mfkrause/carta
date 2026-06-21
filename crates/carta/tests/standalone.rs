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
        (
            "mover".to_owned(),
            MetaValue::MetaString("from-M".to_owned()),
        ),
        (
            "vm".to_owned(),
            MetaValue::MetaString("from-M-vm".to_owned()),
        ),
    ]);
    options.metadata_defaults = BTreeMap::from([(
        "deflt".to_owned(),
        MetaValue::MetaString("default-val".to_owned()),
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
