# Fuzzing

Coverage-guided fuzz targets for oxidoc's readers, built on [`cargo-fuzz`] /
[libFuzzer]. This crate is **detached from the root workspace** (it has its own
`[workspace]` table) because libFuzzer needs a nightly toolchain and a sanitizer,
which the stable-pinned workspace does not use.

## Prerequisites

```sh
rustup toolchain install nightly
cargo install cargo-fuzz
```

## Running

From the repository root:

```sh
cargo +nightly fuzz run commonmark
```

Bound a run by time (as CI does) or by iterations:

```sh
cargo +nightly fuzz run commonmark -- -max_total_time=30
cargo +nightly fuzz run commonmark -- -runs=100000
```

## Targets

One target per reader; each feeds arbitrary UTF-8 into the reader and asserts it
never panics. A reader must treat all input as untrusted, so any panic (including
a slice-index or unwrap) is a bug.

- **`commonmark`** — the CommonMark reader.
- **`json`** — the JSON interchange reader.

Writers are not fuzzed yet: a writer consumes a typed `Document`, not bytes, so a
writer target needs an `arbitrary`-derived AST generator to produce valid-but-
pathological documents (TODO).

A target found nothing if it runs to the time/iteration bound without a crash.
Reproducer inputs and the evolved corpus land in `artifacts/` and `corpus/`,
both gitignored.

[`cargo-fuzz`]: https://github.com/rust-fuzz/cargo-fuzz
[libFuzzer]: https://llvm.org/docs/LibFuzzer.html
