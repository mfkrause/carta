# Shared test corpus

Hand-authored test inputs that drive both layers of the test suite:

- Golden tests (`crates/carta/tests/`, offline): snapshot carta's own output for every corpus case with [`insta`](https://insta.rs). Committed snapshots live under `crates/carta/tests/snapshots/`; review changes with `cargo insta review`, never by hand-editing `.snap` files.
- Conformance suite (`tools/conformance-suite/`, requires `.oracle/`): convert every corpus case with both carta and pandoc and diff the results.

## Layout

```
corpus/
  exclusions.tsv          target<TAB>feature pairs a writer does not yet implement
  ast/<feature>/*.json    drives WRITER tests: one full Document per file
  text/<format>/*.<ext>   drives READER tests: one construct family per file (UTF-8 text)
  binary/<format>/*.<ext> drives READER tests for byte-container formats, read as raw bytes
```

The subdirectory is the tag: under `ast/` it is the document feature; under `text/` and `binary/` it is the source format. The filename stem is the case label. There is no separate manifest; discovery walks the tree.

### `ast/<feature>/`

Each file is one complete `Document` as interchange JSON (the `pandoc-api-version` / `meta` / `blocks` envelope). The feature directories are `common`, `table`, `figure`, `math`, and `image-dimensions`. Together they exercise every block and inline node and their attribute permutations, including AST shapes no reader produces (e.g. table row/column spans), which is the point of driving writers from a standalone AST corpus rather than from reader output.

A writer that cannot yet render a feature lists the `(target, feature)` pair in `exclusions.tsv`; both layers skip those pairs and report the skip count.

### `text/<format>/`

Small source documents, one construct family per file, per reader format. These are the offline reader regression net; exhaustive reader conformance comes from the vendored spec examples (e.g. `vendor/commonmark/spec.txt`), which the conformance suite runs in addition.

### `binary/<format>/`

Reader fixtures for byte-container formats whose input is a binary archive (e.g. a zipped office/e-book container) or otherwise not valid UTF-8, so a reader takes raw bytes (`carta_core::BytesReader`). This tree is read verbatim as bytes; `text/` is decoded as UTF-8 and would reject such a fixture. Because a binary fixture cannot be written by hand, generate it (assemble the container's parts and zip them deterministically) and commit the bytes (do not emit it via the oracle, which can bake generator metadata into the file).

## Adding a case

1. Writer case: add `corpus/ast/<feature>/<label>.json` (a complete Document). If a target cannot render it yet, add the `(target, feature)` line to `exclusions.tsv`.
2. Reader case: add `corpus/text/<format>/<label>.<ext>` covering one construct family. For a byte-container format, use `corpus/binary/<format>/<label>.<ext>` (raw bytes) instead.
3. Refresh snapshots: `cargo insta test --accept` (then review the diff), or `cargo insta review`.
4. Check parity: `tools/conformance-suite/run.sh all` (requires `.oracle/`).
