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
    assert_eq!(
        output,
        "T=Hello \\emph{World}|PT=Hello World|F=yes|G=red,blue|\
O=from-V & raw|M=from-M|VM=from-V-vm|D=default-val|B=Body text."
    );
}
