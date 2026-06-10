# Benchmark suite — runtime comparison vs pandoc

Status: **planned** (implementation starting). Delivery: **one PR** off `main`.

This plan is standalone. It assumes no prior context beyond the repo and `AGENTS.md`
(`.claude/CLAUDE.md`). Read `AGENTS.md` first — the clean-room and source-hygiene rules are
load-bearing here too: the benchmark tooling lives under `tools/**` and `docs/**`, which are
sanctioned to name pandoc; **no product source, corpus data file, or build config may.**

> If you are resuming this work after losing context: this document is the complete spec. Read it
> top to bottom before touching anything. Sections 1–3 are the rationale and the full decision record
> (every question asked and answered during design); section 4 onward is the buildable design.

---

## 1. Motivation & initial requirement

carta already has a **conformance suite** (`tools/conformance-suite/`) that verifies *correctness* —
it diffs carta's output against the pinned pandoc oracle across every conversion surface. What it has
**no** answer for is *performance*: is carta actually faster than pandoc, and by how much, and where?

The original ask, verbatim in intent:

> Implement a benchmark suite to compare our implementation against pandoc itself. Decide how it
> should look, how it should be called/configured, how it should measure, what it should output.

This document is the result of that design plus a full grilling round to pin the specifics.

**One-line goal:** a manual, on-demand dev tool that runs carta and the pinned pandoc binary on
equivalent work and reports how much faster (and leaner) carta is, honestly.

---

## 2. First-draft spec (preserved for the record)

The initial rough proposal, before grilling, was:

- **Intent:** measure runtime performance (speed/throughput) of carta vs pandoc; distinct from the
  conformance suite (correctness). Cheap secondary headline metrics: binary size, startup overhead.
  Open fork: also add in-process criterion regression benches?
- **Location:** `tools/bench-suite/`, mirroring `conformance-suite/` (a `run.sh <surface|all>
  [filter]` dispatcher + a `lib.sh` reusing path discovery).
- **Driver:** `hyperfine` (warmup, statistics, JSON/markdown export); a new external dep gated like
  `jq`/`pandoc`. Build carta `--release`.
- **Metrics:** mean time, throughput (MB/s), speedup (×), standalone startup, binary size; peak RSS
  as a best-effort stretch goal.
- **Inputs:** a new `corpus/bench/` of realistically-sized fixtures (authored/generated, clean-room)
  at S/M/L sizes; optional sweep over the fetched pandoc corpus. Honor `exclusions.tsv`.
- **Surfaces:** `reader` (X→json), `writer` (json→X), `e2e` (real pairs), `startup`, `size`, `all`.
- **Invocation:** `run.sh <surface|all> [filter]`; env knobs `BENCH_RUNS`/`BENCH_WARMUP`/
  `BENCH_SIZES`/`BENCH_OUT`.
- **Output:** human markdown table → stdout; raw hyperfine JSON → gitignored `target/bench/`;
  optional regenerated `docs/BENCHMARKS.md` snapshot; not CI-gated.

Everything below supersedes this draft where they differ.

---

## 3. Decision record (the grilling round)

Each decision below is final. The alternatives are recorded so a future reader understands *why* the
chosen path was chosen and does not relitigate it.

1. **Scope → Comparison only.** Process-level head-to-head vs pandoc; end-to-end timing that includes
   startup. **No** in-process criterion/divan layer and **no** carta-vs-its-own-past regression
   tracking — those answer a different question, the conformance suite already guards correctness, and
   perf-regression-in-CI is a known tar pit. (Rejected: "comparison now, regression later";
   "both from day one".)

2. **Driver → `hyperfine`, hard dependency.** Warmup, outlier detection, statistics, JSON/markdown
   export for free. Gated in `require_tools` + `dev-setup.sh` with an install hint, matching the
   existing oracle-missing pattern. (Rejected: hand-rolled bash timing; soft/optional dependency.)

3. **Fixtures → seeds + runtime scaling.** Commit small authored text seed(s); generate sized
   variants by concatenation and derive AST inputs via carta at runtime into gitignored
   `target/bench/`. Small repo footprint. (Rejected: commit full sized files; reuse fetched pandoc
   corpus only.)
   - **Corollary (decided inline):** we author only **one** commonmark seed for the *text* surfaces.
     The reader surface's non-commonmark text inputs (html/native/json) are derived from that seed at
     runtime via carta itself (`carta -f commonmark -t <fmt>`). carta-generated html is still valid
     html both parsers ingest, so there is no fairness loss, and the only committed text fixture is one
     markdown file.
   - **Critical correction (writer surface):** carta's commonmark reader implements **strict
     CommonMark**, so an AST *derived from a markdown seed can never contain tables, footnotes,
     definition lists, math, etc.* — those are extensions the strict reader does not produce. Deriving
     the writer-surface AST from the seed would therefore never exercise table layout (`grid.rs`) or
     any other rich path, defeating decision 5. **So the writer-surface AST input is built from the
     existing `corpus/ast/` fixtures instead** (which already contain tables, footnotes, the full
     common set — all carta-owned inputs), by concatenating their `blocks` arrays and repeating to
     size. This is the only way the writer surface exercises the interesting constructs.

4. **Sizes → ~10KB / 100KB / 1MB (three points).** 10KB anchors the **startup-dominated** regime,
   1MB the **throughput-dominated** regime, 100KB the crossover. Reporting one number hides one
   regime. 1MB is about as large as a single real document gets; 10MB measures a regime nobody hits.
   (Rejected: two extremes only; single large; add 10MB.)

5. **Seed shape → one balanced kitchen-sink.** Prose-weighted but containing every major construct
   (headings, tight+loose lists, links, emphasis, code blocks, blockquotes, tables, footnotes) so the
   costly paths — notably table layout (`grid.rs`) — are exercised, while output stays one row-set per
   surface. (Rejected: balanced + dedicated table-heavy seed; per-construct seeds; prose-only. A
   table-heavy seed is a trivial later add if table perf becomes a specific question.)

6. **Surfaces / matrix → CLI-configurable, with curated defaults.** Do not hardcode the matrix; let
   the CLI select what to bench, with sensible defaults. Keep all five surfaces (`reader`, `writer`,
   `e2e`, `startup`, `size`) but curate default pairs rather than running a full cartesian
   (4 readers × 8 writers × 3 sizes) wall of redundant rows. (Rejected: hardcoded curated pairs;
   e2e+startup+size only; full cartesian.)

7. **CLI shape → surface+filter, pair escape, env axes.**
   - `run.sh <surface> [filter]` — conformance-familiar; `writer latex`, `reader commonmark`, `e2e`,
     `all`.
   - `run.sh pair <from> <to>` — generic escape hatch to bench *any* combination.
   - env knobs for orthogonal axes: `BENCH_SIZES`, `BENCH_RUNS`, `BENCH_WARMUP`, `BENCH_OUT`.
   - **No** config file. (Rejected: surface+filter only; config-file-driven.)

8. **Output → stdout + gitignored JSON + curated docs.** Human markdown table → stdout; raw
   `hyperfine` JSON → gitignored `target/bench/`; plus a **deliberately-regenerated**
   `docs/BENCHMARKS.md` (reference machine specs + pandoc version stamped, "indicative,
   machine-specific" caveat) so there is a citable "~N× faster" headline in the repo. (Rejected:
   stdout + JSON only — no citable number; auto-commit raw tables — churns, meaningless cross-machine
   diffs.)

9. **CI → not in CI, manual only.** On-demand dev tool, run locally like `cargo llvm-cov`. No gating,
   no CI job — avoids the perf-in-CI noise trap. (Rejected: run in CI without gating; run + threshold
   gate.)

10. **Memory → yes, fully cross-platform.** Peak RSS is a strong differentiator (pandoc carries a
    Haskell GC runtime; carta does not). Measure it in a separate single-run pass (memory is stable
    run-to-run, no statistics needed) via `/usr/bin/time`, handling **both** macOS `-l` (reports
    bytes) and GNU `-v` (reports KB). (Rejected: defer to v2; macOS-only best-effort.)

11. **Startup fairness → raw e2e + explicit startup line.** Report real end-to-end mean at every size
    (what users feel) plus a standalone startup figure. **Never** publish a synthetic
    "end-to-end minus startup" number — subtracting two noisy means is statistically unsound and
    invites cherry-picking. The size curve + the explicit startup line let the reader infer
    startup-vs-throughput. (Rejected: also report processing-only; large-size-only/ignore startup.)

12. **Fairness flags → reuse `oracle_norm`.** Apply the conformance suite's pandoc-side normalization
    (`--syntax-highlighting=none` for html/latex, `--mathjax` for html) so both tools do equivalent
    work producing equivalent output. Leaving pandoc's default syntax highlighting on would time
    pandoc doing work carta skips, inflating carta's win on code-heavy input. (Rejected: bench pandoc
    defaults; report both normalized and default.)

13. **Build → auto-build release at startup.** Run `cargo build --release -p carta-cli` before
    benching (no-op when fresh). Eliminates the stale/debug-binary footgun — publishing a number off
    the wrong binary is worse than a few seconds of build check. The pandoc oracle stays
    require-and-fail (we don't build it). (Rejected: require pre-built + fail with hint.)

14. **Seed construct scope → restrict to the universally-renderable set.** The seed contains only
    constructs every benched target can render today (drop math/figure/image-dimensions/task-list —
    see `corpus/exclusions.tsv`), so every size×target runs cleanly with **zero** skip logic. The seed
    auto-grows as exclusions clear. (Rejected: rich seed + per-target skip; per-target seed variants.)

**Placement details decided inline (not separate questions):**

- Seed lives at `corpus/bench/seed.md` (one authored commonmark seed; `corpus/` is already "inputs we
  own"). Generated sized variants, derived AST/text inputs, and raw JSON all go to gitignored
  `target/bench/`.
- **Generator footgun (resolved by authoring):** concatenating the seed N times could collide
  footnote/reference-definition labels across copies. Avoided entirely by authoring `seed.md`
  **inline-only** — no reference-style links, no footnotes (neither is strict CommonMark anyway, so
  carta's reader would not parse them). Concatenation is then pure repetition with no label rewriting.
  (The writer-surface AST is built from `corpus/ast/` and likewise just repeats whole blocks.)
- Input is fed via **stdin redirect** (`< file`), matching the conformance suite, to avoid file-read
  path differences between the two binaries.

---

## 4. Architecture

Mirror `tools/conformance-suite/` so the two tools feel like siblings.

```
tools/bench-suite/
  run.sh              # dispatcher: run.sh <surface|all> [filter] | run.sh pair <from> <to>
  lib.sh              # shared primitives (see §5); reuses conformance path/oracle_norm conventions
  gen-fixtures.sh     # build sized + derived inputs into $BENCH_OUT (idempotent)
  surfaces/
    reader.sh         # <fmt> -> json, per reader format
    writer.sh         # json -> <target>, per writer target
    e2e.sh            # curated real pairs (commonmark -> html/latex/rst/json)
    startup.sh        # near-empty input across a few pairs; isolates RTS spin-up
    size.sh           # binary sizes only, no timing
  README.md           # how to run, prerequisites, what each surface measures
corpus/bench/
  seed.md             # the single authored kitchen-sink commonmark seed (committed)
docs/BENCHMARKS.md    # deliberately-regenerated, caveated headline results (committed)
```

Gitignored (add to `.gitignore`): `target/bench/` (sized inputs, derived inputs, raw hyperfine JSON,
scratch).

### Format matrix (authoritative — from the registry)

Readers: `commonmark` (alias `markdown`), `json`, `native`, `html`.
Writers: `html` (alias `html5`), `json`, `plain`, `native`, `latex`, `commonmark`, `rst`, `mediawiki`.

Both binaries are invoked with **identical** `-f/-t` flags. For `commonmark` we use `-f commonmark`
on both (pandoc's CommonMark reader, *not* its extended `markdown`) so the workloads match.

### Curated defaults

- `reader` default formats: `commonmark` (the only non-trivial parser) and `html`. (`json`/`native`
  readers are simple; bench them only via explicit filter / `pair`.)
- `writer` default targets: **all 8** (the diverse, interesting half).
- `e2e` default pairs: `commonmark→html`, `commonmark→latex`, `commonmark→rst`, `commonmark→json`.
- `startup` default pairs: `commonmark→html`, `commonmark→json` on a near-empty (~1 line) input.
- All defaults are overridable by `[filter]` or `pair`.

---

## 5. `lib.sh` — shared primitives

Reuse the conformance suite's proven shapes; do not import its file (keep the tools independent), but
copy the small, stable primitives and adapt.

- **Path discovery:** `ROOT`, `ORACLE="$ROOT/.oracle/bin/pandoc"`,
  `OX="${OXIDOC_BIN:-$ROOT/target/release/carta}"` (note: **release**, not debug),
  `CORPUS="$ROOT/corpus"`, `BENCH_OUT="${BENCH_OUT:-$ROOT/target/bench}"`.
- **`require_tools`:** assert `hyperfine`, `/usr/bin/time`, the pandoc oracle, and `jq` (for parsing
  hyperfine JSON if needed) are present; fail loudly with provisioning hints. Do **not** assert the
  carta binary — we build it (next).
- **`ensure_release_binary`:** run `cargo build --release -p carta-cli` (quiet); abort on failure.
- **`oracle_norm <target>`:** identical to conformance — `html|html5 → --syntax-highlighting=none
  --mathjax`; `latex → --syntax-highlighting=none`; else empty. Applied to the **pandoc side only**.
- **`oracle_version`:** read `.oracle/PANDOC_VERSION` (already recorded by `install-pandoc.sh`) for
  stamping output.
- **Env knobs with defaults:** `BENCH_SIZES="10k,100k,1m"`, `BENCH_WARMUP=3`, `BENCH_RUNS` (empty →
  hyperfine adaptive; floor via `--min-runs 10` when set), `BENCH_OUT` (above).
- **`bench_pair <mode> <label> <input_file> <oracle_args> <carta_args>`:** the core timing call —
  shells out to `hyperfine` with `--warmup "$BENCH_WARMUP"`, `--shell=none` (avoid shell-spawn
  overhead; pass argv directly), `--export-json "$BENCH_OUT/<label>.json"`, and **two named
  commands**: `pandoc <oracle_args> < input` and `carta <carta_args> < input`. Because `--shell=none`
  cannot do stdin redirection, feed input via hyperfine's `--input <file>` (applies to every command)
  — confirm the installed hyperfine supports `--input`; if not, fall back to `--shell=default` with
  `"$ORACLE $oargs < $input"`. Parse the JSON to extract mean±σ/min for the table.
- **`measure_rss <cmd...> < input`:** single-run peak RSS. Detect the `/usr/bin/time` flavor once
  (`time_flavor`): try `/usr/bin/time -l true` (BSD/macOS, prints "maximum resident set size" in
  **bytes**) vs `/usr/bin/time -v true` (GNU, prints "Maximum resident set size (kbytes)"). Normalize
  both to bytes. If neither parses, skip RSS with a one-line notice.
- **Tally/report helpers:** mirror conformance (`PASS/FAIL/ERR/SKIP`, `report`, `tally_group`) but a
  bench "failure" means a binary errored or hyperfine failed — not an output diff. Surfaces print one
  human table per group and write raw JSON to `$BENCH_OUT`.

---

## 6. `gen-fixtures.sh` — input generation (idempotent)

Produces everything under `$BENCH_OUT/fixtures/`. Re-runnable; regenerates only missing files unless
`BENCH_REGEN=1`.

1. **Parse `BENCH_SIZES`** (`10k,100k,1m` → bytes: 10240, 102400, 1048576; accept `k`/`m` suffixes).
2. **Build sized commonmark inputs.** For each target size `T`, concatenate `corpus/bench/seed.md`
   `ceil(T / seedBytes)` times (pure repetition — the seed is inline-only, no labels to collide).
   Write `$BENCH_OUT/fixtures/commonmark.<size>.md`. Record exact byte length (used for MB/s).
3. **Derive reader inputs** for non-commonmark reader formats, per size, via carta:
   `carta -f commonmark -t html  < commonmark.<size>.md > html.<size>.html`
   `carta -f commonmark -t native < ... > native.<size>.native`
   `carta -f commonmark -t json   < ... > json.<size>.json`
4. **Build writer-surface AST input** per size from `corpus/ast/` (NOT from the seed — see §3
   correction). Use a **fixed curated subset** of `corpus/ast/*/*.json` (a representative ~15-block mix
   that includes a few tables near the front so even the smallest size exercises table layout), all
   drawn from the universally-renderable set (no figure/image-dimensions/math/task-list, the union of
   `corpus/exclusions.tsv`, so it renders cleanly across all 8 targets). The full corpus would make the
   base ~128KB — too coarse for a 10KB floor — hence a curated ~20KB base. With `jq -s`, collect the
   subset's `blocks` into one base array `B` (length `L`, serialized `baseBytes`). For each target size
   `T`: pick `N = ceil(T / (baseBytes/L))` and emit a Document
   `{ "pandoc-api-version": <from a source file>, "meta": {}, "blocks": [range(N)] | map(B[. % L]) }`
   to `$BENCH_OUT/fixtures/ast.<size>.json` (cycling the base to hit ~`T` bytes at a block boundary).
   The writer surface feeds this same json to both `carta -f json -t <target>` and
   `pandoc -f json -t <target>`. (Cycling whole blocks needs no label rewriting; duplicate footnote
   identifiers across copies are tolerated by both writers and irrelevant to timing.) Throughput for
   the writer surface uses the **AST json byte size** as input bytes.
5. **Startup input:** a fixed ~1-line `startup.md` (and its derived `startup.ast.json`).

All derived files are carta's own output → clean-room safe, never committed.

---

## 7. Surfaces

Every surface: `conf_reset`, generate/ensure fixtures, loop the relevant (pair × size), call
`bench_pair`, optionally `measure_rss` once per (pair × largest size), print a markdown table, write
JSON. Honor curated defaults; accept a `[filter]` to narrow.

- **`reader.sh [format]`** — for each reader format (default `commonmark`,`html`) × size:
  `bench_pair text "reader/<fmt>/<size>" <input> "-f <fmt> -t json" "-f <fmt> -t json"`.
  (Comparison is timing-only; `text`/`json` mode just controls table labeling.)
- **`writer.sh [target]`** — for each writer target (default all 8) × size, input = `ast.<size>.json`
  (built from `corpus/ast/`, §6 step 4): oracle args `-f json -t <target> $(oracle_norm <target>)`,
  carta args `-f json -t <target>`.
- **`e2e.sh [pair]`** — for each default pair (or one `from→to`) × size, input = `commonmark.<size>.md`
  (or the right reader input): oracle args `-f <from> -t <to> $(oracle_norm <to>)`, carta args
  `-f <from> -t <to>`.
- **`startup.sh [pair]`** — default pairs on `startup.md`; this is the explicit startup figure.
- **`size.sh`** — no timing: print `stat`-based byte sizes of `$ORACLE` and `$OX`, plus the ratio.
  (macOS `stat -f%z`, GNU `stat -c%s` — detect like the time flavor.)

`run.sh pair <from> <to>` routes to a generic e2e-style run over all sizes for an arbitrary pair,
generating any missing input on demand.

---

## 8. Output format

**Per-surface markdown table to stdout** (one block per surface/group):

```
## writer — json → latex   (pandoc 3.10, normalized)

| size  | carta mean±σ   | pandoc mean±σ  | speedup | carta MB/s | carta RSS | pandoc RSS |
|-------|----------------|----------------|---------|------------|-----------|------------|
| 10KB  | 0.6 ms ± 0.1   | 28.4 ms ± 1.2  |  47.3×  |    16.6    |   4.1 MB  |  61.8 MB   |
| 100KB | 2.9 ms ± 0.2   | 31.0 ms ± 0.9  |  10.7×  |    34.5    |   6.0 MB  |  72.4 MB   |
| 1MB   | 24.1 ms ± 0.6  | 70.2 ms ± 1.8  |   2.9×  |    41.5    |  18.2 MB  | 140.1 MB   |
```

Plus a **startup** block (its own table) and a **size** block (binary sizes). `speedup` = pandoc mean
/ carta mean. `MB/s` = input bytes / carta mean. RSS columns omitted with a note if the platform's
`/usr/bin/time` flavor is unrecognized. **No** startup-subtracted column anywhere.

**Raw JSON:** hyperfine's `--export-json` per (pair × size) under `$BENCH_OUT/*.json` (gitignored).

**`docs/BENCHMARKS.md`:** regenerated deliberately (e.g. `run.sh all > docs/BENCHMARKS.md` is *not*
the mechanism — instead a `--emit-doc` flag or a small wrapper writes a stamped document). Must carry,
at the top: reference machine (CPU, RAM, OS), carta version/commit, pandoc version, date, and a bold
caveat: *"Indicative only — numbers are machine-specific and will differ on your hardware."* Never
written by CI; regenerated by hand when a headline refresh is wanted.

---

## 9. Clean-room & hygiene checklist (do not skip)

- Bench tooling and `docs/BENCHMARKS.md` may name pandoc (they are under `tools/**` / `docs/**`).
- `corpus/bench/seed.md` is a **corpus data file** → **no** upstream provenance in it (it is just
  markdown we author; that is naturally fine).
- No committed file contains pandoc-generated output. Derived inputs are carta's own output and live
  only in gitignored `target/bench/`.
- `.gitignore`: add `target/bench/` (covered already if `target/` is ignored — verify).
- `git add` only the explicit new paths; never `-A`/`.`/`-u` (repo is multi-agent).

---

## 10. Implementation order

1. `corpus/bench/seed.md` — author the kitchen-sink seed (universal construct set only).
2. `tools/bench-suite/lib.sh` — path discovery, `require_tools`, `ensure_release_binary`,
   `oracle_norm`, env knobs, `time_flavor`/`measure_rss`, `bench_pair`, table helpers.
3. `tools/bench-suite/gen-fixtures.sh` — sized + derived inputs.
4. `tools/bench-suite/run.sh` — dispatcher (`<surface|all> [filter]`, `pair <from> <to>`).
5. Surfaces: `size.sh` (simplest, no timing) → `startup.sh` → `reader.sh` → `writer.sh` → `e2e.sh`.
6. `tools/bench-suite/README.md` — prerequisites, usage, what each surface measures, the fairness
   notes (oracle_norm, startup honesty).
7. `tools/dev-setup.sh` — add a `hyperfine` presence check.
8. Smoke-run locally (`.oracle/` + `hyperfine` present); generate an initial `docs/BENCHMARKS.md`.
9. Update `AGENTS.md` "Build & test" / "Commands" with a one-line bench entry.

Commit as conventional, one logical change per commit (`test`/`build`/`chore`/`docs` scopes as fit;
the suite itself is tooling — `build(bench)` or `chore(bench)` is reasonable). Push only when asked.

---

## 11. Open soft spots (flagged, decided enough to build)

- **Exact seed construct mix** and the **curated `e2e` default-pair list** are the two things most
  worth a human glance before/while building — both are easily adjusted later without structural
  change.
- **hyperfine `--input` / `--shell=none` stdin handling** — verify against the installed hyperfine
  version during step 2; fall back to `--shell=default` with quoted redirection if needed.
