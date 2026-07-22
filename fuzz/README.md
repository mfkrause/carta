# Fuzzing

Coverage-guided fuzz targets for carta's readers, built on [`cargo-fuzz`] / [libFuzzer].

## Prerequisites

```sh
rustup toolchain install nightly
cargo install cargo-fuzz
```

## Running

From the repository root, start a discovery run (random-seeded mutation fuzzing):

```sh
cargo +nightly fuzz run commonmark
```

Bound it by time or by iterations:

```sh
cargo +nightly fuzz run commonmark -- -max_total_time=30
cargo +nightly fuzz run commonmark -- -runs=100000
```

## Targets

One target per reader (see `Cargo.toml` for the current list); each feeds arbitrary bytes into the reader and asserts it never panics.

Writers are not fuzzed yet: a writer consumes a typed `Document`, not bytes, so a writer target needs an `arbitrary`-derived AST generator to produce valid-but-pathological documents (TODO).

## Corpus layout

- **`seeds/<target>/`** — committed. A small representative input per reader, plus every crash/timeout reproducer we have fixed.
- **`artifacts/<target>/`** — gitignored. Where libFuzzer drops a reproducer when it finds a crash, hang, or OOM.

When a discovery run finds a bug, fix the reader, then copy the reproducer from `artifacts/<target>/` into `seeds/<target>/` and commit it.

[`cargo-fuzz`]: https://github.com/rust-fuzz/cargo-fuzz
[libFuzzer]: https://llvm.org/docs/LibFuzzer.html
