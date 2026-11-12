#!/usr/bin/env python3
"""Check controls: cases where pandoc's output is wrong and carta's is correct.

A control accepts a difference from pandoc only when carta's output still matches its reviewed
snapshot. Each entry states what pandoc does wrong. A control never hides a parity gap in carta:
fix such a gap, or let its case fail.
"""

import json
from pathlib import Path
import sys


# (source format, case) -> reason. Covers the reader surface and every e2e target of the case.
# The tree of each `corpus/text` case here is also in `corpus/ast/reader-trees`.
READER = {
    ("docbook", "metadata"): "Pandoc joins author name parts without a space.",
    ("docbook", "nested-documents"): "Pandoc joins author name parts without a space.",
    ("docbook+east_asian_line_breaks", "east-asian-metadata"): (
        "Pandoc joins author name parts without a separator."
    ),
    ("latex+raw_tex", "raw-commands"): "Pandoc drops the star of a raw \\ref*.",
    ("latex", "multirow"): "Pandoc ignores the upward span of \\multirow{-2}.",
    ("latex", "raised-boxes"): (
        "Pandoc drops the text of \\raisebox and keeps its size arguments as text."
    ),
    ("latex", "references"): (
        "Pandoc skips anonymous figures when it numbers figures, and gives \\part no number."
    ),
    ("man", "manual-references"): "Pandoc drops .MR references and .SM/.SB text.",
    ("man", "tables"): "Pandoc replaces tables with spanned cells by the word TABLE.",
    ("org", "internal-links"): (
        "Pandoc gives two targets the same identifier, and its links to them point nowhere."
    ),
    ("rtf", "control-word-delimiter"): "Pandoc drops a hyphen that ends a control word.",
    ("rtf", "unicode-fallback-punctuation"): (
        "Pandoc skips one character too many after a \\u fallback character."
    ),
}

DATA_IMAGE = "Pandoc drops images given as data URIs; carta extracts them to media/."
DROPPED_TARGET = "Pandoc's RST output drops the inline target, so links to it break."
BROKEN_KEY_SPAN = (
    "Pandoc writes the key span after '{plus}' with single marks, which AsciiDoc reads as text."
)
DROPPED_ANCHOR = "Pandoc drops the anchor of a figure without content, so the link to it breaks."
DROPPED_CAPTION = "Pandoc drops the caption of a figure whose image is in a paragraph."
HTML4_FOOT = "Pandoc puts tfoot after tbody, which HTML 4 forbids."

# (target, feature, case) -> reason. A `reader-trees` case is the tree that carta reads from the
# `corpus/text` case of that name, so its entry also covers that e2e conversion.
WRITER = {
    ("asciidoc", "reader-trees", "asciidoc-heading-macro-text"): BROKEN_KEY_SPAN,
    ("asciidoc", "reader-trees", "asciidoc-inline-macros"): BROKEN_KEY_SPAN,
    ("asciidoc", "reader-trees", "latex-references"): DROPPED_ANCHOR,
    ("asciidoc", "reader-trees", "typst-content-spacing"): DROPPED_CAPTION,
    ("asciidoc", "reader-trees", "typst-labels-references"): DROPPED_CAPTION,
    ("asciidoc_legacy", "reader-trees", "asciidoc-heading-macro-text"): BROKEN_KEY_SPAN,
    ("asciidoc_legacy", "reader-trees", "asciidoc-inline-macros"): BROKEN_KEY_SPAN,
    ("asciidoc_legacy", "reader-trees", "latex-references"): DROPPED_ANCHOR,
    ("asciidoc_legacy", "reader-trees", "typst-content-spacing"): DROPPED_CAPTION,
    ("asciidoc_legacy", "reader-trees", "typst-labels-references"): DROPPED_CAPTION,
    ("beamer", "image-dimensions", "image-embedded-jpeg"): DATA_IMAGE,
    ("beamer", "image-dimensions", "image-embedded-png"): DATA_IMAGE,
    ("commonmark", "reader-trees", "ipynb-version-three"): (
        "Pandoc writes a level-9 heading, which CommonMark reads as text."
    ),
    ("commonmark_x", "reader-trees", "ipynb-version-three"): (
        "Pandoc writes a level-9 heading, which CommonMark reads as text."
    ),
    ("gfm", "reader-trees", "ipynb-version-three"): (
        "Pandoc writes a level-9 heading, which GFM reads as text."
    ),
    ("html4", "reader-trees", "asciidoc-definition-macro-bodies"): (
        "Pandoc uses the video and audio elements, which HTML 4 does not have."
    ),
    ("html4", "reader-trees", "asciidoc-table-block-anchors"): HTML4_FOOT,
    ("html4", "reader-trees", "asciidoc-table-in-definition-continuation"): HTML4_FOOT,
    ("html4", "reader-trees", "asciidoc-table-monospace-nested"): HTML4_FOOT,
    ("html4", "table", "table-foot"): HTML4_FOOT,
    ("latex", "image-dimensions", "image-embedded-jpeg"): DATA_IMAGE,
    ("latex", "image-dimensions", "image-embedded-png"): DATA_IMAGE,
    ("latex", "reader-trees", "latex-nested-tables"): (
        "Pandoc nests a longtable in a cell, which LaTeX rejects."
    ),
    ("man", "common", "list-bullet-in-definition"): (
        "Pandoc does not indent the list under its term."
    ),
    ("man", "common", "list-nested-first-block"): "Pandoc prints a literal '.RS 2' in the page.",
    ("man", "table", "table-cell-blocks"): (
        "Pandoc's paragraph macro in the cell adds a large indent and widens the table."
    ),
    ("man", "table", "table-empty-cell"): "Pandoc drops a list of empty items from the cell.",
    ("man", "table", "table-grid-spans"): (
        "Pandoc's table overflows the line, and groff cannot break its lines."
    ),
    ("markdown_mmd", "reader-trees", "docbook-nested-documents"): (
        "Pandoc drops the caption of a figure whose image has its own description."
    ),
    ("mediawiki", "reader-trees", "ipynb-version-three"): (
        "Pandoc writes a level-9 heading, which MediaWiki does not have."
    ),
    ("org", "reader-trees", "asciidoc-definitions-nested"): (
        "Pandoc starts a nested list on the term line, where Org reads its markers as text."
    ),
    ("org", "reader-trees", "asciidoc-list-block-indentation"): (
        "Pandoc starts an example block on the term line, where Org reads it as text."
    ),
    ("org", "reader-trees", "asciidoc-table-in-definition-continuation"): (
        "Pandoc starts a table on the term line and indents its rows unevenly."
    ),
    ("org", "reader-trees", "asciidoc-table-monospace-nested"): (
        "Pandoc writes the footnote of a table body cell twice and never refers to the first copy."
    ),
    ("revealjs", "reader-trees", "ipynb-version-three"): (
        "Pandoc leaves the cells after a level-9 heading outside every slide."
    ),
    ("rst", "common", "span"): "Pandoc's RST output drops the span's identifier.",
    ("rst", "common", "span-semantic"): "Pandoc's RST output drops the span's identifier.",
    ("rst", "reader-trees", "asciidoc-anchor-labels"): DROPPED_TARGET,
    ("rst", "reader-trees", "asciidoc-cross-references"): DROPPED_TARGET,
    ("rst", "reader-trees", "asciidoc-definition-terms"): DROPPED_TARGET,
    ("rst", "reader-trees", "asciidoc-discrete-cross-references"): DROPPED_TARGET,
    ("rst", "reader-trees", "asciidoc-formatted-shorthand"): (
        "Pandoc's RST output drops inline targets and strong emphasis."
    ),
    ("rst", "reader-trees", "asciidoc-heading-macro-text"): (
        "Pandoc's RST output drops an inline target and keeps a trailing space in a title."
    ),
    ("rst", "reader-trees", "asciidoc-inline-macros"): DROPPED_TARGET,
    ("rst", "reader-trees", "asciidoc-inline-roles"): "Pandoc's RST output drops strong emphasis.",
    ("rst", "reader-trees", "asciidoc-macro-attributes"): (
        "Pandoc's RST output drops strong emphasis."
    ),
    ("rst", "reader-trees", "asciidoc-malformed-inline-delimiters"): (
        "Pandoc's RST output drops emphasis."
    ),
    ("rst", "reader-trees", "asciidoc-table-block-anchors"): (
        "Pandoc's RST output drops targets in cells."
    ),
    ("rst", "reader-trees", "asciidoc-user-attribute-names"): DROPPED_TARGET,
    ("rst", "reader-trees", "asciidoc-zero-table-span"): (
        "Pandoc writes a table without columns as '|  |', a broken RST substitution."
    ),
    ("rst", "reader-trees", "docbook-cross-reference-labels"): DROPPED_TARGET,
    ("rst", "reader-trees", "docbook-inline"): DROPPED_TARGET,
    ("rst", "reader-trees", "docbook-media"): (
        "Pandoc writes the figure option without colons, which corrupts the image path."
    ),
    ("rst", "reader-trees", "jira-anchor-whitespace"): DROPPED_TARGET,
    ("rst", "reader-trees", "jira-spans"): DROPPED_TARGET,
    ("rst", "reader-trees", "latex-figures"): (
        "Pandoc writes the figure option without colons, which corrupts the image path."
    ),
    ("rst", "reader-trees", "latex-longtable-captions"): (
        "Pandoc breaks a caption over two lines, which ends the table directive early."
    ),
    ("rst", "reader-trees", "rst-inline-targets"): DROPPED_TARGET,
    ("rst", "reader-trees", "rst-internal-targets-multiple"): DROPPED_TARGET,
    ("rst", "reader-trees", "rtf-bookmark"): DROPPED_TARGET,
    ("rst", "reader-trees", "rtf-bookmark-empty"): DROPPED_TARGET,
    ("rst", "reader-trees", "typst-labels-references"): DROPPED_TARGET,
    ("typst", "reader-trees", "asciidoc-table-cell-contexts"): (
        "Pandoc opens a cell with a bare '==', which Typst reads as a heading."
    ),
}

XHTML11 = "which XHTML 1.1 does not allow"
FAILED_FETCH = "Pandoc writes the broken path '../https://…' when the fetch fails."
CONTROL_CHARS = "Pandoc keeps characters that XML forbids, so the page is not well-formed."
MISSING_FILE = (
    "Pandoc keeps the path of a file that it could not read, so the page refers to a file that"
    " is not in the package."
)
MARK_ELEMENT = f"Pandoc uses the mark element, {XHTML11}."
U_ELEMENT = f"Pandoc uses the u element, {XHTML11}."
SPAN_NAME = f"Pandoc puts a name attribute on a span, {XHTML11}."
FOOT_AFTER_BODY = f"Pandoc puts tfoot after tbody, {XHTML11}."

# (dialect, feature, case) -> reason. A later entry for the same key replaces an earlier one.
EPUB = {
    **{
        (dialect, "common", case): reason
        for dialect in ("epub2", "epub3")
        for case, reason in (
            ("image-external", FAILED_FETCH),
            ("literal-control-chars", CONTROL_CHARS),
        )
    },
    **{
        (dialect, "reader-trees", case): MISSING_FILE
        for dialect in ("epub2", "epub3")
        for case in (
            "asciidoc-definition-macro-bodies",
            "asciidoc-images",
            "asciidoc-inline-macros",
            "asciidoc-table-monospace-nested",
            "asciidoc-user-attribute-names",
            "docbook-media",
            "latex-figures",
            "latex-raised-boxes",
            "typst-content-spacing",
            "typst-labels-references",
        )
    },
    ("epub2", "common", "ordered-start"): f"Pandoc uses the start attribute, {XHTML11}.",
    ("epub2", "common", "span-semantic"): MARK_ELEMENT,
    ("epub2", "common", "underline-smallcaps"): U_ELEMENT,
    ("epub2", "reader-trees", "asciidoc-definition-terms"): SPAN_NAME,
    ("epub2", "reader-trees", "asciidoc-formatted-shorthand"): MARK_ELEMENT,
    ("epub2", "reader-trees", "asciidoc-images"): (
        f"{MISSING_FILE} It also puts an alt attribute on a division, {XHTML11}."
    ),
    ("epub2", "reader-trees", "asciidoc-table-block-anchors"): (
        f"Pandoc puts tfoot after tbody and a name attribute on a span, {XHTML11}."
    ),
    ("epub2", "reader-trees", "asciidoc-table-in-definition-continuation"): FOOT_AFTER_BODY,
    ("epub2", "reader-trees", "asciidoc-table-monospace-nested"): (
        f"{MISSING_FILE} It also puts tfoot after tbody, {XHTML11}."
    ),
    ("epub2", "reader-trees", "asciidoc-table-title-counters"): SPAN_NAME,
    ("epub2", "reader-trees", "docbook-inline"): U_ELEMENT,
    ("epub2", "reader-trees", "jira-color-inline-multiline"): (
        f"Pandoc puts a color attribute on a span, {XHTML11}."
    ),
    ("epub2", "reader-trees", "latex-emphasis"): U_ELEMENT,
    ("epub2", "reader-trees", "org-internal-links"): (
        f"Pandoc puts a target attribute on a span, {XHTML11}."
    ),
    ("epub2", "reader-trees", "rst-math-directive-label"): (
        f"Pandoc puts a label attribute on a span, {XHTML11}."
    ),
    ("epub2", "reader-trees", "rtf-hyperlink-formatted"): U_ELEMENT,
    ("epub2", "reader-trees", "typst-content-spacing"): (
        f"{MISSING_FILE} It also uses the u element, {XHTML11}."
    ),
    ("epub2", "table", "table-caption-blocks"): f"Pandoc puts paragraphs in a caption, {XHTML11}.",
    ("epub2", "table", "table-caption-div"): f"Pandoc puts a division in a caption, {XHTML11}.",
    ("epub2", "table", "table-empty"): f"Pandoc writes a table without rows, {XHTML11}.",
    ("epub2", "table", "table-foot"): FOOT_AFTER_BODY,
    ("epub2", "task-list", "checkboxes"): "Pandoc uses form controls, which EPUB 2 forbids.",
}

# (case, part) -> reason.
DOCX = {
    (case, part): reason
    for case, reason in (
        ("language", "Pandoc drops the scoped language from some runs."),
        (
            "direction",
            "Pandoc drops the scoped direction from some runs, and paragraph styles where it"
            " adds bidi.",
        ),
    )
    for part in ("document", "footnotes", "comments")
}


# Mirrors DIALECT_FAMILIES in crates/carta/tests/golden_writer.rs. A writer has no snapshot for a
# case when an earlier writer of its family gives the same output.
DIALECT_FAMILIES = (
    ("html", "html4"),
    ("asciidoc", "asciidoc_legacy"),
    (
        "commonmark",
        "markdown",
        "gfm",
        "commonmark_x",
        "markdown_github",
        "markdown_mmd",
        "markdown_phpextra",
        "markdown_strict",
    ),
)


# Target names that select the same writer as another name; only that name has snapshots.
ALIASES = {"asciidoctor": "asciidoc", "docbook5": "docbook"}


def reviewed_snapshots(directory, name):
    """The snapshot of a control, or those of the earlier writers in its dialect family."""
    snapshot = directory / f"{name}.snap"
    if snapshot.exists() or not name.startswith("golden_writer__"):
        return [snapshot]
    target, _, case = name.removeprefix("golden_writer__").partition("__")
    family = next((family for family in DIALECT_FAMILIES if target in family), ())
    earlier = family[: family.index(target)] if family else ()
    return [directory / f"golden_writer__{dialect}__{case}.snap" for dialect in earlier]


def expectation(label):
    """The reviewed snapshot and the reason for a control label, or None."""
    parts = label.split("/")
    if len(parts) == 3 and parts[0] in {"reader", "reader-ext", "e2e"}:
        surface, formats, filename = parts
        stem = Path(filename).stem
        source, _, target = formats.partition("->")
        reason = READER.get((source, stem))
        if surface == "e2e" and target != "json":
            tree = f"{source}-{stem}"
            reason = WRITER.get((target, "reader-trees", tree), reason)
            return (f"golden_writer__{target}__reader-trees__{tree}", reason) if reason else None
        return (f"golden_reader__{source}__{stem}", reason) if reason else None
    if len(parts) == 4 and parts[0] == "writer":
        _, target, feature, filename = parts
        target = ALIASES.get(target, target)
        stem = Path(filename).stem
        reason = WRITER.get((target, feature, stem))
        return (f"golden_writer__{target}__{feature}__{stem}", reason) if reason else None
    if len(parts) == 4 and parts[0] == "epub":
        _, dialect, feature, stem = parts
        reason = EPUB.get((dialect, feature, stem))
        return (f"epub__{dialect}_body__{feature}__{stem}", reason) if reason else None
    if len(parts) == 4 and parts[:2] == ["docx", "docx"]:
        case, part = parts[2:]
        reason = DOCX.get((case, part))
        if reason:
            name = f"docx__docx__{case}" if part == "document" else f"docx__{case}__{part}"
            return name, reason
    return None


def main():
    mode, label, actual_path = sys.argv[1:]
    expected = expectation(label)
    if expected is None:
        return 2
    name, reason = expected
    root = Path(__file__).resolve().parents[2]
    actual = Path(actual_path).read_bytes()
    if mode == "docx":
        actual = actual.replace(b"><", b">\n<")
    if mode == "epub":
        actual = actual.split(b"<body", 1)[1].split(b">\n", 1)[1].split(b"</body>", 1)[0]
    for snapshot in reviewed_snapshots(root / "crates/carta/tests/snapshots", name):
        if not snapshot.exists():
            continue
        body = snapshot.read_bytes().split(b"\n---\n", 1)[1]
        if mode == "json":
            matches = json.loads(body) == json.loads(actual)
        else:
            matches = body.removesuffix(b"\n") == actual.removesuffix(b"\n")
        if matches:
            print(f"{label}: {reason} Checked {snapshot.name}.")
            return 0
    print(f"Output also differs from the reviewed snapshot: {name}.snap")
    return 1


if __name__ == "__main__":
    sys.exit(main())
