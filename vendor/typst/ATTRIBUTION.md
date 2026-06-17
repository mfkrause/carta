# Vendored Typst syntax specification

`spec.md` captures the public [Typst syntax reference](https://typst.app/docs/reference/syntax/)
(with shorthands from the [symbols reference](https://typst.app/docs/reference/symbols/)), pinned to
Typst version `0.14.2` (see `VERSION`). The Typst documentation is generated from the
[`typst/typst`](https://github.com/typst/typst) repository, authored by the Typst contributors and
released under the [Apache License 2.0](https://www.apache.org/licenses/LICENSE-2.0) (see `LICENSE`).

It is vendored here as a public format specification — an allowed source of truth for this
clean-room implementation — to document the markup surface the Typst writer targets. The text is a
condensed, reformatted summary authored for this repository from the public reference; it is not a
verbatim copy of the upstream documentation pages, which are rendered from Typst-markup and Rust
doc-comment sources rather than published as a single specification file.

To refresh or re-pin: re-read the reference and symbols pages linked above, update `spec.md` and the
version in both this file and `VERSION`, and replace `LICENSE` with the current
`https://raw.githubusercontent.com/typst/typst/main/LICENSE`.
