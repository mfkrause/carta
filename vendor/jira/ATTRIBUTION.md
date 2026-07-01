# Vendored Jira wiki markup specification

`spec.md` captures the public Jira wiki markup "text formatting notation", as published at
<https://jira.atlassian.com/secure/WikiRendererHelpAction.jspa?section=all> (the same reference is
served by every Jira instance at `/secure/WikiRendererHelpAction.jspa?section=all`). The reference
capture is pinned in `VERSION` by date, as the notation carries no public version number.

It is vendored here as a public format specification — an allowed source of truth for this
implementation — to document the markup surface the Jira reader parses.

## Licensing

The upstream reference page is Atlassian documentation: it is copyrighted, all rights reserved, and
covered by the Atlassian website terms of use. There is no open licence permitting redistribution,
so the page is **not** vendored verbatim and no upstream `LICENSE` file is included here. The source
URL above is recorded instead.

`spec.md` is therefore an original, condensed description authored for this repository from the
public reference. It records the markup tokens — functional notation rather than protected creative
expression — and the structures they produce; it is not a copy of the upstream page's prose.

## Refreshing

To refresh or re-pin: re-read the reference page linked above, update `spec.md` to reflect any
changed notation, and set `VERSION` to the new capture date.
