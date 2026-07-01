# Fuzzing

Coverage-guided fuzz targets for carta's readers, built on [`cargo-fuzz`] /
[libFuzzer]. This crate is **detached from the root workspace** (it has its own
`[workspace]` table) because libFuzzer needs a nightly toolchain and a sanitizer,
which the stable-pinned workspace does not use.

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

Replay only the committed seed corpus — no mutation, so it is deterministic. This is exactly
what the per-PR gate does:

```sh
cargo +nightly fuzz run commonmark fuzz/seeds/commonmark -- -runs=0
```

## Targets

One target per reader (see `Cargo.toml` for the current list); each feeds arbitrary
bytes into the reader and asserts it never panics. A reader must treat all input as
untrusted, so any panic — a slice index, an `unwrap`, unbounded recursion, or a runaway
allocation — is a bug.

Writers are not fuzzed yet: a writer consumes a typed `Document`, not bytes, so a
writer target needs an `arbitrary`-derived AST generator to produce valid-but-
pathological documents (TODO).

## Corpus layout

- **`seeds/<target>/`** — committed. A small representative input per reader, plus every
  crash/timeout reproducer we have fixed. This is the deterministic regression set: it
  seeds discovery and pins fixed bugs so they cannot silently return.
- **`corpus/<target>/`** — gitignored scratch. The evolving, coverage-guided corpus a
  discovery run reads from and writes to.
- **`artifacts/<target>/`** — gitignored. Where libFuzzer drops a reproducer when it finds
  a crash, hang, or OOM.

When a discovery run finds a bug, fix the reader, then copy the reproducer from
`artifacts/<target>/` into `seeds/<target>/` and commit it — the regression replay guards it
from then on.

## Two-tier CI

- **Per PR** (`ci.yml`) — deterministic. Each target is built and replays its committed
  `seeds/` once with no mutation (`-runs=0`). Fast, reproducible, and coupled only to the code
  a PR actually changes, so it can never be reddened by an unrelated latent bug in another
  reader.
- **Nightly** (`fuzz.yml`) — the real hunt. Random-seeded mutation fuzzing on a schedule, with
  the scratch corpus cached so coverage compounds across nights and crash reproducers uploaded
  as artifacts. It is not a merge gate, so a finding never blocks a contributor's PR.

Both pass `-timeout=10 -rss_limit_mb=2048`: `-max_total_time` is only checked between units, so
these bound a single hanging or memory-hungry input instead of letting it stall the runner.

Adding a reader means wiring four places — a target in `fuzz_targets/`, a `[[bin]]` in
`Cargo.toml`, a seed under `seeds/`, and both workflow matrices. The offline test
`crates/carta/tests/fuzz_wiring.rs` asserts they stay in agreement, so a missing piece fails at
PR time with a message naming what to add.

[`cargo-fuzz`]: https://github.com/rust-fuzz/cargo-fuzz
[libFuzzer]: https://llvm.org/docs/LibFuzzer.html
