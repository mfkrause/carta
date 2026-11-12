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

DROPPED_TARGET = "Pandoc's RST output drops the inline target, so links to it break."

# (source format, target, case) -> reason.
E2E = {
    ("asciidoc", "rst", "anchor-labels"): DROPPED_TARGET,
    ("asciidoc", "rst", "cross-references"): DROPPED_TARGET,
    ("asciidoc", "rst", "definition-terms"): DROPPED_TARGET,
    ("asciidoc", "rst", "discrete-cross-references"): DROPPED_TARGET,
    ("asciidoc", "rst", "formatted-shorthand"): (
        "Pandoc's RST output drops inline targets and strong emphasis."
    ),
    ("asciidoc", "rst", "heading-macro-text"): (
        "Pandoc's RST output drops an inline target and keeps a trailing space in a title."
    ),
    ("asciidoc", "rst", "inline-macros"): DROPPED_TARGET,
    ("asciidoc", "rst", "inline-roles"): "Pandoc's RST output drops strong emphasis.",
    ("asciidoc", "rst", "macro-attributes"): "Pandoc's RST output drops strong emphasis.",
    ("asciidoc", "rst", "malformed-inline-delimiters"): "Pandoc's RST output drops emphasis.",
    ("asciidoc", "rst", "table-block-anchors"): "Pandoc's RST output drops targets in cells.",
    ("asciidoc", "rst", "user-attribute-names"): DROPPED_TARGET,
    ("asciidoc", "rst", "zero-table-span"): (
        "Pandoc writes a table without columns as '|  |', a broken RST substitution."
    ),
    ("docbook", "rst", "cross-reference-labels"): DROPPED_TARGET,
    ("docbook", "rst", "inline"): DROPPED_TARGET,
    ("docbook", "rst", "media"): (
        "Pandoc writes the figure option without colons, which corrupts the image path."
    ),
    ("ipynb", "commonmark", "version-three"): (
        "Pandoc writes a level-9 heading, which CommonMark reads as text."
    ),
    ("ipynb", "mediawiki", "version-three"): (
        "Pandoc writes a level-9 heading, which MediaWiki does not have."
    ),
    ("jira", "rst", "anchor-whitespace"): DROPPED_TARGET,
    ("jira", "rst", "spans"): DROPPED_TARGET,
    ("latex", "latex", "nested-tables"): "Pandoc nests a longtable in a cell, which LaTeX rejects.",
    ("latex", "rst", "figures"): (
        "Pandoc writes the figure option without colons, which corrupts the image path."
    ),
    ("latex", "rst", "longtable-captions"): (
        "Pandoc breaks a caption over two lines, which ends the table directive early."
    ),
    ("rst", "rst", "inline-targets"): DROPPED_TARGET,
    ("rst", "rst", "internal-targets-multiple"): DROPPED_TARGET,
    ("rtf", "rst", "bookmark-empty"): DROPPED_TARGET,
    ("rtf", "rst", "bookmark"): DROPPED_TARGET,
    ("typst", "rst", "labels-references"): DROPPED_TARGET,
}

DATA_IMAGE = "Pandoc drops images given as data URIs; carta extracts them to media/."

# (target, feature, case) -> reason.
WRITER = {
    ("beamer", "image-dimensions", "image-embedded-jpeg"): DATA_IMAGE,
    ("beamer", "image-dimensions", "image-embedded-png"): DATA_IMAGE,
    ("html4", "table", "table-foot"): "Pandoc puts tfoot after tbody, which HTML 4 forbids.",
    ("latex", "image-dimensions", "image-embedded-jpeg"): DATA_IMAGE,
    ("latex", "image-dimensions", "image-embedded-png"): DATA_IMAGE,
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
    ("rst", "common", "span"): "Pandoc's RST output drops the span's identifier.",
    ("rst", "common", "span-semantic"): "Pandoc's RST output drops the span's identifier.",
}

XHTML11 = "which XHTML 1.1 does not allow"
FAILED_FETCH = "Pandoc writes the broken path '../https://…' when the fetch fails."
CONTROL_CHARS = "Pandoc keeps characters that XML forbids, so the page is not well-formed."

# (dialect, feature, case) -> reason.
EPUB = {
    **{
        (dialect, "common", case): reason
        for dialect in ("epub2", "epub3")
        for case, reason in (
            ("image-external", FAILED_FETCH),
            ("literal-control-chars", CONTROL_CHARS),
        )
    },
    ("epub2", "common", "ordered-start"): f"Pandoc uses the start attribute, {XHTML11}.",
    ("epub2", "common", "span-semantic"): f"Pandoc uses the mark element, {XHTML11}.",
    ("epub2", "common", "underline-smallcaps"): f"Pandoc uses the u element, {XHTML11}.",
    ("epub2", "table", "table-caption-blocks"): f"Pandoc puts paragraphs in a caption, {XHTML11}.",
    ("epub2", "table", "table-caption-div"): f"Pandoc puts a division in a caption, {XHTML11}.",
    ("epub2", "table", "table-empty"): f"Pandoc writes a table without rows, {XHTML11}.",
    ("epub2", "table", "table-foot"): f"Pandoc puts tfoot after tbody, {XHTML11}.",
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
        if surface == "e2e":
            reason = E2E.get((source, target, stem), reason)
            if reason and target != "json":
                return f"golden_e2e__{source}__{target}__{stem}", reason
        return (f"golden_reader__{source}__{stem}", reason) if reason else None
    if len(parts) == 4 and parts[0] == "writer":
        _, target, feature, filename = parts
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
