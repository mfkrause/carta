# Vendored Jupyter Notebook format specification

`spec.rst` is the human-readable
[Jupyter Notebook format description](https://nbformat.readthedocs.io/en/latest/format_description.html),
copied verbatim as the reStructuredText source published in the
[`jupyter/nbformat`](https://github.com/jupyter/nbformat) repository and pinned to release `5.10.4`
(see `VERSION`). It is authored by the Jupyter Development Team (and the IPython Development Team
before it) and released under the [BSD 3-Clause License](https://opensource.org/license/bsd-3-clause)
(see `LICENSE`).

It is vendored here as a public format specification — an allowed source of truth for this
clean-room implementation — to document the on-disk `.ipynb` structure that this direction's reader
parses and the writer emits. The document describes notebook format version 4 (major `4`, current
minor `5`); the canonical machine-readable JSON schema it references is not vendored.

Refresh or re-pin with `tools/fetch-ipynb-spec.sh` (edit `SPEC_VERSION` to bump).
