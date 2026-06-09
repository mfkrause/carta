# Refactor 2 — Testing architecture overhaul

Status: **planned** (not started). Delivery: **one PR** off `main`.

This plan is standalone. It assumes no prior context beyond the repo and `AGENTS.md`
(`.claude/CLAUDE.md`). Read `AGENTS.md` first — the clean-room and source-hygiene rules below are
load-bearing.

---

## 1. Motivation

The current test setup has two structural problems:

1. **Oracle tests live inside `cargo test`.** `crates/carta-testkit` is a workspace member, and all
   its differential `#[test]`s (CommonMark spec parity, per-writer parity, corpus round-trip) run in
   the default `cargo nextest run --workspace`. So the regular suite **hard-requires** the gitignored
   `.oracle/` pandoc binary. A contributor without the oracle cannot run the suite; CI must provision
   pandoc for the unit-test job.
2. **Coverage gaps hide in the oracle layer.** Pure helpers (width measurement, escaping, entity
   decoding, list-marker logic, the `Dimension` parser, the line-fill engine) have **no direct unit
   tests** — only indirect end-to-end coverage. This is exactly how a batch of 12 writer-output bugs
   stayed latent until a cross-writer corpus surfaced them: the CommonMark reader cannot produce
   certain AST shapes (alpha/roman lists), so the writers were never exercised on them.

The goal: **cleanly separate oracle-backed conformance from our own self-contained test suite**, and
**fill the coverage gaps once**, with a durable structure that future format work plugs into.

---

## 2. Licensing finding (why we author our own fixtures)

pandoc is **GPL-2.0-or-later**; carta is **AGPL-3.0-only**.

**Could copying pandoc's test fixtures relicense our code? No — risk to our source is LOW.** Two
independent reasons, grounded in the license texts and FSF/CC guidance:

- **Code ≠ data.** Copyright on a GPL *program* covers its code, not data it processes. A harness
  that *reads* a fixture file at runtime is not a derivative of that file. (FSF GPL FAQ: program
  output / data is not covered by the program's license.)
- **Mere aggregation** (GPLv2 §2 final paragraph; GPLv3 §5) explicitly exempts independent works
  shipped in the same tree. Copyleft, if it reaches anything, reaches **only the copied fixture
  files** — never the harness or product source.
- The CommonMark spec's examples are **CC-BY-SA-4.0**; ShareAlike binds *adaptations* of the data,
  not unrelated code in the same repo. The vendored `spec.txt` (attributed) does not touch our Rust
  license. This is already correct in the repo today.

**But we still do not copy pandoc's fixtures**, for non-copyright reasons:
1. The repo's own constitution forbids it (`AGENTS.md`: "Never copy pandoc's test fixtures").
2. Redistributing pandoc's hand-authored fixtures keeps *those files* GPL, with notice/redistribution
   obligations — a compliance surface on an AGPL repo.
3. It pollutes the clean-room independence narrative.

Note pandoc-*generated* golden outputs are not even GPL (program output is not covered by the
program's license) — which is why the existing "regenerate from the pinned binary, never commit"
posture is the clean path, and why our committed golden values will be **carta's own output**, not
pandoc's.

**Conclusion: author our own fixtures; the oracle is consulted live and never committed.**

---

## 3. Locked decisions (the design tree)

These were resolved up front; the rest of the plan implements them. Do not relitigate without cause.

1. **Writer test corpus = hand-authored AST-JSON.** A committed full-model AST-JSON corpus drives all
   writer tests in both layers. Layer 1 snapshots `carta -f json -t TARGET` (offline, decoupled from
   any reader). Layer 2 compares `carta -f json -t TARGET` vs `pandoc -f json -t TARGET` (pandoc
   reads JSON natively — verified). One corpus, every writer; can express AST shapes no reader
   produces (table spans).
2. **Reader test corpus = small authored per-format text corpus.** Layer 1 snapshots its AST offline;
   Layer 2 runs it plus the full 652 CommonMark spec examples vs pandoc. The 652 spec examples stay
   **Layer-2-only** (not committed as 652 `.snap` files).
3. **Corpus layout: repo-root `corpus/`, subdirectory = feature/format.** `corpus/ast/<feature>/…`
   and `corpus/text/<format>/…`; the subdir *is* the feature/format, so no per-case manifest. One
   `corpus/exclusions.tsv` (`target<TAB>feature`) lists unimplemented writer features, read by both
   the Rust tests and the shell suite. Reachable from crates (`../../corpus`) and `tools/`.
4. **Golden (Layer 1) tests live in the facade crate; `carta-testkit` is deleted.** Golden insta
   tests in `crates/carta/tests/`; offline tests scatter to natural homes (codec → `carta-ast`,
   native-pair → `carta`). No extra crate.
5. **Layer 2 conformance suite = shell scripts** under `tools/conformance-suite/`. It orchestrates
   `carta` and `pandoc` and diffs: `jq -S` for JSON surfaces, byte-diff for text surfaces. The only
   new dependency is `jq`. Not Rust — the tool is orchestration + diff, lives in `tools/` (sanctioned
   for provenance vocabulary), and tests the actual built binary.
6. **All five surfaces ship now**, including the declarative **command-tests** runner (with a
   skip/report policy for tests using flags/extensions carta does not yet support).
7. **CI gates conformance as a required PR job.** Cached `.oracle/` provisioning stays; the suite runs
   on every PR and blocks.
8. **Coverage: gate at a 90% floor**, product crates only, measured from **offline Rust tests alone**
   (`cargo llvm-cov` cannot see the shell-spawned binary). This sets the real bar for corpus
   comprehensiveness — the committed offline suite must clear 90% without the oracle.
9. **Layer 0 unit tests cover helpers *and* parser internals** (broad), backed by the coverage gate.
10. **Golden assertion tool = `insta`** (file snapshots, committed, reviewed via `cargo insta`).
11. **`jq` accepted**; port the Python spec-extraction to `awk` to drop the Python dependency.

---

## 4. Target architecture — four layers

```
Layer 0  Unit tests          in-crate #[cfg(test)], OFFLINE
         pure helpers + parser internals — fast, precise, edge-focused

Layer 1  Golden (insta)       committed snapshots of carta's OWN output, OFFLINE
         readers: corpus/text/<fmt>/* → snapshot AST JSON
         writers: corpus/ast/<feature>/* → snapshot each TARGET (minus exclusions)
         + relocated offline identity tests (codec, native pair, spec-parse safety)

Layer 2  Conformance suite    tools/conformance-suite/*.sh — carta vs pandoc
         surfaces reader|writer|e2e|roundtrip|commands
         over corpus/ + 652 CommonMark spec + fetched pandoc corpus + command tests
         NOT in cargo test; CI provisions .oracle and runs it (required)

Layer 3  Fuzz                 unchanged — reader panic-safety (nightly, smoke in CI)
```

**Division of labor.** Layer 2 proves *carta == pandoc* (correctness vs the oracle). Layer 1 proves
*carta == its own frozen output* (regression + works with no pandoc). Layer 0 proves the *units* are
right with focused assertions. No committed test data is pandoc-derived: golden values are carta's
own; parity is checked live against the gitignored oracle.

After this refactor, **`cargo nextest run --workspace` is fully offline.**

---

## 5. The shared corpus (`corpus/` at repo root)

```
corpus/
  README.md                      # what this is, how both layers consume it, how to add a case
  exclusions.tsv                 # target<TAB>feature — writer features not yet implemented
  ast/                           # drives WRITER tests (both layers)
    common/<label>.json          # nodes every writer renders
    table/<label>.json
    figure/<label>.json
    math/<label>.json
    image-dimensions/<label>.json
  text/                          # drives READER tests (both layers)
    commonmark/<label>.md
    html/<label>.html
    native/<label>.native
    json/<label>.json
vendor/
  commonmark/                    # relocated from crates/carta-testkit/vendor/commonmark/
    spec.txt  ATTRIBUTION.md  LICENSE  VERSION
```

### 5.1 AST-JSON corpus (`corpus/ast/<feature>/`)

Each file is a complete Document JSON (the `pandoc-api-version`/`meta`/`blocks` envelope — see the
seed files at `crates/carta-testkit/fixtures/roundtrip/{blocks,inlines,table,meta}.json`). The
**subdirectory is the feature tag**; the filename stem is the label.

Features (mirror the existing `Feature` enum in the WIP `cases.rs`):
`common`, `table`, `figure`, `math`, `image-dimensions`.

**Coverage requirement (this is the bar that makes the 90% offline floor reachable).** The `ast/`
corpus must exercise **every** Block and Inline variant and their attribute permutations:

- Blocks: `Plain`, `Para`, `LineBlock`, `CodeBlock` (with/without lang/id/numberLines/attrs),
  `RawBlock` (html, latex, other), `BlockQuote`, `OrderedList` (every `ListNumberStyle` ×
  `ListNumberDelim`, non-1 start), `BulletList` (tight/loose, nested), `DefinitionList`
  (tight/loose, multi-def), `Header` (levels 1–6, attrs, unnumbered), `HorizontalRule`, `Table`
  (alignments, col widths, caption, header/body/foot, colspan/rowspan), `Figure` (captioned,
  no-alt, with-dims), `Div` (with id/classes/attrs, admonition-style).
- Inlines: `Str`, `Emph`, `Strong`, `Strikeout`, `Superscript`, `Subscript`, `SmallCaps`,
  `Underline`, `Quoted` (single/double), `Cite`, `Code` (with backticks/specials/attrs), `Space`,
  `SoftBreak`, `LineBreak`, `Math` (inline/display), `RawInline` (html, latex, other), `Link`
  (plain, titled, attrs, autolink, uri/email class), `Image` (alt, title, width/height),
  `Note` (single-para, multi-para, code-block body), `Span` (id/classes/data attrs).
- Edge cases that previously hid bugs: non-decimal/two-paren ordered lists, footnote whose body is a
  code block, anchored links, literal C0 control characters, wide/combining characters for width.

Use the WIP `crates/carta-testkit/src/cases.rs` (on the `test/shared-writer-corpus` branch, committed
as `wip(testkit): …`) as the **content seed** — it already enumerates ~73 labelled cases as markdown
snippets. Convert each to an AST-JSON file by minting once with the oracle during authoring
(`pandoc -f markdown -t json`), then **hand-verify and commit the JSON** (the committed artifact is
carta-independent data we own; we are not committing pandoc *output as golden* — these are *inputs*).
For cases that markdown cannot express (table spans), author the JSON directly or mint from the raw
HTML the seed used.

### 5.2 Text corpus (`corpus/text/<format>/`)

Small, hand-authored source documents, one construct family per file, per reader format
(`commonmark`, `html`, `native`, `json`). Concise — these drive reader snapshots and reader/e2e
conformance. They do **not** need to be exhaustive (the 652 CommonMark spec examples carry exhaustive
reader conformance in Layer 2); they need to cover each construct once for the offline regression net.

### 5.3 `corpus/exclusions.tsv`

One `target<TAB>feature` line per unimplemented writer feature. Derive from the `todo!()` sites and
**verify each line corresponds to an actual error** (feed a fixture of that feature to that writer and
confirm it errors). Current `todo!()` sites (`grep -rn 'todo!' crates/carta-writers/src/`):

```
commonmark	figure
commonmark	table
commonmark	math
commonmark	image-dimensions
latex	table
rst	table
plain	table
plain	math
```

`html`, `json`, `native`, `mediawiki` have no exclusions. Both the Rust golden-writer test and the
shell writer/e2e surfaces read this file and skip the listed `(target, feature)` pairs, **reporting
the skip count** (never silent). When a `todo!()` is implemented later, delete its line and the
corpus case activates automatically.

---

## 6. Layer 2 — the conformance suite (`tools/conformance-suite/`)

Pure shell. `tools/` is sanctioned for provenance vocabulary, so "pandoc"/"oracle"/"reference" are
fine here. Layout:

```
tools/conformance-suite/
  run.sh             # dispatcher: run.sh <surface> [target|format] ;  run.sh all
  lib.sh             # shared: path discovery, normalization, compare fns, spec extraction, reporting
  surfaces/
    reader.sh        # text → JSON AST, jq-compare vs pandoc
    writer.sh        # AST-JSON → target, byte/jq compare vs pandoc
    e2e.sh           # text → target, byte/jq compare vs pandoc
    roundtrip.sh     # fetched .native corpus → JSON codec identity vs pandoc
    commands.sh      # declarative command tests
  README.md
```

### 6.1 Shared primitives (`lib.sh`)

Port these exactly from the current `crates/carta-testkit/src/differential.rs` (read it; do not
guess):

- **Paths.** `ROOT` = repo root. `ORACLE="$ROOT/.oracle/bin/pandoc"`. `OX="$ROOT/target/debug/carta"`
  (build first: `cargo build -p carta`). `SPEC="$ROOT/vendor/commonmark/spec.txt"`. Fetched corpus:
  `$ROOT/.oracle/tests/test`. Fail loudly with provisioning instructions if `.oracle` is missing.
- **Normalization** (`oracle_normalization(to)`): `html|html5` → `--syntax-highlighting=none
  --mathjax`; `latex` → `--syntax-highlighting=none`; everything else → none. Apply to the **pandoc**
  side only.
- **JSON compare** (reader, roundtrip, and the `json` target): canonicalize both with `jq -S .` and
  `diff`. A non-empty diff is a mismatch; print the diff as the divergence report.
- **Text compare** (all non-`json` targets): strip a single trailing newline from both sides
  (pandoc and the carta CLI each append one), then byte-compare. On mismatch, report the first
  differing line.
- **Oracle-rejected**: if pandoc exits non-zero on an input, that case is **not counted against
  carta** (it is `skip`, surfaced separately) — matches the existing `Diff::OracleRejected`.
- **Reporting**: each surface prints `RESULT <surface> <fmt> pass=N fail=N err=N skip=N` and writes a
  full per-case diff dump under a work dir (current sweep uses `/tmp/conf/`). `run.sh` aggregates and
  exits non-zero if any `fail>0` or `err>0`.

### 6.2 Surfaces

- **reader** (`-f FMT -t json`): for each `corpus/text/<fmt>/*` and each of the 652 spec examples
  (commonmark), compare `carta -f fmt -t json` vs `pandoc -f fmt -t json` via jq.
- **writer** (`-f json -t TARGET`): for each `corpus/ast/<feature>/*` not excluded for TARGET,
  compare `carta -f json -t TARGET` vs `pandoc -f json -t TARGET <norm>`. (This replaces the old
  AST-pinned-via-markdown minting — the corpus *is* the AST.)
- **e2e** (`-f FMT -t TARGET`): for `corpus/text/*` and the spec examples, compare full pipelines.
- **roundtrip** (JSON codec identity over the fetched `.native` corpus): golden = `pandoc -f native
  -t json`; actual = `carta -f json -t json` fed that golden; jq-compare. (Tests carta's JSON codec
  over realistic ASTs. Mirror the current `roundtrip.rs` mint-and-compare, including its version-keyed
  cache if you want the speed; the cache is optional.)
- **commands**: see §6.3.

Targets to sweep: writers `html latex rst plain commonmark mediawiki native json`; readers
`commonmark html native json`. `run.sh all` runs every surface across every implemented direction.

### 6.3 Command tests (implement now)

pandoc's declarative command tests live at `.oracle/tests/test/command/*.md` (fetched, gitignored).
**Read the grammar from the corpus's own `README` in the fetched `test/` tree — public test docs, not
source. Never read `*.hs`.** The known shape: a fenced block containing a `% pandoc <args>` command
line, the stdin input, a `^D` separator line, then the expected stdout. The runner:

1. Parse each block (awk): extract args, input, expected.
2. Substitute the `carta` binary for `pandoc`; translate/forward the args carta understands.
3. Run, compare stdout to expected (text or jq per target).
4. **Skip policy**: a test whose command uses a format carta does not implement, or an extension/flag
   carta does not support, is **skipped and counted** (`skip=N`), not failed. Maintain an allowlist
   of supported flags/formats; everything outside it is skipped with a reason. This keeps unimplemented
   features from drowning the signal while still reporting how much is skipped.

This surface will have a large `skip` count initially (most command tests exercise unported
formats/extensions) — that is expected and must be reported, not hidden.

---

## 7. Layer 1 — golden tests (`insta`, in the facade crate)

Add `insta` as a dev-dependency of `crates/carta`. Snapshots are committed under
`crates/carta/tests/snapshots/` and reviewed with `cargo insta review`. Initial snapshots are
generated from current carta output and **human-reviewed on creation** (they bake in current
behavior; Layer 2 guards correctness against the oracle).

```
crates/carta/tests/
  common/mod.rs          # corpus discovery (walk corpus/), exclusions.tsv parsing, convert helper
  golden_reader.rs       # for each corpus/text/<fmt>/*: snapshot carta -f fmt -t json
  golden_writer.rs       # for each corpus/ast/<feature>/* × each target (minus exclusions):
                         #   snapshot carta -f json -t target
  native_roundtrip.rs    # MOVED from carta-testkit/tests/ (Rust-authored Document identity)
  spec_parse.rs          # parse all 652 vendored spec examples, assert Ok (offline panic-safety)
  snapshots/
```

- Corpus path resolves via `env!("CARGO_MANIFEST_DIR")` + `../../corpus` and `../../vendor`.
- The golden tests drive carta through the facade in-process (`carta::convert` /
  `reader_for` / `writer_for`), no subprocess.
- Snapshot naming: include format/target + label so a failure points to the exact case.
- These run in **every** `cargo nextest` invocation (including the `minimal` CI job and contributor
  machines) — fully offline.

---

## 8. Layer 0 — unit tests (helpers + parser internals)

In-crate `#[cfg(test)]` modules. Broad scope. Minimum checklist (add more as coverage demands):

**`carta-writers/src/common.rs`**: `display_width`/`char_width`/`is_zero_width` (ASCII, CJK wide,
combining marks, zero-width joiners, C0 controls → width 0); `escape_xml`/`escape_attr` (quotes
on/off, the special set); `is_percent_escaped_uri`; `fill`/`fill_offset` (wrap boundary, initial
offset, words longer than the column).

**`carta-writers/src/latex.rs`**: the `Dimension` parser (px/bare→inches at 96dpi, `%`→fraction,
in/cm/mm/pt/pc/em verbatim, unknown units ignored, width-only/height-only/both).

**`carta-writers/src/commonmark.rs`**: list-marker downgrade (every non-decimal style → decimal;
two-paren → one-paren; start preserved); autolink-class detection.

**`carta-readers/src/entities.rs`**: `code_point` (0 and out-of-Unicode → U+FFFD); `lookup_named`
(hit, miss, the binary-search boundaries).

**`carta-readers/src/commonmark/` submodules** and **`carta-readers/src/html.rs`**: list-marker /
ordered-start parsing, blockquote/indent continuation, HTML block categorization (CDATA, comment,
PI, declaration, tag-name matching), link-reference-definition normalization.

The 90% coverage gate (offline) is the backstop: if a product line is uncovered after Layers 0–1, add
a unit test or a corpus case.

---

## 9. Dissolving `carta-testkit`

Delete the crate. Redistribute its pieces:

| Current (`crates/carta-testkit/…`) | New home |
|---|---|
| `src/differential.rs` (reader/writer/e2e surfaces) | `tools/conformance-suite/` (shell) |
| `src/roundtrip.rs` (mint + JSON identity) | `tools/conformance-suite/surfaces/roundtrip.sh` |
| `src/command_test.rs` (stub) | `tools/conformance-suite/surfaces/commands.sh` (implemented) |
| `src/commonmark_spec.rs` (spec extractor) | `tools/conformance-suite/lib.sh` (awk) + `spec_parse.rs` (Rust, offline) |
| `src/bin/spec_report.rs` | folded into `run.sh` reporting |
| `src/lib.rs` path helpers | `lib.sh` path discovery |
| `tests/commonmark.rs` (oracle) | Layer 2 (reader + e2e surfaces) |
| `tests/{writer,latex_writer,rst_writer,plain_writer,native_writer,html_reader,native_reader}.rs` | Layer 2 (writer/reader surfaces) + Layer 1 golden |
| `tests/roundtrip.rs` (oracle) | Layer 2 (roundtrip surface) |
| `tests/fixtures.rs` (offline JSON codec round-trip) | `crates/carta-ast/tests/codec_roundtrip.rs`, reading `corpus/ast/**` |
| `tests/native_roundtrip.rs` (offline, Rust-authored) | `crates/carta/tests/native_roundtrip.rs` |
| `fixtures/roundtrip/*.json` | seed → `corpus/ast/common/` (expand to full model) |
| `vendor/commonmark/` | `vendor/commonmark/` at repo root |
| WIP `src/cases.rs` (`test/shared-writer-corpus` branch) | content seed → `corpus/ast/` + `exclusions.tsv` |

Remove `carta-testkit` from anywhere it is referenced (it is matched by `members = ["crates/*"]`, so
deleting the directory suffices for the workspace, but grep for the name across the repo).

---

## 10. CI rewrite (`.github/workflows/ci.yml`)

Current `check` job provisions the oracle and runs `cargo nextest run --workspace --all-features`
(oracle-coupled). Target shape:

- **`check` (offline)** — drop oracle provisioning. `cargo fmt --all --check`; `cargo clippy
  --all-targets --all-features`; `cargo nextest run --workspace --all-features` (now fully offline:
  Layers 0, 1, 3 + cli/convert); `cargo test --doc --workspace --all-features`.
- **`conformance` (oracle, NEW, required)** — keep the `.oracle` cache (`key:
  oracle-pandoc-${PANDOC_PIN}`) and `tools/install-pandoc.sh --version="$PANDOC_PIN" &&
  tools/fetch-pandoc-tests.sh`; install `jq`; `cargo build -p carta`; run
  `tools/conformance-suite/run.sh all`. Non-zero exit fails the job.
- **`coverage` (offline, NEW, required)** — `taiki-e/install-action` `cargo-llvm-cov`; run
  `cargo llvm-cov --workspace --summary-only --fail-under-lines 90` (product crates; nothing to
  exclude once testkit is gone — confirm the denominator is product `src/` only, add
  `--ignore-filename-regex` if any test-support code leaks in).
- **`minimal`, `fuzz-smoke`, `typos`, `deny`** — unchanged.

All jobs except `conformance` run without `.oracle/`.

---

## 11. Update the agent workflows and docs

- **`.claude/workflows/conformance-sweep.sh`** — delete; superseded by `tools/conformance-suite/`.
- **`.claude/workflows/conformance-loop.mjs`** — repoint `SWEEP` (line ~38) to
  `tools/conformance-suite/run.sh writer` (and/or the relevant surface). Verify the `RESULT …
  pass=N fail=N err=N` parsing still matches (keep the same line format, now with `skip=N`).
- **`.claude/workflows/implement-format.mjs`** — repoint every `crates/carta-testkit/…` path
  (lines ~77, 130, 350, 439, 444, 490, 589, 596, 631, 721, 872). The **Wire-Up step** (the prior
  outstanding request) must instruct the agent to, for a new format: (a) add `corpus/text/<fmt>/`
  and/or `corpus/ast/<feature>/` cases; (b) update `corpus/exclusions.tsv` as `todo!()`s are filled;
  (c) add/refresh golden snapshots (`cargo insta review`); (d) run
  `tools/conformance-suite/run.sh <surface> <fmt>`. Vendored specs now go under repo-root `vendor/`.
- **`AGENTS.md` (`.claude/CLAUDE.md`) "Build & test"** — rewrite to describe the four layers,
  `cargo nextest run --workspace` being offline, `tools/conformance-suite/run.sh`, the coverage gate,
  and the new `corpus/` + `vendor/` locations. Update the source-hygiene sanctioned-location list
  (`crates/carta-testkit/**` → remove; `tools/**` already covers the suite; add `corpus/**` and
  note its data is our own).
- **`docs/PORTING.md`** — update §5/§7/§8–9 references to the testkit and sweep.
- **`tools/dev-setup.sh`** — add availability checks for `jq`, `cargo-insta`, `cargo-llvm-cov`.

---

## 12. Delivery — one PR, ordered commits

Single branch off `main` (e.g. `refactor/testing-architecture`). Conventional Commits, one logical
change each. Suggested order (each builds green on the last):

1. `test: add shared corpus (ast + text) and exclusions` — `corpus/**`, `vendor/` relocation.
2. `build: add insta, jq, llvm-cov to dev tooling` — dev-deps + `dev-setup.sh`.
3. `test(conformance): add shell conformance suite` — `tools/conformance-suite/**` (all five
   surfaces), verified locally against `.oracle/`.
4. `test: add layer-1 golden tests in the facade` — `golden_reader.rs`, `golden_writer.rs`,
   `common/mod.rs`, committed snapshots.
5. `test: relocate offline identity tests` — codec round-trip → `carta-ast`; native pair +
   spec-parse → `carta`.
6. `test(unit): cover writer/reader helpers and parser internals` — Layer 0.
7. `refactor: remove carta-testkit` — delete the crate after its pieces are rehomed.
8. `ci: split offline check, conformance, and coverage jobs` — `ci.yml` rewrite.
9. `chore: repoint agent workflows and docs to the new suite` — `.claude/workflows/**`, `AGENTS.md`,
   `docs/PORTING.md`.

Keep the WIP `test/shared-writer-corpus` branch as the content seed for commit 1; it can be deleted
after.

---

## 13. Verification checklist (definition of done)

- `cargo nextest run --workspace` passes **with `.oracle/` absent** (offline guarantee).
- `cargo test --doc --workspace` passes.
- `cargo clippy --all-targets --all-features` clean under `-D warnings`.
- `cargo llvm-cov --workspace --summary-only --fail-under-lines 90` passes (product crates).
- `tools/conformance-suite/run.sh all` with `.oracle/` present: every surface `fail=0 err=0`; `skip`
  counts reported and sane (writer/e2e skips match `exclusions.tsv`; command-tests skips match the
  unsupported-flag allowlist). CommonMark reader + e2e remain `pass=652`.
- No product source (outside `tools/**`, `docs/**`, `README.md`, `AGENTS.md`, `corpus/README.md`,
  `vendor/**` attributions) contains "pandoc"/"oracle"/provenance phrasing — grep to confirm.
- No committed test data is pandoc *output*: `corpus/ast/**` and `corpus/text/**` are inputs we own;
  all golden expected values are carta snapshots.
- `grep -rn 'carta-testkit' .` returns nothing (crate fully dissolved and de-referenced).
- `.claude/workflows/*.mjs` run against the new suite (smoke the repointed `SWEEP`).

---

## 14. Risks and notes

- **Coverage realism.** Because coverage is offline-only (decision 8), the `ast/` corpus must hit
  every writer node and the `text/` corpus + unit tests must hit every reader path to clear 90%. If
  90% is unreachable without contrived cases, prefer adding focused **unit tests** over bloating the
  corpus. Do not lower the floor without discussion.
- **Command-test grammar.** Confirm the exact separator/format from the fetched `test/README`
  (public). If the grammar is richer than the `% / ^D` shape, capture it in `commands.sh` and the
  suite README. Never read `*.hs`.
- **`pandoc -f json` version skew.** The corpus AST-JSON carries a `pandoc-api-version`. The pinned
  oracle (`PANDOC_PIN=3.10`) must accept it; if pandoc rejects a hand-authored version array, mint
  the envelope from the pinned binary during authoring. carta's accepted version lives in
  `carta-ast` (`pandoc-api-version` constant) — keep the corpus consistent with it.
- **insta churn.** Snapshot diffs on intentional writer changes are reviewed via `cargo insta
  review`; document this in the suite/corpus README so contributors do not hand-edit `.snap` files.
- **Two corpora, one feature taxonomy.** `exclusions.tsv` features must match the `ast/`
  subdirectory names exactly; the golden-writer test and the shell writer surface both parse it —
  keep one parser spec in `corpus/README.md`.
```
