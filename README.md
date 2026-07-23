<div align="center">

<img src="assets/logo.png" alt="carta logo" width="140" height="140">

# carta

**A universal document converter written in Rust.** Read a markup format, render it back out in another. A fast, lightweight reimplementation of [pandoc](https://github.com/jgm/pandoc).

[![crates.io](https://img.shields.io/crates/v/carta.svg)](https://crates.io/crates/carta)
[![docs.rs](https://img.shields.io/docsrs/carta)](https://docs.rs/carta)
[![CI](https://github.com/mfkrause/carta/actions/workflows/ci.yml/badge.svg)](https://github.com/mfkrause/carta/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
![MSRV](https://img.shields.io/badge/MSRV-1.93-orange.svg)

</div>

> [!WARNING]
> carta is still in active development. Not all of pandoc's formats and features are implemented yet and the API is still unstable.

## Goals

- Performance and a lightweight binary.
- A developer-friendly library, with the CLI as a thin shell over it.
- Feature parity with pandoc across all formats and extensions.

## Status

carta already supports the most-used pandoc formats; the rest are in active development. [`STATUS.md`](docs/STATUS.md) has a per-format breakdown, extension coverage, and the full feature backlog.

## Installation

### Prebuilt binaries

Download the archive for your platform from the [latest release][latest-release]. Builds are provided for Linux (x86-64 gnu and static musl, arm64), macOS (Intel and Apple Silicon), and Windows (x86-64).

With [cargo-binstall](https://github.com/cargo-bins/cargo-binstall) installed, the matching prebuilt binary can be fetched directly:

```sh
cargo binstall carta
```

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

# discover what this build supports
carta --list-input-formats
carta --list-output-formats
carta --list-extensions=gfm
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

`convert_text` is the shortcut for text-to-text conversion. The general entry point is `convert`, which takes raw bytes and returns an `Output` that is text or bytes depending on the target format.

You can select formats at compile time via per-direction features to shrink the binary further.

```sh
cargo build -p carta --no-default-features --features read-commonmark,write-html
```

### Syntax-highlighting grammars

carta highlights code blocks using KDE-format syntax definitions. The permissively licensed definitions (Rust, Swift, TypeScript, Markdown, and others) are compiled into the binary. The rest of the catalog (C, Python, JavaScript, Bash, JSON, and more) carries copyleft or unspecified upstream licenses, so it is not compiled in; it ships with the [release archives][latest-release] as a `syntax/` directory that carta discovers automatically next to the executable. You can also place definitions in your data directory (`~/.local/share/carta/syntax`), point `$CARTA_SYNTAX_DIR` at a directory (an empty value disables directory loading), or pass individual files with `--syntax-definition`.

When building from source, either copy `crates/carta-highlight/data/syntax-copyleft/` to one of those locations, or embed the full catalog with the default-off `embed-copyleft-grammars` feature. A binary built this way bundles copyleft-licensed data:

```sh
cargo build -p carta --release --features embed-copyleft-grammars
```

## Development

```sh
cargo build                         # build the workspace
cargo nextest run --workspace       # run tests
cargo clippy --all-targets          # lint
```

## License

Copyright © 2026 Maximilian Krause.

Licensed under either of

- Apache License, Version 2.0 ([`LICENSE-APACHE`](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([`LICENSE-MIT`](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

The syntax-highlighting grammar files under `crates/carta-highlight/data/` are third-party works that retain their own upstream licenses; see [`vendor/syntax-highlighting/ATTRIBUTION.md`](vendor/syntax-highlighting/ATTRIBUTION.md) for the per-file breakdown. Only the permissively licensed grammars are compiled into carta, so the built artifacts are covered by the license above in full. The remaining grammars (copyleft or unspecified licenses) ship as a separate runtime-loaded pack; see [Syntax-highlighting grammars](#syntax-highlighting-grammars).

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
