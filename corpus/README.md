# Shared test corpus

Hand-authored test inputs that drive both golden tests (`crates/carta/tests/`) and the conformance/parity suite (`tools/conformance-suite/`).

## Layout

```
corpus/
  ast/<feature>/*.json    drives WRITER tests: one full Document per file
  text/<format>/*.<ext>   drives READER tests: one construct family per file (UTF-8 text)
  binary/<format>/*.<ext> drives READER tests for byte-container formats, read as raw bytes
  standalone/*           source documents and images for template compilation checks
```

The subdirectory is the tag: under `ast/` it is the document feature, under `text/` and `binary/` it is the source format. The filename stem is the case label.

`ast/reader-trees/` holds trees that the readers produce and whose shapes no hand-authored `ast/` case has. Each file is named `<format>-<label>.json` and is the tree of the case `text/<format>/<label>`, so the writers are tested on it without a snapshot for each reader and writer pair.
