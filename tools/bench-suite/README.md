# Benchmark suite

Times `carta` against the pinned pandoc binary on **equivalent work** and reports how much faster (and
leaner) carta is. This is a manual, on-demand dev tool — the sibling of `tools/conformance-suite/`,
which checks *correctness*. It is **not** part of `cargo test` and **not** run in CI (perf on shared
runners is too noisy to gate). Results are machine-specific; nothing it produces is committed except
the deliberately-regenerated `docs/BENCHMARKS.md`.

See `docs/plans/benchmark-suite.md` for the full design and the decision record behind it.

## Prerequisites

- **hyperfine** — `brew install hyperfine` (or `cargo install hyperfine`). The timing driver.
- **jq** — builds fixtures and parses results.
- **`.oracle/`** — the pinned pandoc binary: `tools/install-pandoc.sh`.
- The carta release binary is built automatically (`cargo build --release -p carta-cli`).

## Usage

```sh
tools/bench-suite/run.sh all                 # every surface
tools/bench-suite/run.sh writer              # one surface, default targets
tools/bench-suite/run.sh writer latex        # narrow to one target
tools/bench-suite/run.sh reader commonmark   # one reader format
tools/bench-suite/run.sh e2e commonmark:html # one from:to pair
tools/bench-suite/run.sh pair commonmark mediawiki  # arbitrary pair, all sizes
tools/bench-suite/run.sh size                # binary sizes only (no timing)
```

### Surfaces

| surface   | measures                                                                 |
|-----------|--------------------------------------------------------------------------|
| `reader`  | `<fmt> → json` parsing (default: commonmark, html)                        |
| `writer`  | `json → <target>` rendering (all 8 targets; rich AST incl. tables)        |
| `e2e`     | full `from → to` conversion — what users actually run                     |
| `startup` | near-empty conversion — isolates process spin-up (the fairness baseline)  |
| `size`    | binary sizes (no timing)                                                  |

### Tunables (env)

| var            | default     | meaning                                             |
|----------------|-------------|-----------------------------------------------------|
| `BENCH_SIZES`  | `10k,100k,1m` | input sizes to sweep (`k`/`m` = KiB/MiB)           |
| `BENCH_WARMUP` | `3`         | hyperfine warmup runs                               |
| `BENCH_RUNS`   | *(adaptive)*| fixed run count (else hyperfine decides)            |
| `BENCH_OUT`    | `target/bench` | output dir for fixtures + raw hyperfine JSON     |
| `BENCH_REGEN`  | `0`         | set `1` to rebuild fixtures from scratch            |

## How it stays fair

- **Identical `-f/-t` flags** on both binaries; for commonmark we use `-f commonmark` (not pandoc's
  extended markdown) so the workloads match.
- **pandoc is normalized** (`--syntax-highlighting=none`, `--mathjax` for HTML) so both produce
  equivalent output — otherwise we'd be timing pandoc doing work carta skips.
- **Three sizes.** Small inputs are *startup-dominated* (pandoc's runtime spin-up dwarfs the work);
  large inputs are *throughput-dominated*. The `startup` surface reports the spin-up cost explicitly.
  Read the small-input gap as mostly startup and the large-input gap as real throughput — there is no
  synthetic "startup-subtracted" number, by design.
- **Release build**, always rebuilt before timing so numbers never come from a stale or debug binary.

## Output

Markdown tables to stdout; raw hyperfine JSON per case under `$BENCH_OUT` (gitignored). To refresh the
committed headline document, regenerate `docs/BENCHMARKS.md` by hand and stamp it with this machine's
specs, the carta commit, and the pandoc version — never automate it.

## Fixtures

One authored seed (`corpus/bench/seed.md`, strict CommonMark) is repeated to size for the reader/e2e
inputs; the other reader formats are derived from it via carta. The writer surface uses a curated
subset of `corpus/ast/` (rich constructs incl. tables) cycled to size. All generated/derived inputs
live in the gitignored output dir — none are committed, and none are pandoc output.
