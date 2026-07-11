<div align="center">

<img src="assets/logo.png" alt="carta logo" width="140" height="140">

# carta

**A universal document converter written in Rust.** Read a markup format, render it back out in another. A performant and lightweight reimplementation of [pandoc](https://pandoc.org).

[![crates.io](https://img.shields.io/crates/v/carta.svg)](https://crates.io/crates/carta)
[![docs.rs](https://img.shields.io/docsrs/carta)](https://docs.rs/carta)
[![CI](https://github.com/mfkrause/carta/actions/workflows/ci.yml/badge.svg)](https://github.com/mfkrause/carta/actions/workflows/ci.yml)
[![License: AGPL-3.0](https://img.shields.io/badge/license-AGPL--3.0-blue.svg)](LICENSE)
![MSRV](https://img.shields.io/badge/MSRV-1.93-orange.svg)

</div>

> [!WARNING]
> carta is an **early-stage alpha under active development**. Not all of pandoc's formats are implemented yet and the API is still unstable.

## Goals

- **Performance** and a **smaller binary** than pandoc.
- **Feature parity** with pandoc across all formats and extensions.
- A **developer-friendly library**, with the CLI as a thin shell over it.

## Status

This tracks carta's status of all formats pandoc supports. See [`STATUS.md`](docs/STATUS.md) for a detailed per-format breakdown including extension coverage, and the full feature backlog.

✅ usable — basically done; any remaining parity gaps are minor and unlikely to affect regular use · 🚧 in development — large parity gaps or breaking issues (e.g. panics), not recommended for use yet · ❌ not started · ➖ not applicable (pandoc has no such direction)

**Markdown family**

| Format | Reader | Writer |
| --- | :---: | :---: |
| CommonMark (`commonmark`) | ✅ | ✅ |
| CommonMark-X (`commonmark_x`) | ✅ | ✅ |
| GitHub-Flavored Markdown (`gfm`) | ✅ | ✅ |
| Pandoc Markdown (`markdown`) | ✅ | ✅ |
| Markdown strict (`markdown_strict`) | ✅ | ✅ |
| MultiMarkdown (`markdown_mmd`) | ✅ | ✅ |
| PHP Markdown Extra (`markdown_phpextra`) | ✅ | ✅ |
| GitHub Markdown, legacy (`markdown_github`) | ✅ | ✅ |
| Djot (`djot`) | ❌ | ❌ |
| Markua (`markua`) | ➖ | ❌ |

**HTML & slides**

| Format | Reader | Writer |
| --- | :---: | :---: |
| HTML (`html`, `html5`, `html4`) | ✅ | ✅ |
| Chunked HTML (`chunkedhtml`) | ➖ | ❌ |
| reveal.js (`revealjs`) | ➖ | ✅ |
| Beamer (`beamer`) | ➖ | ✅ |
| Slidy (`slidy`) | ➖ | ❌ |
| S5 (`s5`) | ➖ | ❌ |
| Slideous (`slideous`) | ➖ | ❌ |
| DZSlides (`dzslides`) | ➖ | ❌ |
| PowerPoint (`pptx`) | ❌ | ❌ |

**TeX & typesetting**

| Format | Reader | Writer |
| --- | :---: | :---: |
| LaTeX (`latex`) | ✅ | ✅ |
| Typst (`typst`) | ❌ | ✅ |
| ConTeXt (`context`) | ➖ | ❌ |
| Texinfo (`texinfo`) | ➖ | ❌ |
| PDF (`pdf`) | ➖ | ❌ |

**Lightweight markup**

| Format | Reader | Writer |
| --- | :---: | :---: |
| reStructuredText (`rst`) | ✅ | ✅ |
| AsciiDoc (`asciidoc`) | ❌ | ✅ |
| AsciiDoc legacy (`asciidoc_legacy`) | ➖ | ❌ |
| Asciidoctor (`asciidoctor`) | ➖ | ❌ |
| Org mode (`org`) | ✅ | ✅ |
| Textile (`textile`) | ❌ | ❌ |
| Muse (`muse`) | ❌ | ❌ |
| Haddock (`haddock`) | ❌ | ❌ |
| txt2tags (`t2t`) | ❌ | ➖ |
| Perl POD (`pod`) | ❌ | ➖ |

**Wikis**

| Format | Reader | Writer |
| --- | :---: | :---: |
| MediaWiki (`mediawiki`) | ✅ | ✅ |
| DokuWiki (`dokuwiki`) | ✅ | ✅ |
| Jira (`jira`) | ✅ | ✅ |
| Creole (`creole`) | ❌ | ➖ |
| TikiWiki (`tikiwiki`) | ❌ | ➖ |
| TWiki (`twiki`) | ❌ | ➖ |
| Vimwiki (`vimwiki`) | ❌ | ➖ |
| XWiki (`xwiki`) | ➖ | ❌ |
| ZimWiki (`zimwiki`) | ➖ | ❌ |

**roff**

| Format | Reader | Writer |
| --- | :---: | :---: |
| man (`man`) | ✅ | ✅ |
| mdoc (`mdoc`) | ❌ | ➖ |
| ms (`ms`) | ➖ | ❌ |
| vimdoc (`vimdoc`) | ➖ | ❌ |

**Word processor, ebook & notebook**

| Format | Reader | Writer |
| --- | :---: | :---: |
| Word (`docx`) | ❌ | ✅ |
| OpenDocument Text (`odt`) | ❌ | ❌ |
| OpenDocument (`opendocument`) | ➖ | ❌ |
| EPUB (`epub`, `epub2`, `epub3`) | ❌ | ✅ |
| Jupyter Notebook (`ipynb`) | ✅ | ✅ |
| FictionBook2 (`fb2`) | ❌ | ❌ |
| InDesign ICML (`icml`) | ➖ | ❌ |
| Rich Text Format (`rtf`) | ✅ | ✅ |
| Spreadsheet (`xlsx`) | ❌ | ➖ |

**XML & publishing**

| Format | Reader | Writer |
| --- | :---: | :---: |
| DocBook (`docbook`, `docbook4`, `docbook5`) | ❌ | ❌ |
| JATS (`jats`, `jats_archiving`, `jats_articleauthoring`, `jats_publishing`) | ❌ | ❌ |
| BITS (`bits`) | ❌ | ➖ |
| TEI (`tei`) | ➖ | ❌ |
| Generic XML (`xml`) | ❌ | ❌ |

**Bibliography**

| Format | Reader | Writer |
| --- | :---: | :---: |
| BibTeX (`bibtex`) | ❌ | ❌ |
| BibLaTeX (`biblatex`) | ❌ | ❌ |
| CSL JSON (`csljson`) | ❌ | ❌ |
| RIS (`ris`) | ❌ | ➖ |
| EndNote XML (`endnotexml`) | ❌ | ➖ |

**Data, interchange & terminal**

| Format | Reader | Writer |
| --- | :---: | :---: |
| Pandoc JSON (`json`) | ✅ | ✅ |
| Native Pandoc AST (`native`) | ✅ | ✅ |
| OPML (`opml`) | ✅ | ✅ |
| CSV (`csv`) | ✅ | ➖ |
| TSV (`tsv`) | ✅ | ➖ |
| Plain text (`plain`) | ➖ | ✅ |
| BBCode (`bbcode`, `bbcode_phpbb`, `bbcode_steam`, …) | ➖ | ❌ |
| ANSI terminal (`ansi`) | ➖ | ❌ |

## Installation

### Prebuilt binaries

Download the archive for your platform from the [latest release][latest-release]. Builds are provided for Linux (x86-64 gnu and static musl, arm64), macOS (Intel and Apple Silicon), and Windows (x86-64).

### From crates.io

```sh
cargo install carta
```

This installs the `carta` binary. For a smaller build, pass `--no-default-features` with only the formats you need, e.g. `--features cli,read-commonmark,write-html`.

### From source

```sh
git clone https://github.com/mfkrause/carta
cd carta
cargo build --release
# binary at target/release/carta
```

[latest-release]: https://github.com/mfkrause/carta/releases/latest

## Usage

### Command line

```sh
# CommonMark to HTML
carta -f commonmark -t html input.md -o output.html

# read from stdin, write to stdout
echo '# Hello' | carta -f commonmark -t html

# inspect the document model
carta -f commonmark -t json input.md

# standalone document with a table of contents and numbered sections
carta -f commonmark -t html -s --toc --number-sections input.md -o output.html

# render HTML math with MathJax (or --katex)
carta -f commonmark -t html -s --mathjax input.md -o output.html

# colorize code blocks with a named theme (--no-highlight turns it off)
carta -f commonmark -t html -s --highlight-style=breezedark input.md -o output.html

# extract a notebook's embedded images to files, rewriting the references
carta -f ipynb -t markdown --extract-media=media notebook.ipynb -o notebook.md

# embed a document's images into a container format, searching extra directories for them
carta -f commonmark -t docx --resource-path=assets:img input.md -o output.docx

# produce a self-contained HTML file with every image inlined as a data: URI
carta -f commonmark -t html -s --embed-resources input.md -o output.html

# transform the document through a JSON filter before writing (repeatable, applied in order)
carta -f commonmark -t html -F ./my-filter.py input.md -o output.html

# discover what this build supports
carta --list-input-formats
carta --list-output-formats
carta --list-extensions          # extensions for the Markdown dialect
carta --list-extensions=gfm      # extensions and defaults for a given format
carta --list-highlight-languages # languages the highlighter can colorize
carta --list-highlight-styles    # built-in color themes
```

### Library

```rust
use carta::{convert_text, ReaderOptions, WriterOptions};

let html = convert_text(
    "commonmark",
    "html",
    "# Hello, *world*",
    &ReaderOptions::default(),
    &WriterOptions::default(),
)?;
```

`convert_text` is the shortcut for text-to-text conversion. The general entry point is `convert`, which takes raw bytes and returns an `Output` that is text or bytes depending on the target format — use it when either side is a binary format.

You can select formats at compile time via per-direction features to make binaries even more lightweight for your individual needs.

```sh
cargo build -p carta --no-default-features --features read-commonmark,write-html
```

## Development

```sh
cargo build                         # build the workspace
cargo nextest run --workspace       # run tests
cargo clippy --all-targets          # lint
cargo +nightly fuzz run commonmark  # fuzz a reader (see fuzz/README.md)
```

The workspace splits into `carta-ast` (the document model), `carta-core` (shared traits and options), `carta-readers`, `carta-writers`, and `carta` (the library facade, which also ships the command-line binary behind its `cli` feature).

## License

Copyright © 2026 Maximilian Krause.

carta is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License, version 3, as published by the Free Software Foundation. See [`LICENSE`](LICENSE) for the full text.
