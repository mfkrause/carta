# AGENTS.md — operating rules for carta

carta is a **clean-room reimplementation of pandoc in Rust**. This file is the operating rules.
Read `docs/PORTING.md` for architecture, methodology, and roadmap.

## The rule that overrides everything: clean-room

- **Never read pandoc's source code** (Haskell or otherwise) — not for reference, not to resolve
  ambiguity, not ever.
- **Never copy pandoc's test fixtures** into this repo.
- **Never translate pandoc line-by-line**, and never commit any pandoc-derived code.
- **Allowed sources of truth, in order:** (1) public format specifications (CommonMark, LaTeX,
  reStructuredText, …); (2) pandoc's documented JSON AST contract; (3) observable CLI behavior of
  the pinned pandoc binary — run it, diff the output.
- pandoc is installed black-box in gitignored `.oracle/`. Its outputs are golden values;
  generated fixtures are **never committed**.

Why: carta must be a legally and architecturally independent project. A line-by-line translation
of GPL source is a derivative work, and even reading the source taints the clean-room boundary.

## Source hygiene — no upstream provenance

carta must read as an independent, original implementation. The name "pandoc", the phrases
"reference implementation", "port", "clean-room", "derived from", or any other hint of upstream
provenance may appear **only** in: `AGENTS.md`, `README.md`, `docs/**`, the conformance tooling
(`tools/**`), the vendored-spec attributions (`vendor/**`), and `corpus/README.md`. Every other file —
all product source, Cargo manifests, build and CI config, and the corpus data files themselves
(`corpus/ast/**`, `corpus/text/**`) — must contain none of it: not in identifiers, comments,
doc-comments, or package descriptions. The corpus data files are inputs we author and own, so they
carry no upstream provenance.

This extends past the upstream's *name* to any phrasing that frames the code as matching, imitating,
or being derived from an external implementation — even an unnamed one. In product source, **state
behavior as the code's own design**: assert what the code does, never that it reproduces what some
other tool does. Banned in product source (non-exhaustive): "the reference writer/binary/tool", "the
pinned binary", "the oracle", "matches/to match the reference", "matching the reference X's output",
"derived empirically from …", "verified differentially against …", "observable contract", "a quirk
of the reference X reproduced here". Rewrite each as a plain statement of the rule
(*"a loose list's items are separated with a blank line"*, not *"matching how the reference writer
separates items"*). Where a value or rule looks arbitrary without its rationale, point at `docs/**`
rather than at the upstream tool.

The test is meaning, not substring: ordinary domain vocabulary that happens to reuse these words is
fine — CommonMark's "link reference definition" and "character reference", Rust "references", the
"pinned toolchain" (the Rust toolchain) carry no upstream provenance and stay. Ban the *hint*, not
the word.

- The root AST type is `Document`, never `Pandoc`.
- A few external formats embed the upstream name in their own wire vocabulary. Those literals are
  unavoidable for interoperability and are the **only** sanctioned occurrences in product source.
  Treat each as an opaque external-format token, confined to the single site that emits or parses it
  — never let the name spread beyond these:
  - `pandoc-api-version` — the JSON interchange root key; a single named constant in `carta-ast`.
  - `Pandoc` — the native format's top-level constructor; a parse literal in the native reader.
  - `\pandocbounded` — a LaTeX macro emitted to bound oversized images; a literal in the LaTeX writer.

  Sanctioning a new such literal takes the same justification: it is part of an external format's
  published wire form and cannot be expressed any other way.
- Commit messages are history, not files, but keep them provenance-neutral too.

## Code style

- Idiomatic, safe Rust. **Near-zero `unsafe`**; any `unsafe` needs a `// SAFETY:` comment and a
  real justification.
- Names: complete words, concise, specific — understandable without prior knowledge of the codebase.
- Comments only when the *why* isn't obvious (non-obvious logic, deliberate deviation, unavoidable
  gotcha). Never restate the code; never narrate change history.
- Make invalid states unrepresentable (the Block/Inline split is the canonical example).
- **No panics in shipped paths.** No `.unwrap()`, `.expect()`, `panic!`, `unreachable!`, or slice
  indexing (`xs[i]`) in reader/writer/library code — a converter ingests arbitrary input, so a
  panic is a correctness bug and a DoS. Return `Result` and propagate with `?`; index with `.get()`.
  Lint-enforced (clippy restriction lints); allowed in tests. `todo!("…")` is the one sanctioned
  panic — it marks tracked, unfinished work.
- **Deterministic output.** Output must be byte-reproducible across runs. Use ordered maps for any
  map in the AST or writers — pandoc's `Meta` serializes in sorted-key order, so `BTreeMap` matches
  it; never `HashMap` (its iteration order is randomized and would produce flaky diffs). Verify the
  exact ordering against the pinned binary, don't assume.

## Correctness bar

- **Differential parity with pandoc is the bar**, not "tests pass" — see `docs/PORTING.md` §5, §8–9.
- Adversarial, default-deny verification: assume a finding is *not* a real bug until verified
  against pandoc's actual output (`confirmed=false` by default).
- Track unfinished work as `todo!("…")`; grep them as IOUs before calling a unit done.
- Keep units small enough that a human can actually review them.

## Build & test

Landed so far:

- **Document model + JSON codec** (`carta-ast`) — the Block/Inline model and the JSON interchange
  codec, with the `carta -f json -t json` path.
- **Readers** (`carta-readers`) — CommonMark, HTML, native, and JSON. The CommonMark reader is
  byte-identical to the oracle on all 652 vendored spec examples and honors a low-complexity set of
  extension toggles (`strikeout`, `superscript`, `subscript`, `hard_line_breaks`, `task_lists`,
  `raw_html`); other toggles are not yet wired.
- **Writers** (`carta-writers`) — HTML, LaTeX, reStructuredText, plain, CommonMark, MediaWiki,
  native, and JSON. **Tables render across every writer**, including spans, alignments, fractional
  widths, captions, and multiple bodies; plain/reStructuredText/LaTeX share a text-grid layout
  engine (`grid.rs`).

The offline suite is green and the full conformance suite passes on every surface; product-crate
line coverage is ~93%.

A library facade (`carta`) is the single public entry point — `convert`, `reader_for`/`writer_for`,
`supported_input_formats`/`supported_output_formats`, and the document model re-exported as `ast`.
The CLI is a thin shell over it. Formats are selected at compile time through per-direction features
on the facade (`read-commonmark`, `read-json`, `write-html`, `write-json`; `default = full` enables
all). Each forwards to a per-format feature on the reader/writer crate, so a build can carry a single
direction. A format that is recognized but compiled out is an `Error::FormatNotEnabled`; a genuinely
unknown one is `Error::UnsupportedFormat`. Reader/writer behavior is configured through
`ReaderOptions`/`WriterOptions`, which carry an `Extensions` set (`carta-core`); the CommonMark
reader honors the low-complexity toggles listed above and otherwise defaults to the strict preset.

### Test architecture — four layers

Tests are split so the everyday suite is **fully offline** and oracle-backed parity is a separate,
CI-gated layer. See `docs/PORTING.md` and `docs/plans/refactor-2-testing-architecture.md` for the
full design.

- **Layer 0 — unit tests.** In-crate `#[cfg(test)]` modules over pure helpers and parser internals.
  Offline, fast, edge-focused.
- **Layer 1 — golden snapshots.** `insta` snapshots of carta's **own** output, committed under
  `crates/carta/tests/snapshots/` and reviewed with `cargo insta review`. Readers:
  `corpus/text/<fmt>/*` → snapshot AST JSON. Writers: `corpus/ast/<feature>/*` → snapshot each target
  (minus `corpus/exclusions.tsv`). Plus the relocated offline identity tests (JSON codec in
  `carta-ast`, the native round-trip and spec-parse safety in `carta`). Offline.
- **Layer 2 — conformance suite.** `tools/conformance-suite/run.sh` — shell, **not** part of
  `cargo test`. It runs the built `carta` and the pinned pandoc oracle and diffs them across five
  surfaces (`reader|writer|e2e|roundtrip|commands`) over `corpus/`, the 652 vendored CommonMark spec
  examples, and the fetched pandoc corpus. Requires `.oracle/` and `jq`. CI-gated.
- **Layer 3 — fuzz.** Reader panic-safety (nightly + `cargo-fuzz`), smoke-run in CI.

No committed test data is pandoc output: golden values are carta's own; the corpus under `corpus/`
and the vendored spec under `vendor/` are inputs we own; parity is checked live against the
gitignored oracle, never committed.

### Commands

- Build: `cargo build`
- Build a single direction: `cargo build -p carta --no-default-features --features read-commonmark,write-html`
- Tests (Layers 0, 1, 3 + cli/convert — **fully offline**, no `.oracle/` needed):
  `cargo nextest run --workspace` (doctests separately: `cargo test --doc`)
- Review/accept golden snapshots after an intentional output change: `cargo insta review`
  (never hand-edit `.snap` files).
- Conformance (Layer 2, against pinned pandoc): `tools/conformance-suite/run.sh all`, or one surface
  e.g. `tools/conformance-suite/run.sh writer html`. Each surface prints
  `RESULT <surface> <group> pass=N fail=N err=N skip=N` and exits non-zero on any fail/err.
  **Hard-requires** `.oracle/` and `jq`.
- Benchmark vs pandoc (perf, not correctness; manual, never CI): `tools/bench-suite/run.sh all`, or one
  surface e.g. `tools/bench-suite/run.sh writer latex`. **Hard-requires** `hyperfine`, `jq`, `.oracle/`;
  builds the release binary itself. Machine-specific; nothing committed but `docs/BENCHMARKS.md`.
- Fuzz a reader (nightly + `cargo-fuzz`): `cargo +nightly fuzz run commonmark` (see `fuzz/README.md`)
- Coverage gate (offline product crates, floored at 90%):
  `cargo llvm-cov --workspace --summary-only --fail-under-lines 90` (run
  `cargo llvm-cov clean --workspace` first — stale profraw skews the result).
- Install/pin pandoc: `tools/install-pandoc.sh` (writes to gitignored `.oracle/`, records version)
- Fetch pandoc's test corpus: `tools/fetch-pandoc-tests.sh` (sparse, gitignored, **test files only —
  no source**; see below)
- One-time dev setup (git hooks + tool check): `tools/dev-setup.sh`

Update this section as each piece lands.

## Testing against pandoc's own tests

We reuse pandoc's *test data*, never its test *harness* or implementation. Two layers:

- **Command tests** (`test/command/*.md`) — declarative: a pandoc invocation + input + expected
  output. The conformance suite's `commands` surface parses them and substitutes the `carta` binary
  for `pandoc`, then diffs (skipping and counting tests that use formats/flags carta does not yet
  support). Directly reusable. The format grammar is documented in the corpus's own `README` (public
  test docs, not source).
- **Golden data files** (`*.native`, `*.md`, `*.html`, …) — reused as inputs only. The
  input→expected wiring lives in pandoc's Haskell test modules, which we do **not** read; instead
  the pinned binary regenerates the expected output live. One oracle, no source — nothing committed.

`tools/fetch-pandoc-tests.sh` pulls the corpus **at the git tag matching the pinned binary** (so
embedded golden values are exactly that binary's output — no version-drift false positives), via a
sparse partial checkout of `test/` **with every `.hs` file stripped**. The reader/writer
implementation never lands on disk.

**Clean-room guardrail (hard):** you may read files under the fetched `test/` corpus (`.md`, `.native`,
`.html`, the test `README`, etc.). You must **never** read `*.hs` or anything under pandoc's `src/`,
even if it appears on disk — that is the implementation, and reading it taints the clean room.

## Git guardrails

The repo may be edited by several agents at once. Touch only what you own, and never run a command
that operates on the whole working tree.

- **Stage explicit paths only** — `git add <path> …` for the files you created or changed. Never
  `git add -A`, `git add .`, `git add -u`, or `git commit -a`; they sweep up other agents' work.
- **No bulk or destructive worktree ops.** Never `git reset` (especially `--hard`),
  `git checkout -- .`, `git restore .`, `git clean`, or `git stash` — each can erase uncommitted work
  you didn't create. If a reset/restore is truly unavoidable, scope it to explicit paths you own.
- **No history rewrites on shared branches; never force-push.** Rewriting your own commits on a
  local, unshared branch is allowed only when explicitly asked.
- Branch off `main` for non-trivial changes; don't commit directly to `main`.
- **Conventional Commits, one logical change per commit.** Commit each relevant piece of work as you
  finish it, with a subject of the form `<type>[(scope)][!]: <description>` (types: `feat`, `fix`,
  `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`, plus additional repo-specifics ones called `wip` for WIP and `sec` for security). The `commit-msg`
  hook enforces this.
- Commit freely as you go; **push only when asked.** Never leave uncommitted changes behind.

## Workflows (`.claude/workflows/`)

- **implement-format** — port one reader/writer: implement → differential-verify → fix.
- **differential-verify** — 2-vote + tiebreak adversarial verification against pandoc.
- **conformance-loop** — run pandoc's suite against our binary, dedup divergences, fix-and-review
  until clean.

See `docs/PORTING.md` §7. These run on the harness `Workflow` primitives; orchestration logic stays
in the JS script, not in the model context.
