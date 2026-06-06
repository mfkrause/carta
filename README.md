# 🦀 oxidoc

A reimplementation of [pandoc](https://pandoc.org) in Rust — a fast, small-footprint universal
document converter that reads a markup format and renders it back out in another.

> [!WARNING]
> oxidoc is an **early prototype under active development**. Only a small slice of pandoc's formats
> is implemented today, the API is unstable, and it is not yet ready for production use.

## Goals

- **Performance** and a **smaller binary** than pandoc.
- **Feature parity** with the reference implementation, reached one format at a time.
- A **library first**, with the CLI as a thin shell over it.

## Installation

No releases yet — build from source. Requires Rust 1.93+ (stable).

```sh
cd oxidoc
cargo build --release
# binary at target/release/oxidoc
```

## Usage

### Command line

```sh
# CommonMark to HTML
oxidoc -f commonmark -t html input.md -o output.html

# read from stdin, write to stdout
echo '# Hello' | oxidoc -f commonmark -t html

# inspect the document model
oxidoc -f commonmark -t json input.md
```

### Library

```rust
use oxidoc::{convert, ReaderOptions, WriterOptions};

let html = convert(
    "commonmark",
    "html",
    "# Hello, *world*",
    &ReaderOptions::default(),
    &WriterOptions::default(),
)?;
```

Formats are selected at compile time via per-direction features, so a build can carry only the
directions it needs:

```sh
cargo build -p oxidoc --no-default-features --features read-commonmark,write-html
```

## Development

```sh
cargo build                         # build the workspace
cargo nextest run --workspace       # run tests
cargo clippy --all-targets          # lint
cargo +nightly fuzz run commonmark  # fuzz a reader (see fuzz/README.md)
```

The workspace splits into `oxidoc-ast` (the document model), `oxidoc-core` (shared traits and
options), `oxidoc-readers`, `oxidoc-writers`, the `oxidoc` library facade, and the `oxidoc-cli`
binary. See [`docs/PORTING.md`](docs/PORTING.md) for the architecture and roadmap.

## License

Copyright © 2026 Maximilian Krause.

oxidoc is free software: you can redistribute it and/or modify it under the terms of the GNU Affero
General Public License, version 3, as published by the Free Software Foundation. See
[`LICENSE`](LICENSE) for the full text.
