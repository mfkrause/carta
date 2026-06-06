# AGENTS.md — operating rules for oxidoc

oxidoc is a **clean-room reimplementation of pandoc in Rust**. This file is the operating rules.
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

Why: oxidoc must be a legally and architecturally independent project. A line-by-line translation
of GPL source is a derivative work, and even reading the source taints the clean-room boundary.

## Source hygiene — no upstream provenance

oxidoc must read as an independent, original implementation. The name "pandoc", the phrases
"reference implementation", "port", "clean-room", "derived from", or any other hint of upstream
provenance may appear **only** in: `AGENTS.md`, `README.md`, `docs/**`, and the testing toolkit
(`crates/oxidoc-testkit/**` and `tools/**`). Every other file — all product source, Cargo manifests,
build and CI config — must contain none of it: not in identifiers, comments, doc-comments, or
package descriptions.

- The root AST type is `Document`, never `Pandoc`.
- The JSON interchange format requires a literal `pandoc-api-version` key. Confine that one string
  to a single named constant in `oxidoc-ast` and treat it as an opaque external protocol identifier
  — it is the lone unavoidable occurrence; do not let the name spread.
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

Slices 0 and 1 have landed:

- **Slice 0** — the document model and JSON interchange codec (`oxidoc-ast`), and the
  `oxidoc -f json -t json` conversion path.
- **Slice 1** — the `CommonMark` reader (`oxidoc-readers`) and HTML writer (`oxidoc-writers`),
  exposing `oxidoc -f commonmark -t html` (and `-t json` from CommonMark, `-f json -t html`).
  Byte-identical to the pinned binary on all 652 vendored CommonMark spec examples; ~96%
  product-crate line coverage.

Other formats are a recognized-but-unsupported error.

- Build: `cargo build`
- Unit + integration tests: `cargo nextest run --workspace` (doctests separately: `cargo test --doc`)
- Differential tests (against pinned pandoc): `cargo nextest run -p oxidoc-testkit`. These
  **hard-require** `.oracle/` (binary + corpus) — they fail, not skip, if it is absent. The
  committed offline fixtures under `crates/oxidoc-testkit/fixtures/roundtrip/` round-trip without
  any oracle. Surfaces: reader (CommonMark→JSON), writer (AST→HTML across the full model), and
  end-to-end (CommonMark→HTML); the writer-parity suite lives in `oxidoc-testkit/tests/writer.rs`.
- Spec-parity report: `cargo run -p oxidoc-testkit --bin spec_report -- --surface=e2e` (default
  surface is reader→JSON; `--show=N` prints the first N divergences).
- Product-only coverage: `cargo llvm-cov --workspace --ignore-filename-regex 'oxidoc-testkit'
  --summary-only` (the testkit is the harness, excluded from the denominator).
- Install/pin pandoc: `tools/install-pandoc.sh` (writes to gitignored `.oracle/`, records version)
- Fetch pandoc's test corpus: `tools/fetch-pandoc-tests.sh` (sparse, gitignored, **test files only —
  no source**; see below)
- One-time dev setup (git hooks + tool check): `tools/dev-setup.sh`

Update this section as each piece lands.

## Testing against pandoc's own tests

We reuse pandoc's *test data*, never its test *harness* or implementation. Two layers:

- **Command tests** (`test/command/*.md`) — declarative: a pandoc invocation + input + expected
  output. Our runner parses them and substitutes the `oxidoc` binary for `pandoc`, then diffs.
  Directly reusable. The format grammar is documented in the corpus's own `README` (public test
  docs, not source).
- **Golden data files** (`*.native`, `*.md`, `*.html`, …) — reused as inputs only. The
  input→expected wiring lives in pandoc's Haskell test modules, which we do **not** read; instead
  the pinned binary regenerates the expected output. One oracle, no source.

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
