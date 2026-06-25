# 📜 carta

A universal document converter written in Rust that reads a markup format and renders it back out in another. A performant and lightweight reimplementation of [pandoc](https://pandoc.org).

> [!WARNING]
> carta is a **prototype under active development**. Only a small slice of pandoc's formats is implemented today, the API is unstable, and it is not yet ready for production use.

## Goals

- **Performance** and a **smaller binary** than pandoc.
- **Feature parity** with pandoc across all formats and extensions.
- A **DX-friendly library**, with the CLI as a thin shell over it.

## Status

This tracks carta's status of all formats pandoc supports. See [`STATUS.md`](docs/STATUS.md) for a detailed per-format breakdown including extension coverage, and the full feature backlog.

✅ complete · 🚧 in progress · ❌ not started · ➖ not applicable (pandoc has no such direction)

**Markdown family**

| Format | Reader | Writer |
| --- | :---: | :---: |
| CommonMark (`commonmark`) | ✅ | ✅ |
| CommonMark-X (`commonmark_x`) | ✅ | ❌ |
| GitHub-Flavored Markdown (`gfm`) | ✅ | ✅ |
| Pandoc Markdown (`markdown`) | 🚧 | 🚧 |
| Markdown strict (`markdown_strict`) | ❌ | ❌ |
| MultiMarkdown (`markdown_mmd`) | ❌ | ❌ |
| PHP Markdown Extra (`markdown_phpextra`) | ❌ | ❌ |
| GitHub Markdown, legacy (`markdown_github`) | ❌ | ❌ |
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
| LaTeX (`latex`) | ❌ | ✅ |
| Typst (`typst`) | ❌ | ✅ |
| ConTeXt (`context`) | ➖ | ❌ |
| Texinfo (`texinfo`) | ➖ | ❌ |
| PDF (`pdf`) | ➖ | ❌ |

**Lightweight markup**

| Format | Reader | Writer |
| --- | :---: | :---: |
| reStructuredText (`rst`) | ❌ | ✅ |
| AsciiDoc (`asciidoc`) | ❌ | ✅ |
| AsciiDoc legacy (`asciidoc_legacy`) | ➖ | ❌ |
| Asciidoctor (`asciidoctor`) | ➖ | ❌ |
| Org mode (`org`) | ❌ | ❌ |
| Textile (`textile`) | ❌ | ❌ |
| Muse (`muse`) | ❌ | ❌ |
| Haddock (`haddock`) | ❌ | ❌ |
| txt2tags (`t2t`) | ❌ | ➖ |
| Perl POD (`pod`) | ❌ | ➖ |

**Wikis**

| Format | Reader | Writer |
| --- | :---: | :---: |
| MediaWiki (`mediawiki`) | ❌ | ✅ |
| DokuWiki (`dokuwiki`) | ❌ | ✅ |
| Jira (`jira`) | ❌ | ✅ |
| Creole (`creole`) | ❌ | ➖ |
| TikiWiki (`tikiwiki`) | ❌ | ➖ |
| TWiki (`twiki`) | ❌ | ➖ |
| Vimwiki (`vimwiki`) | ❌ | ➖ |
| XWiki (`xwiki`) | ➖ | ❌ |
| ZimWiki (`zimwiki`) | ➖ | ❌ |

**roff**

| Format | Reader | Writer |
| --- | :---: | :---: |
| man (`man`) | ❌ | ✅ |
| mdoc (`mdoc`) | ❌ | ➖ |
| ms (`ms`) | ➖ | ❌ |
| vimdoc (`vimdoc`) | ➖ | ❌ |

**Word processor, ebook & notebook**

| Format | Reader | Writer |
| --- | :---: | :---: |
| Word (`docx`) | ❌ | ❌ |
| OpenDocument Text (`odt`) | ❌ | ❌ |
| OpenDocument (`opendocument`) | ➖ | ❌ |
| EPUB (`epub`, `epub2`, `epub3`) | ❌ | ❌ |
| Jupyter Notebook (`ipynb`) | ❌ | ❌ |
| FictionBook2 (`fb2`) | ❌ | ❌ |
| InDesign ICML (`icml`) | ➖ | ❌ |
| Rich Text Format (`rtf`) | ❌ | ❌ |
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

No releases yet. You can build from source with Rust 1.93+.

```sh
cd carta
cargo build --release
# binary at target/release/carta
```

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

# discover what this build supports
carta --list-input-formats
carta --list-output-formats
carta --list-extensions          # extensions for the Markdown dialect
carta --list-extensions=gfm      # extensions and defaults for a given format
```

### Library

```rust
use carta::{convert, ReaderOptions, WriterOptions};

let html = convert(
    "commonmark",
    "html",
    "# Hello, *world*",
    &ReaderOptions::default(),
    &WriterOptions::default(),
)?;
```

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

The workspace splits into `carta-ast` (the document model), `carta-core` (shared traits and options), `carta-readers`, `carta-writers`, the `carta` library facade, and the `carta-cli` binary.

## License

Copyright © 2026 Maximilian Krause.

carta is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License, version 3, as published by the Free Software Foundation. See [`LICENSE`](LICENSE) for the full text.
