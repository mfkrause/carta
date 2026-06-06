# oxidoc — Porting Plan & Methodology

Canonical reference for the project. Compact by design: every agent should read this at the
start of a task. Operating rules (clean-room, code style, git, commands) live in `../AGENTS.md`.

## 1. What this is

oxidoc is a **clean-room reimplementation of pandoc** (Haskell → Rust). Drivers: smaller binary,
better performance, compile-time memory safety, developer experience. It is an **independent
project**. Target scope: **broad coverage** of pandoc's ~30+ readers and writers.

## 2. Non-negotiables

- **Clean-room.** Never read pandoc's source, never copy its fixtures, never translate it
  line-by-line, never commit pandoc-derived code. Work only from public format specs, pandoc's
  documented JSON AST contract, and observable CLI behavior of the pinned pandoc binary. Full
  rule in `../AGENTS.md`.
- **Idiomatic, safe Rust.** Near-zero `unsafe` — this is string/tree manipulation; if you reach
  for `unsafe`, reconsider.
- **Differential correctness.** The oracle is the pandoc binary itself, run black-box. "Tests
  pass" is not the bar; output must match pandoc.

## 3. Architecture (mirror pandoc's M×N)

`readers → Pandoc AST → writers`. The AST is the single contract; readers and writers are
independent and depend only on it. This is what makes the work parallelizable and failures
isolable.

Crate layout (Cargo workspace):

- `oxidoc-ast` — AST types + JSON (de)serialization matching pandoc's `pandoc-api-version`. The
  contract. Pure data.
- `oxidoc-core` — shared options, error type, text/attribute helpers.
- `oxidoc-readers` — one module per input format.
- `oxidoc-writers` — one module per output format.
- `oxidoc-cli` — the `oxidoc` binary; arg parsing + `reader → (filter) → writer` dispatch.
- `oxidoc-testkit` — differential harness (drives pandoc black-box, diffs results).
- later: `oxidoc-templates`, `oxidoc-citeproc`, filter support, etc.

## 4. The AST contract

pandoc emits JSON shaped like
`{ "pandoc-api-version":[..], "meta":{}, "blocks":[...] }`, where nodes are `{"t":Tag,"c":content}`
(some tagless), and `Attr` is `[id, [classes], [[key,val]]]`. **Derive the exact shape and
api-version from `pandoc -t json` of the pinned binary — never from memory.** The Block/Inline
distinction is load-bearing: encode it as two separate Rust enums so invalid nesting (e.g. a block
inside link text) is unrepresentable.

## 5. Two differential oracle surfaces (decouple readers from writers)

- **Reader:** `oxidoc -f X -t json`  vs  `pandoc -f X -t json`  → AST equality.
- **Writer:** feed `pandoc -f json` (pandoc's own AST) into our writer, diff vs `pandoc -t Y`.

A reader bug can't be blamed on a writer, and vice-versa. The clean-room boundary and the
correctness oracle are the *same mechanism* — we never need pandoc's source.

Pinned pandoc lives in gitignored `.pandoc-ref/`; record the exact version (api-version major.minor
must match or pandoc rejects our JSON). Expected outputs are generated at test time into a gitignored
cache keyed by (pandoc version + input hash + args) — **never committed**. CommonMark's own spec
suite (CC-BY-SA) may be vendored with attribution for standard conformance (pandoc-markdown is a
superset, so differential-vs-pandoc is still required).

## 6. Roadmap (vertical slices; tiers ordered by cost)

- **Slice 0 — AST + JSON contract.** Round-trip a corpus of `pandoc -t json` outputs through
  deserialize→serialize losslessly. Gate before anything else.
- **Slice 1 — CommonMark → HTML.** First end-to-end path; proves the pipeline and the harness.
  Gate: CommonMark conformance + differential-vs-pandoc.
- **Tier A — text↔text formats** (markdown variants, HTML, LaTeX, reStructuredText, Org, AsciiDoc,
  Textile, MediaWiki, DocBook, JATS, Typst, plain, native, json). ~80% of real usage, cleanest
  harness, embarrassingly parallel. The bulk of early work.
- **Tier C subset** pulled in as Tier A needs it: templates (standalone output), syntax
  highlighting (code blocks). Heavier Tier C later: citeproc, Lua/JSON filters, PDF engines,
  self-contained media.
- **Tier B — binary/container formats** (docx, odt, pptx, rtf, epub, fb2): ZIP + XML, structural
  (not text) diffing. Separate campaign, last.
- **Finalization.** Rewrite the test suite natively in Rust; remove the local pandoc install.

## 7. Agent workflow templates (`.claude/workflows/`, adapted from the Bun Zig→Rust port)

Built on this harness's `Workflow`/`agent`/`pipeline`/`parallel` primitives. Determinism (paths,
sharding, batching) lives in the JS script, not in agent choices.

- **implement-format** — `pipeline(targets, implement → differential-verify → fix)`, parameterized
  by format + direction. The implement agent reads the *spec* + AST contract (never pandoc source);
  output path computed in JS; per-stage structured-output schemas; only must-fix issues trigger the
  fix stage.
- **differential-verify** — 2 independent verifiers → dedup findings by signature → 3rd-agent
  tiebreak only on disputed findings, **default `confirmed=false`**. Verifiers run our binary vs
  pandoc and report divergences.
- **conformance-loop** — `for round in 1..MAX`: run pandoc's suite + conformance against our binary
  → **dedup divergence signatures** → fix the root cause (never a suppression/early-return) →
  2-vote adversarial review → refix. Exit when clean.

## 8. Verification standards

- Differential first: a unit is correct when it matches pandoc on its fixtures — not when tests are
  green. Passing tests miss error handling, boundaries, and invariants.
- Adversarial, default-deny review (see §7).
- Track unfinished work with `todo!("…")` / explicit markers; grep them as IOUs before declaring a
  unit done.
- Human review is a feature, not friction. The central failure of the Bun port was "nobody read
  it." Keep units small enough to actually read.

## 9. Definition of done (per reader/writer)

1. Differential parity with pandoc on the format's fixture set (and conformance suite where one
   exists).
2. No `unsafe` without a `// SAFETY:` justification (you almost never need any).
3. No outstanding `todo!` / IOUs in shipped paths.
4. Passed the adversarial differential-verify pass.
