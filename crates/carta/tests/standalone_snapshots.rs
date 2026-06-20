//! Layer 1 standalone-output golden tests: snapshot each wrapping format's default template applied
//! to one metadata-rich document. Freezes carta's own scaffold (title block, preamble, body slot)
//! for every format that ships a default template. Differential parity is the conformance suite's
//! job; these run fully offline. Reviewed with `cargo insta review`; never hand-edit the `.snap`s.

#![cfg(all(feature = "standalone", feature = "read-commonmark"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use carta::{ReaderOptions, WriterOptions, convert};

/// A document touching every title-block slot a default template can interpolate: an inline title
/// with markup, a subtitle, an author list, a date, a block-level abstract, keywords, and a body
/// with a heading, prose, and a code block (which marks a slide fragile in the slide formats).
const INPUT: &str = "\
---
title: A *Grand* Report
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
lang: en
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
    "plain", "man", "opml",
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
        let output = convert(
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
