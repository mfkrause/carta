# AGENTS.md — carta

carta is a **lightweight, performant rewrite of pandoc in Rust**.

## Clean room

- **Never read pandoc's source code**, not for reference, not ever.
- **Never copy pandoc's test fixtures** into this repo.
- **Never refer to "pandoc" or an "oracle" in any source file**, not even in comments or Markdown files (see below).
- **Allowed sources of truth, in order:** (1) public format specifications (CommonMark, LaTeX, reStructuredText, …); (2) pandoc's documented JSON AST contract; (3) observable CLI behavior of the pinned pandoc binary — run it, diff the output.
- pandoc is / can be installed to gitignored `.oracle/`.

## Source hygiene — no upstream provenance

The name "pandoc", the phrases "reference implementation", "port", "clean-room", "derived from", or any other hint of upstream provenance may appear **only** in: `AGENTS.md`, `README.md`, `docs/**`, the conformance tooling (`tools/**`), the vendored-spec attributions (`vendor/**`), and `corpus/README.md`. Every other file — all product source, Cargo manifests, build and CI config, etc. — must contain none of it: not in identifiers, comments, doc-comments, or package descriptions.

This extends past the upstream's *name* to any phrasing that frames the code as matching, imitating, or being derived from an external implementation, even an unnamed one. In product source, **state behavior as the code's own design**: assert what the code does, never that it reproduces what some other tool does. Banned in product source (non-exhaustive):
- "the reference writer/binary/tool"
- "the pinned binary"
- "the oracle"
- "matches/to match the reference"
- "matching the reference X's output"
- "derived empirically from …"
- "verified differentially against …"
- "observable contract"
- "a quirk of the reference X reproduced here"

A few external formats embed the upstream name in their own wire vocabulary. Those literals are unavoidable for interoperability and are the **only** sanctioned occurrences in product source. Treat each as an opaque external-format token, confined to the single site that emits or parses it — never let the name spread beyond these:
- `pandoc-api-version` — the JSON interchange root key; a single named constant in `carta-ast`.
- `Pandoc` — the native format's top-level constructor; a parse literal in the native reader.
- `\pandocbounded` — a LaTeX macro emitted to bound oversized images; a literal in the LaTeX writer.

Keep commit messages absolutely provenance-neutral too.

## Code style

- Idiomatic, safe Rust. **Near-zero `unsafe`**; any `unsafe` needs a `// SAFETY:` comment and a real justification.
- Names: complete words, concise, specific — understandable without prior knowledge of the codebase.
- Comments only when the *why* isn't obvious (non-obvious logic, deliberate deviation, unavoidable gotcha). Never restate the code; never narrate change history.
- Make invalid states unrepresentable.
- **No panics in shipped paths.** No `.unwrap()`, `.expect()`, `panic!`, `unreachable!`, or slice indexing (`xs[i]`) in reader/writer/library code — a converter ingests arbitrary input, so a panic is a correctness bug and a DoS. Return `Result` and propagate with `?`; index with `.get()`. Lint-enforced (clippy restriction lints); allowed in tests.
- **Deterministic output.** Output must be byte-reproducible across runs. Use ordered maps for any map in the AST or writers.

### Test architecture — four layers

Tests are split so the everyday suite is **fully offline** and oracle-backed parity is a separate, CI-gated layer. In the future, once parity has been achieved, we plan to remove the oracle-backed parity tests, so our own offline suite already needs to be made comprehensive.

- **Layer 0 — unit tests.** In-crate `#[cfg(test)]` modules over pure helpers and parser internals.
- **Layer 1 — golden snapshots.** `insta` snapshots of carta's **own** output, committed under `crates/carta/tests/snapshots/` and reviewed with `cargo insta review`. Readers: `corpus/text/<fmt>/*` → snapshot AST JSON. Writers: `corpus/ast/<feature>/*` → snapshot each target (minus `corpus/exclusions.tsv`). Plus the relocated offline identity tests (JSON codec in `carta-ast`, the native round-trip and spec-parse safety in `carta`).
- **Layer 2 — conformance suite.** `tools/conformance-suite/run.sh` — shell, **not** part of `cargo test`. It runs the built `carta` and the pinned pandoc oracle and diffs them across five surfaces (`reader|writer|e2e|roundtrip|commands`) over `corpus/`, the 652 vendored CommonMark spec examples, and the fetched pandoc corpus. Requires `.oracle/` and `jq`.
- **Layer 3 — fuzz.** Reader panic-safety (nightly + `cargo-fuzz`).

### Commands

- Build: `cargo build`
- Build a single direction: `cargo build -p carta --no-default-features --features read-commonmark,write-html`
- Tests (Layers 0, 1, 3 + cli/convert — **fully offline**, no `.oracle/` needed): `cargo nextest run --workspace` (doctests separately: `cargo test --doc`)
- Review/accept golden snapshots after an intentional output change: `cargo insta review` (**never** hand-edit `.snap` files).
- Conformance (Layer 2, against pinned pandoc): `tools/conformance-suite/run.sh all`, or one surface e.g. `tools/conformance-suite/run.sh writer html`. Each surface prints `RESULT <surface> <group> pass=N fail=N err=N skip=N` and exits non-zero on any fail/err. Requires `.oracle/` and `jq`.
- Benchmark vs pandoc (perf, not correctness; manual, never CI): `tools/bench-suite/run.sh all`, or one surface e.g. `tools/bench-suite/run.sh writer latex`. Requires `hyperfine`, `jq`, `.oracle/`; builds the release binary itself.
- Fuzz a reader (nightly + `cargo-fuzz`): `cargo +nightly fuzz run commonmark` (see `fuzz/README.md`)
- Coverage gate (offline product crates, floored at 90%): `cargo llvm-cov --workspace --summary-only --fail-under-lines 90` (run `cargo llvm-cov clean --workspace` first — stale profraw skews the result).
- Install/pin pandoc: `tools/install-pandoc.sh` (writes to gitignored `.oracle/`, records version)
- Fetch pandoc's test corpus: `tools/fetch-pandoc-tests.sh` (sparse, gitignored, **test files only — no source**; see below)
- One-time dev setup (git hooks + tool check): `tools/dev-setup.sh`

## Status docs

Two files track feature parity and must stay in sync with the code:

- `README.md` — the `## Status` table (format-level reader/writer support).
- `docs/STATUS.md` — per-format detail: extensions honored, known gaps, and the parity backlog.

Whenever you implement, extend, or change support for a format, extension, or cross-cutting feature (standalone output, TOC, citations, …), update **both** files in the same change.

## Git guardrails

The repo may be edited by several agents at once. Touch only what you own, and never run a command that operates on the whole working tree.

- **Stage explicit paths only** — `git add <path> …` for the files you created or changed. Never `git add -A`, `git add .`, `git add -u`, or `git commit -a`; they sweep up other agents' work.
- **No bulk or destructive worktree ops.** Never `git reset` (especially `--hard`), `git checkout -- .`, `git restore .`, `git clean`, or `git stash` — each can erase uncommitted work you didn't create. If a reset/restore is truly unavoidable, scope it to explicit paths you own.
- **No history rewrites on shared branches; never force-push.** Rewriting your own commits on a local, unshared branch is allowed only when explicitly asked.
- Branch off `main` for non-trivial changes; don't commit directly to `main`.
- **Conventional Commits, one logical change per commit.** Commit each relevant piece of work as you finish it, with a subject of the form `<type>[(scope)][!]: <description>` (types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`, plus additional repo-specifics ones called `wip` for WIP and `sec` for security). The `commit-msg` hook enforces this.
- Commit freely as you go; **push only when asked.**
