# Conformance suite

Differential conformance tests: run carta and the pinned pandoc oracle over the same inputs and
diff their output. This is the layer that proves **carta == pandoc**; it is pure shell, lives
outside `cargo test`, and is gated as a required CI job. The offline Rust suites (unit + golden
snapshots) prove carta against its own frozen output and need no oracle; this suite is the only
thing that consults pandoc.

## Running

```sh
cargo build -p carta-cli                   # the suite diffs the built debug binary
tools/conformance-suite/run.sh all          # every surface
tools/conformance-suite/run.sh writer       # one surface
tools/conformance-suite/run.sh writer rst   # one surface, narrowed to a format/target
```

Each surface prints one line per group:

```
RESULT <surface> <group> pass=N fail=N err=N skip=N
```

- **pass** — carta and the oracle agree.
- **fail** — they disagree; the first divergence is dumped to `$CONF_WORK/<surface>-<group>.log`.
- **err** — carta errored or panicked where the oracle produced output.
- **skip** — the oracle rejected the input, or the case is an excluded/unsupported direction (see
  each surface below). Skips are never silent — they are counted and reported.

`run.sh` exits non-zero if any surface recorded a `fail` or `err`. **The suite is expected to be red
until carta reaches full parity** — every `fail` is a real conformance gap the suite has surfaced,
and CI gates on them so they cannot regress further or be forgotten.

## Requirements

The gitignored `.oracle/` tree must be provisioned, plus `jq`:

```sh
tools/install-pandoc.sh        # .oracle/bin/pandoc
tools/fetch-pandoc-tests.sh    # .oracle/tests/test (native corpus + command tests)
```

`run.sh` fails loudly with these hints if anything is missing.

## Environment

- `OXIDOC_BIN` — path to the carta binary (default `target/debug/carta`).
- `CONF_WORK` — scratch + per-case diff logs (default `$TMPDIR/carta-conformance`).

## Surfaces

| surface | what it diffs | inputs |
|---|---|---|
| `reader` | `-f FMT -t json`, compared with `jq -S` | `corpus/text/<fmt>/*` + the 652 CommonMark spec examples (commonmark) |
| `writer` | `-f json -t TARGET`, JSON structurally / others as text | `corpus/ast/<feature>/*` minus `corpus/exclusions.tsv` |
| `e2e` | `-f FMT -t TARGET` full pipeline | `corpus/text/<fmt>/*` to every target; spec examples to HTML |
| `roundtrip` | JSON codec identity: `pandoc -f native -t json` then `carta -f json -t json` | fetched `.native` corpus |
| `commands` | declarative command tests, vs a live normalized oracle | `.oracle/tests/test/command/*.md` |

### Comparison and normalization

- **JSON targets** (`json`, and the reader/roundtrip surfaces) are canonicalized with `jq -S` before
  diffing, so object-key order is never a divergence.
- **Text targets** strip one trailing newline from each side (carta's CLI and pandoc each append
  one) and byte-compare.
- The oracle is run with normalization flags that neutralize nondeterminism carta does not
  reproduce: HTML gets `--syntax-highlighting=none --mathjax`, LaTeX gets
  `--syntax-highlighting=none`. Applied to the pandoc side only (`oracle_norm` in `lib.sh`).
- An input the oracle itself rejects is a `skip`, never counted against carta.

### Writer exclusions

`corpus/exclusions.tsv` lists `target<TAB>feature` pairs a writer cannot yet render (a `todo!()`
site). The `writer` surface skips those pairs and counts them; when a `todo!()` is implemented,
delete its line and the corpus cases activate automatically. The feature tag is the `corpus/ast/`
subdirectory name.

### Command tests

Each command test is a fenced block: a `% pandoc <args>` line, the stdin input, a `^D` separator,
then the expected stdout. `commands.sh` parses `(args, input)` with awk and runs the conversion
through both binaries.

Two deliberate scoping choices:

1. **Compared against a live normalized oracle, not the baked expected.** The committed expected
   output was produced without carta's deterministic normalization (suppressed syntax highlighting,
   MathJax), so diffing against it would flag intentional deltas as failures. Re-running the oracle
   with normalization is the correct reference and keeps this surface consistent with the others.
2. **Strict allowlist.** Only tests whose command is a bare `pandoc` invocation using exclusively
   input/output format flags (`-f`/`-r`/`--from`/`--read`, `-t`/`-w`/`--to`/`--write`) with a fully
   implemented `(from ∈ {commonmark, html, native, json}, to ∈ {html, native, json, mediawiki})`
   pair are run. Everything else — unported formats, extension flags, file arguments, pipelines — is
   skipped and counted. The skip count is large by design (most command tests exercise unported
   formats) and is reported, never hidden.

## Adding cases

Add inputs to the shared corpus, not here:

- a reader construct → `corpus/text/<fmt>/<label>.<ext>`
- a writer node shape → `corpus/ast/<feature>/<label>.json` (a complete Document JSON)
- a newly implemented writer feature → delete its `corpus/exclusions.tsv` line

See `corpus/README.md`. The same corpus drives the offline golden snapshots, so a new case is
covered by both layers at once.
