//! Snapshot default standalone templates applied to a metadata-rich document.
//!
//! Review changes with `cargo insta review`.

#![cfg(all(feature = "standalone", feature = "read-commonmark"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use carta::{ReaderOptions, WriterOptions, convert_text};

/// A document touching every title-block and metadata slot a default template can interpolate: an
/// inline title carrying both markup and a quotation (so the plain-text identity variables keep the
/// quote glyphs), a subtitle, an author list, a date, a block-level abstract, keywords, an
/// institute, a thanks note, a description, a language and writing direction, a font size, and a
/// body with a heading, prose, and a code block (which marks a slide fragile in the slide formats).
const INPUT: &str = "\
---
title: A *Grand* \"Report\"
subtitle: On Matters of Import
author:
  - Ada Lovelace
  - Alan Turing
date: 2026-06-20
abstract: |
  A concise overview of the
  matters discussed herein.
keywords:
  - analysis
  - computing
institute: The Analytical Society
thanks: With thanks to the reviewers.
description: A short standalone document.
lang: en
dir: ltr
fontsize: 12pt
---

# Introduction

The opening remarks, with *emphasis* and a [link](https://example.com).

```rust
fn main() {}
```
";

/// Every format that ships a default standalone template.
const WRAPPING_TARGETS: &[&str] = &[
    "html", "html4", "latex", "beamer", "revealjs", "typst", "markdown", "gfm", "rst", "asciidoc",
    "plain", "man", "opml", "rtf", "docbook",
];

#[test]
fn default_template_snapshots() {
    let writers = carta::supported_output_formats();
    for &target in WRAPPING_TARGETS {
        if !writers.contains(&target) {
            continue;
        }
        let mut options = WriterOptions::default();
        options.standalone = true;
        let output = convert_text(
            "markdown",
            target,
            INPUT,
            &ReaderOptions::default(),
            &options,
        )
        .unwrap_or_else(|error| panic!("standalone markdown -> {target}: {error}"));
        insta::assert_snapshot!(target, output);
    }
}
