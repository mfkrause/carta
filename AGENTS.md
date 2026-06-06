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
- pandoc is installed black-box in gitignored `.pandoc-ref/`. Its outputs are golden values;
  generated fixtures are **never committed**.

Why: oxidoc must be a legally and architecturally independent project. A line-by-line translation
of GPL source is a derivative work, and even reading the source taints the clean-room boundary.

## Code style

- Idiomatic, safe Rust. **Near-zero `unsafe`**; any `unsafe` needs a `// SAFETY:` comment and a
  real justification.
- Names: complete words, concise, specific — understandable without prior knowledge of the codebase.
- Comments only when the *why* isn't obvious (non-obvious logic, deliberate deviation, unavoidable
  gotcha). Never restate the code; never narrate change history.
- Make invalid states unrepresentable (the Block/Inline split is the canonical example).

## Correctness bar

- **Differential parity with pandoc is the bar**, not "tests pass" — see `docs/PORTING.md` §5, §8–9.
- Adversarial, default-deny verification: assume a finding is *not* a real bug until verified
  against pandoc's actual output (`confirmed=false` by default).
- Track unfinished work as `todo!("…")`; grep them as IOUs before calling a unit done.
- Keep units small enough that a human can actually review them.

## Build & test (intended interface — not yet implemented; the workspace is scaffolding)

- Build: `cargo build`
- Unit tests: `cargo test`
- Differential tests (against pinned pandoc): `cargo test -p oxidoc-testkit` (requires `.pandoc-ref/`)
- Install/pin pandoc: `tools/install-pandoc.sh` (writes to gitignored `.pandoc-ref/`, records version)

Update this section as each piece lands (starting with slice 0).

## Git guardrails

- Commit with **explicit paths** (`git add <paths>`), never `git add -A` / `git add .`.
- Never `git reset --hard`, `checkout --`, `stash`, `rebase`, or force-push to discard work you
  did not create.
- Branch off `main` for non-trivial changes; don't commit directly to `main`.
- Commit or push only when asked.

## Workflows (`.claude/workflows/`)

- **implement-format** — port one reader/writer: implement → differential-verify → fix.
- **differential-verify** — 2-vote + tiebreak adversarial verification against pandoc.
- **conformance-loop** — run pandoc's suite against our binary, dedup divergences, fix-and-review
  until clean.

See `docs/PORTING.md` §7. These run on the harness `Workflow` primitives; orchestration logic stays
in the JS script, not in the model context.
