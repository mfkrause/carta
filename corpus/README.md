# Shared test corpus

Hand-authored test inputs that drive both layers of the test suite:

- **Layer 1 — golden tests** (`crates/oxidoc/tests/`, offline): snapshot oxidoc's own output for
  every corpus case with [`insta`](https://insta.rs). Committed snapshots live under
  `crates/oxidoc/tests/snapshots/`; review changes with `cargo insta review`, never by hand-editing
  `.snap` files.
- **Layer 2 — conformance suite** (`tools/conformance-suite/`, requires `.oracle/`): convert every
  corpus case with both oxidoc and pandoc and diff the results.

Everything here is an **input we own**. No committed file is pandoc *output* used as a golden value:
golden expected values are oxidoc's own snapshots (Layer 1), and parity is checked live against the
pinned pandoc binary (Layer 2). The AST-JSON inputs were authored once and are maintained as static
data; the JSON AST shape itself is pandoc's published interchange contract.

## Layout

```
corpus/
  exclusions.tsv          target<TAB>feature pairs a writer does not yet implement
  ast/<feature>/*.json    drives WRITER tests — one full Document per file
  text/<format>/*.<ext>   drives READER tests — one construct family per file
```

The **subdirectory is the tag**: under `ast/` it is the document *feature*; under `text/` it is the
source *format*. The filename stem is the case label. There is no separate manifest — discovery walks
the tree.

### `ast/<feature>/`

Each file is one complete `Document` as interchange JSON (the `pandoc-api-version` / `meta` / `blocks`
envelope). The feature directories are `common`, `table`, `figure`, `math`, and `image-dimensions`.
Together they exercise every block and inline node and their attribute permutations — including AST
shapes no reader produces (e.g. table row/column spans), which is the point of driving writers from a
standalone AST corpus rather than from reader output.

A writer that cannot yet render a feature lists the `(target, feature)` pair in `exclusions.tsv`; both
layers skip those pairs and report the skip count.

### `text/<format>/`

Small source documents, one construct family per file, per reader format (`commonmark`, `html`,
`native`, `json`). These are the offline reader regression net; exhaustive reader conformance comes
from the vendored CommonMark spec examples (`vendor/commonmark/spec.txt`), which the conformance suite
runs in addition.

## Adding a case

1. **Writer case**: add `corpus/ast/<feature>/<label>.json` (a complete Document). If a target cannot
   render it yet, add the `(target, feature)` line to `exclusions.tsv`.
2. **Reader case**: add `corpus/text/<format>/<label>.<ext>` covering one construct family.
3. Refresh snapshots: `cargo insta test --accept` (then review the diff), or `cargo insta review`.
4. Check parity: `tools/conformance-suite/run.sh all` (requires `.oracle/`).
