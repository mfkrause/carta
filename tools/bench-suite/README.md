# Benchmark suite

Times `carta` against pandoc on equivalent work and reports how much faster (and leaner) carta is.

## Prerequisites

- `hyperfine`: the timing driver.
- `jq`: builds fixtures and parses results.
- `.oracle/`: the pinned pandoc binary (`tools/install-pandoc.sh`).
- The carta release binary is built automatically (`cargo build --release -p carta`).

## Usage

```sh
tools/bench-suite/run.sh all                 # every surface
tools/bench-suite/run.sh writer              # one surface, default targets
tools/bench-suite/run.sh writer latex        # narrow to one target
tools/bench-suite/run.sh reader commonmark   # one reader format
tools/bench-suite/run.sh e2e commonmark:html # one from:to pair
tools/bench-suite/run.sh pair commonmark mediawiki  # arbitrary pair, all sizes
tools/bench-suite/run.sh size                # binary sizes only (no timing)
tools/bench-suite/run.sh all --json out.json # markdown as usual, plus structured results
```

### Surfaces

| surface   | measures                                                                 |
|-----------|--------------------------------------------------------------------------|
| `reader`  | `<fmt> → json` parsing (default: commonmark, html)                        |
| `writer`  | `json → <target>` rendering (all 8 targets; rich AST incl. tables)        |
| `e2e`     | full `from → to` conversion (what users actually run)                     |
| `startup` | near-empty conversion; isolates process spin-up (the fairness baseline)   |
| `size`    | binary sizes (no timing)                                                  |

### Tunables (env)

| var            | default     | meaning                                             |
|----------------|-------------|-----------------------------------------------------|
| `BENCH_SIZES`  | `10k,100k,1m` | input sizes to sweep (`k`/`m` = KiB/MiB)           |
| `BENCH_WARMUP` | `3`         | hyperfine warmup runs                               |
| `BENCH_RUNS`   | *(adaptive)*| fixed run count (else hyperfine decides)            |
| `BENCH_OUT`    | `target/bench` | output dir for fixtures + raw hyperfine JSON     |
| `BENCH_REGEN`  | `0`         | set `1` to rebuild fixtures from scratch            |
| `BENCH_RUNNER` | *(detected)*| how the machine is described in the provenance header |

## How it stays fair

Both binaries run with identical `-f`/`-t` flags, and pandoc is normalized (`--syntax-highlighting=none`, `--mathjax` for HTML) so both produce equivalent output. Inputs come in three sizes: small inputs are startup-dominated (pandoc's runtime spin-up dwarfs the work), large inputs are throughput-dominated, and the `startup` surface reports the spin-up cost explicitly. The release binary is always rebuilt before timing so numbers never come from a stale or debug build.

## Output

Markdown tables to stdout; raw hyperfine JSON per case under `$BENCH_OUT` (gitignored).

`--json <path>` additionally writes one structured document holding every measurement plus a provenance header: the date, the carta, pandoc and hyperfine versions, the machine, and the warmup and run counts. Markdown still goes to stdout, unchanged.

### Refreshing the committed numbers

```sh
tools/bench-suite/run.sh all --json /tmp/bench.json
tools/bench-suite/to-toml.sh /tmp/bench.json >docs/benchmarks.toml
cargo run --manifest-path tools/docgen/Cargo.toml -- --write
```

`docs/benchmarks.toml` holds the measurements; docgen renders `docs/BENCHMARKS.md` and the site's `benchmarks.json` from it, deriving every headline figure from the rows below it. Nothing in either output is written by hand, so no claim can outlive the numbers behind it. The `Benchmarks` workflow runs these three steps on a GitHub runner and opens a PR with the result.

## Fixtures

One authored seed (`corpus/bench/seed.md`, strict CommonMark) is repeated to size for the reader/e2e inputs; the other reader formats are derived from it via carta. The writer surface uses a curated subset of `corpus/ast/` (rich constructs incl. tables) cycled to size.
