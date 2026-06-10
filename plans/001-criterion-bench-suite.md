# Plan 001: Add an offline criterion benchmark suite for readers, writers, and end-to-end convert

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat f5d2e3b..HEAD -- Cargo.toml crates/carta/`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: LOW
- **Depends on**: none
- **Category**: perf
- **Planned at**: commit `5e110f9`, 2026-06-10 (reconciled at `f5d2e3b`, 2026-06-10 — no in-scope code drift; premise prose refreshed after `tools/bench-suite/` landed)

## Why this matters

Performance is the project's first stated goal (README: "Performance and a smaller binary than pandoc"). A comparative CLI-level benchmark suite now exists (`tools/bench-suite/`, hyperfine-driven, times the release binary against the pinned pandoc), but its design record (`docs/plans/benchmark-suite.md`, decision 1) deliberately scoped out an in-process criterion layer — it measures whole-process head-to-head numbers, requires `.oracle/`, and has no pathological inputs. What it cannot do is what plans 003–005 need: isolate per-function hot paths offline, with adversarial inputs (`emphasis_heavy`, `pathological_brackets`), at statistically meaningful per-iteration resolution. This plan adds exactly that missing layer: offline criterion benches in `crates/carta/benches/`, distinct from and complementary to `tools/bench-suite/`. Per the same decision record, these stay out of CI (see Maintenance notes).

## Current state

- `Cargo.toml` (workspace root) — `[workspace.dependencies]` lists `insta = "1"` as the only test-support dependency; no criterion.
- `crates/carta/` — the library facade. Its public entry point (`crates/carta/src/lib.rs:28`):

  ```rust
  pub fn convert(
      from: &str,
      to: &str,
      input: &str,
      reader_options: &ReaderOptions,
      writer_options: &WriterOptions,
  ) -> Result<String> {
  ```

- `crates/carta/Cargo.toml` — has `[dev-dependencies] insta = { workspace = true }`; default feature `full` enables all readers/writers. No `[[bench]]` section.
- `corpus/text/commonmark/*.md` — ten small (~75-byte) feature-focused CommonMark inputs (emphasis.md, lists.md, links-images.md, …). Too small to benchmark directly; useful as building blocks for synthetic inputs.
- `corpus/ast/` — JSON AST fixtures usable as writer-bench inputs via the `json` reader.
- `corpus/bench/seed.md` — the authored ~3 KB strict-CommonMark seed the CLI bench suite repeats to size; usable here as another realistic mixed-feature input.
- `tools/bench-suite/` — the hyperfine CLI comparison suite (shell, oracle-backed, manual). This plan does not touch it; the two suites answer different questions (see Why this matters).
- Tests run with `cargo nextest run --workspace`; benches are not part of that.

Repo conventions that apply:

- **Provenance rule (hard, from `AGENTS.md`)**: the word "pandoc" and any phrasing implying an upstream reference implementation ("the reference", "the oracle", "matching X's output") must NOT appear anywhere in bench code, comments, or Cargo manifests. State behavior as the code's own design.
- **Panic discipline**: clippy `unwrap_used`/`expect_used`/`panic`/`indexing_slicing` warn workspace-wide and `clippy.toml` only relaxes them for `#[cfg(test)]`. Bench files are compiled by `cargo clippy --all-targets`, so the bench file needs a file-level `#![allow(clippy::unwrap_used, clippy::expect_used)]` (benches are not shipped paths).
- **Determinism**: benches must be fully offline — no `.oracle/`, no network.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Build | `cargo build` | exit 0 |
| Tests | `cargo nextest run --workspace` | all pass |
| Lint | `cargo clippy --all-targets` | exit 0, no new warnings |
| Run benches | `cargo bench -p carta` | benches compile and report times |
| Smoke-run benches fast | `cargo bench -p carta -- --quick` | exit 0 |

## Scope

**In scope** (the only files you should modify or create):
- `Cargo.toml` (workspace root) — add `criterion` to `[workspace.dependencies]` only.
- `crates/carta/Cargo.toml` — dev-dependency + `[[bench]]` section.
- `crates/carta/benches/convert.rs` (create)
- `plans/README.md` (status row)

**Out of scope** (do NOT touch):
- Any file under `crates/*/src/` — this plan measures, it does not optimize.
- `tools/conformance-suite/**` — parity testing is a separate layer.
- CI config — wiring benches into CI is deferred (see Maintenance notes).

## Git workflow

- Branch: `advisor/001-criterion-benches` off `main`. Do NOT reuse `feat/bench-suite` — that branch carried the (now landed) hyperfine CLI suite, a different deliverable.
- Conventional Commits, e.g. `test(bench): add criterion suite for readers, writers, and convert`.
- Stage explicit paths only (`git add Cargo.toml crates/carta/Cargo.toml crates/carta/benches/convert.rs`). Never `git add -A`.
- Do NOT push.

## Steps

### Step 1: Add criterion

In root `Cargo.toml` under `[workspace.dependencies]` add:

```toml
criterion = { version = "0.5", default-features = false, features = ["cargo_bench_support"] }
```

In `crates/carta/Cargo.toml` add:

```toml
[dev-dependencies]
criterion = { workspace = true }

[[bench]]
name = "convert"
harness = false
```

(keep the existing `insta` dev-dependency).

**Verify**: `cargo build` → exit 0.

### Step 2: Write synthetic input generators

Create `crates/carta/benches/convert.rs`. Start with a file-level `#![allow(clippy::unwrap_used, clippy::expect_used)]` and a one-line comment that benches are not shipped paths.

Write deterministic generator functions (plain `fn`, no randomness) producing CommonMark inputs of a target byte size by repeating composed building blocks:

- `prose(bytes)` — paragraphs of plain sentences with occasional `*emphasis*`, `**strong**`, and `` `code` ``.
- `links(bytes)` — paragraphs of inline links `[text](http://example.com/path "title")` and reference links with a definitions block at the end.
- `lists(bytes)` — nested bullet/ordered lists three levels deep.
- `emphasis_heavy(bytes)` — many short matched emphasis pairs, e.g. `*a* _b_ **c** ` repeated. This is the regression input for plan 003.
- `pathological_brackets(bytes)` — repeated unmatched `]` and `[` characters (e.g. `"[a]"` openers never resolving plus long runs of `]`); also exercises plan 003's bracket path.

Each generator must return a `String` whose length is within ±10% of the requested size. Use sizes 10 KiB and 1 MiB (constants `SMALL` and `LARGE`).

**Verify**: `cargo clippy --all-targets` → exit 0, no new warnings.

### Step 3: Reader, writer, and end-to-end bench groups

In the same file, three criterion groups, all going through the public facade (`carta::convert`, `carta::reader_for`, `carta::writer_for`):

1. `read_commonmark` — parse each generator's SMALL and LARGE inputs to a `Document` via `reader_for("commonmark")`. Use `Throughput::Bytes(input.len() as u64)`.
2. `write_targets` — build one `Document` by parsing `prose(LARGE)` once outside the timing loop, then render it with each enabled writer: `html`, `plain`, `commonmark`, `rst`, `latex`, `mediawiki`, `native`, `json`.
3. `convert_end_to_end` — `convert("commonmark", "html", …)` on `prose(LARGE)` and `lists(LARGE)`.

Also add a `read_corpus` bench that concatenates all files matching `corpus/text/commonmark/*.md` (read at bench startup via `std::fs`, path relative to `CARGO_MANIFEST_DIR`: `concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/text/commonmark")`) repeated until ≥ 100 KiB, as a realistic mixed-feature input.

Keep the standard criterion main:

```rust
criterion_group!(benches, read_commonmark, write_targets, convert_end_to_end, read_corpus);
criterion_main!(benches);
```

**Verify**: `cargo bench -p carta -- --quick` → exit 0, every group reports a time estimate.

### Step 4: Record the baseline

Run `cargo bench -p carta` (full run) and save criterion's summary output (the `time:` lines per benchmark) into the commit message body or a code-review note — do NOT commit `target/criterion` artifacts. Confirm `target/` is already gitignored (it is).

**Verify**: `git status --porcelain` shows only the three in-scope files (plus `plans/README.md`).

## Test plan

Benches are the deliverable; no unit tests required. Confirm the existing suite is untouched:

- `cargo nextest run --workspace` → all pass, same count as before the change.
- `cargo bench -p carta -- --quick` → exit 0.

## Done criteria

- [ ] `cargo bench -p carta -- --quick` exits 0 and lists all four groups
- [ ] `cargo nextest run --workspace` exits 0
- [ ] `cargo clippy --all-targets` exits 0 with no new warnings
- [ ] `grep -ri pandoc crates/carta/benches/` returns no matches
- [ ] No files outside the in-scope list are modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- `criterion` 0.5 fails to resolve or compile against the pinned toolchain (`rust-toolchain.toml`, Rust 1.93) — report the error rather than downgrading other deps.
- `cargo bench` cannot see corpus files at the computed path (worktree layouts can differ) — report; do not hardcode absolute paths.
- Any existing test fails after the manifest change.
- A bench takes longer than ~60 s for a single iteration set — shrink LARGE rather than letting CI-scale runs explode, and note it.

## Maintenance notes

- Plans 002–005 cite these benches as their verification mechanism; bench names (`read_commonmark`, `emphasis_heavy`, `pathological_brackets`) are referenced there — renaming them breaks those plans.
- Do not wire `cargo bench` into CI: `docs/plans/benchmark-suite.md` (decision 1) records perf-regression-gating in CI as deliberately rejected (shared-runner noise). These benches are a local, on-demand tool, like `tools/bench-suite/`.
- When new readers/writers land, extend `write_targets`/`read_*` rather than creating parallel bench files.
