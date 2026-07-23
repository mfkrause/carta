<div align="center">

<img src="assets/logo.png" alt="carta logo" width="140" height="140">

# carta

**A universal document converter written in Rust.** Read a markup format, render it back out in another. A fast, lightweight reimplementation of [pandoc](https://github.com/jgm/pandoc).

[![crates.io](https://img.shields.io/crates/v/carta.svg)](https://crates.io/crates/carta)
[![docs.rs](https://img.shields.io/docsrs/carta)](https://docs.rs/carta)
[![CI](https://github.com/mfkrause/carta/actions/workflows/ci.yml/badge.svg)](https://github.com/mfkrause/carta/actions/workflows/ci.yml)
[![License: AGPL-3.0](https://img.shields.io/badge/license-AGPL--3.0-blue.svg)](LICENSE)
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

## Development

```sh
cargo build                         # build the workspace
cargo nextest run --workspace       # run tests
cargo clippy --all-targets          # lint
```

## License

Copyright © 2026 Maximilian Krause.

carta is free software: you can redistribute it and/or modify it under the terms of the GNU Affero General Public License, version 3, as published by the Free Software Foundation. See [`LICENSE`](LICENSE) for the full text.
