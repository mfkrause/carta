# Shared test corpus

Hand-authored test inputs that drive both golden tests (`crates/carta/tests/`) and the conformance/parity suite (`tools/conformance-suite/`).

## Layout

```
corpus/
  exclusions.tsv          target<TAB>feature pairs a writer does not yet implement
  ast/<feature>/*.json    drives WRITER tests: one full Document per file
  text/<format>/*.<ext>   drives READER tests: one construct family per file (UTF-8 text)
  binary/<format>/*.<ext> drives READER tests for byte-container formats, read as raw bytes
  standalone/*           source documents and images for template compilation checks
```

The subdirectory is the tag: under `ast/` it is the document feature, under `text/` and `binary/` it is the source format. The filename stem is the case label.
