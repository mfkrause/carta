# Conformance suite

Differential conformance tests: run carta and the pinned pandoc oracle over the same inputs and diff their output. This is the layer that proves **carta == pandoc**; it is pure shell, lives outside `cargo test`, and is gated as a required CI job. The offline Rust suites (unit + golden snapshots) prove carta against its own frozen output and need no oracle; this suite is the only thing that consults pandoc.

## Running

```sh
cargo build -p carta                        # the suite diffs the built debug binary
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
- **skip** — the oracle rejected the input, or the case is an excluded/unsupported direction (see each surface below). Skips are never silent — they are counted and reported.

`run.sh` exits non-zero if any surface recorded a `fail` or `err`. **The suite is expected to be red until carta reaches full parity** — every `fail` is a real conformance gap the suite has surfaced, and CI gates on them so they cannot regress further or be forgotten.

## Reproducing one failure

When a group records a `fail` or `err`, the surface prints the path to its full log:

```
RESULT writer html pass=120 fail=3 err=0 skip=5
  details: /tmp/carta-conformance.a1B2c3/writer-html.log
```

Each log entry carries the case label, the two exact `repro:` invocations (oracle and carta), and the full diff (bounded at 200 lines). The workflow to iterate on one divergence:

1. Run the surface and read the `details:` log to find the failing case's stem.
2. Rerun the surface narrowed to that one case — pass the case stem as a second argument:

   ```sh
   tools/conformance-suite/run.sh writer html my-case   # only corpus/ast/*/my-case.json
   tools/conformance-suite/run.sh reader latex my-case   # only corpus/text/latex/my-case.*
   ```

   The stem is the corpus filename without its extension (`corpus/ast/<feature>/<stem>.json`, `corpus/text/<fmt>/<stem>.<ext>`).
3. Copy the `repro:` lines from the log and run them by hand to iterate on the divergence.

Case narrowing is currently wired for the `reader` and `writer` surfaces only.

## Requirements

The gitignored `.oracle/` tree must be provisioned, plus `jq`:

```sh
tools/install-pandoc.sh        # .oracle/bin/pandoc
tools/fetch-pandoc-tests.sh    # .oracle/tests/test (native corpus + command tests)
```

`run.sh` fails loudly with these hints if anything is missing.

## Environment

- `CARTA_BIN` — path to the carta binary (default `target/debug/carta`).
- `CONF_WORK` — scratch + per-case diff logs. Defaults to a fresh per-run directory (`$TMPDIR/carta-conformance.XXXXXX`) so concurrent runs never collide; set it to pin a fixed location. Not auto-deleted, so the `.log` files survive for inspection.

## Surfaces

| surface | what it diffs | inputs |
|---|---|---|
| `reader` | `-f FMT -t json`, compared with `jq -S` | `corpus/text/<fmt>/*`, the extension-toggle cases in `corpus/text-ext/<spec>/*`, + the 652 CommonMark spec examples (commonmark) |
| `writer` | `-f json -t TARGET`, JSON structurally / others as text | `corpus/ast/<feature>/*` minus `corpus/exclusions.tsv` |
| `e2e` | `-f FMT -t TARGET` full pipeline | `corpus/text/<fmt>/*` to every target; spec examples to HTML |
| `roundtrip` | JSON codec identity: `pandoc -f native -t json` then `carta -f json -t json` | fetched `.native` corpus |
| `commands` | declarative command tests, vs a live normalized oracle | `.oracle/tests/test/command/*.md` |
| `extensions` | structural gate: every reader-honored extension has an oracle-parity case | `crates/carta-core` (the variant table), the reader source, and `corpus/text-ext/` |
| `templates` | `-f markdown -t TARGET --template=T`, compared byte-for-byte (verbatim output, no trailing-newline tolerance) | self-contained `corpus/templates/<case>/` (an owned template + body/metadata + optional flags) across eight targets |
| `standalone` | structural parity of each format's default `-s` scaffold (title block, preamble): proves both sides carry the same content and metadata, NOT a byte diff | `corpus/ast/<feature>/*` rendered standalone per target |
| `media` | both sides of the media bag: `--extract-media` rewrites the document and writes embedded resources, diffing the rewritten output and the extracted file tree | `corpus/text/ipynb/*.ipynb` |
| `epub` | carta's EPUB writer two ways — structurally against the oracle (unpack + diff text entries) and against the EPUB spec with EPUBCheck | `corpus/ast/<feature>/*` (epub3 and epub2) |
| `docx` | carta's DOCX writer structurally: unpack both Office Open XML packages and diff each content-bearing part after canonicalizing the XML | `corpus/ast/<feature>/*` |

### Comparison and normalization

- **JSON targets** (`json`, and the reader/roundtrip surfaces) are canonicalized with `jq -S` before diffing, so object-key order is never a divergence.
- **Text targets** strip one trailing newline from each side (carta's CLI and pandoc each append one) and byte-compare.
- The oracle is run with normalization flags that neutralize nondeterminism carta does not reproduce: HTML gets `--syntax-highlighting=none --mathjax`, LaTeX gets `--syntax-highlighting=none`. Applied to the pandoc side only (`oracle_norm` in `lib.sh`).
- An input the oracle itself rejects is a `skip`, never counted against carta.

### Writer exclusions

`corpus/exclusions.tsv` lists `target<TAB>feature` pairs a writer cannot yet render (a `todo!()` site). The `writer` surface skips those pairs and counts them; when a `todo!()` is implemented, delete its line and the corpus cases activate automatically. The feature tag is the `corpus/ast/` subdirectory name.

### Command tests

Each command test is a fenced block: a `% pandoc <args>` line, the stdin input, a `^D` separator, then the expected stdout. `commands.sh` parses `(args, input)` with awk and runs the conversion through both binaries.

Two deliberate scoping choices:

1. **Compared against a live normalized oracle, not the baked expected.** The committed expected output was produced without carta's deterministic normalization (suppressed syntax highlighting, MathJax), so diffing against it would flag intentional deltas as failures. Re-running the oracle with normalization is the correct reference and keeps this surface consistent with the others.
2. **Strict allowlist.** Only tests whose command is a bare `pandoc` invocation using exclusively input/output format flags (`-f`/`-r`/`--from`/`--read`, `-t`/`-w`/`--to`/`--write`) with a fully implemented `(from ∈ {commonmark, html, native, json}, to ∈ {html, native, json, mediawiki})` pair are run. Everything else — unported formats, extension flags, file arguments, pipelines — is skipped and counted.

## Adding cases

Add inputs to the shared corpus, not here:

- a reader construct → `corpus/text/<fmt>/<label>.<ext>`
- an extension toggle → `corpus/text-ext/<spec>/<label>.<ext>`, where `<spec>` is the exact `-f`
  string (e.g. `commonmark+mark`, `markdown-blank_before_header`). The `extensions` surface
  **requires** one such directory for every extension the reader honors.
- a writer node shape → `corpus/ast/<feature>/<label>.json` (a complete Document JSON)
- a newly implemented writer feature → delete its `corpus/exclusions.tsv` line

See `corpus/README.md`. The same corpus drives the offline golden snapshots, so a new case is covered by both layers at once.
