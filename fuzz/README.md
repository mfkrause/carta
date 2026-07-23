# Fuzzing

Coverage-guided fuzz targets for carta's readers and writers, built on [`cargo-fuzz`] / [libFuzzer].

## Prerequisites

```sh
rustup toolchain install nightly
cargo install cargo-fuzz
```

## Running

From the repository root, start a discovery run (random-seeded mutation fuzzing):

```sh
cargo +nightly fuzz run read_commonmark
```

Bound it by time or by iterations:

```sh
cargo +nightly fuzz run read_commonmark -- -max_total_time=30
cargo +nightly fuzz run read_commonmark -- -runs=100000
```

## Targets

One `read_<format>` target per reader (see `Cargo.toml` for the current list); each feeds arbitrary bytes into the reader and asserts it never panics. The unprefixed `highlight` target is the analog for the syntax-highlighting tokenizer, which is not a conversion direction.

One `write_<format>` target per writer module: the whole AST derives [`Arbitrary`], so libFuzzer's byte mutations become structured mutations of a typed `Document`.

## Corpus layout

- `seeds/<target>/`: committed. A small representative input per reader, plus every crash/timeout reproducer we have fixed. Writer targets have no hand-written seeds; libFuzzer grows a corpus from scratch.
- `artifacts/<target>/`: gitignored. Where libFuzzer drops a reproducer when it finds a crash, hang, or OOM.

[`Arbitrary`]: https://docs.rs/arbitrary

When a discovery run finds a bug, fix the reader or writer, then copy the reproducer from `artifacts/<target>/` into `seeds/<target>/` and commit it.

[`cargo-fuzz`]: https://github.com/rust-fuzz/cargo-fuzz
[libFuzzer]: https://llvm.org/docs/LibFuzzer.html
