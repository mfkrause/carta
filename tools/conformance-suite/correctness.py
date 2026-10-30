#!/usr/bin/env python3
"""Check reviewed output for cases with intentional converter differences."""

import json
from pathlib import Path
import sys


READER_CASES = {
    ("docbook", "metadata"): "Keep spaces between name fields.",
    ("docbook", "nested-documents"): "Keep spaces between name fields.",
    ("docbook+east_asian_line_breaks", "east-asian-metadata"): "Keep explicit name-field separators.",
    ("latex+raw_tex", "raw-commands"): "Preserve the starred reference command.",
    ("latex", "multirow"): "Place upward-spanning content in its first row.",
    ("latex", "raised-boxes"): "Keep box content and consume layout arguments.",
    ("latex", "references"): "Count anonymous figures and label parts.",
    ("man", "manual-references"): "Keep manual references and inline styling.",
    ("man", "tables"): "Keep merged table cells.",
    ("org", "internal-links"): "Make target identifiers unique and keep links valid.",
    ("rtf", "control-word-delimiter"): "Keep text after a control word.",
    ("rtf", "unicode-fallback-punctuation"): "Keep text after Unicode fallback characters.",
}

RST_CASES = {
    ("docbook", "cross-reference-labels"),
    ("docbook", "inline"),
    ("docbook", "media"),
    ("jira", "anchor-whitespace"),
    ("jira", "spans"),
    ("latex", "figures"),
    ("latex", "longtable-captions"),
    ("rst", "inline-targets"),
    ("rst", "internal-targets-multiple"),
    ("rtf", "bookmark-empty"),
    ("rtf", "bookmark"),
    ("typst", "labels-references"),
}


def expectation(label):
    parts = label.split("/")
    if len(parts) == 3 and parts[0] in {"reader", "reader-ext", "e2e"}:
        surface, formats, filename = parts
        stem = Path(filename).stem
        source, _, target = formats.partition("->")
        reason = READER_CASES.get((source, stem))
        if surface == "e2e":
            if target == "rst" and (source, stem) in RST_CASES:
                reason = "Keep RST targets, captions, and directive options valid."
            if (source, target, stem) == ("latex", "latex", "nested-tables"):
                reason = "Use a valid inner table environment."
            if (source, target, stem) == ("typst", "html", "math-shorthands"):
                reason = "Keep the bidirectional arrow glyph."
            if (
                source == "ipynb"
                and stem == "version-three"
                and target in {"html", "commonmark", "mediawiki"}
            ):
                reason = "Limit headings to the target format's supported levels."
        if reason:
            name = f"golden_reader__{source}__{stem}"
            if surface == "e2e" and target != "json":
                name = f"golden_e2e__{source}__{target}__{stem}"
            return name, reason
    if parts == ["writer", "html4", "table", "table-foot.json"]:
        return "golden_writer__html4__table__table-foot", "Place the footer before the table bodies."
    if len(parts) == 4 and parts[:3] == ["writer", "rst", "common"]:
        stem = Path(parts[3]).stem
        if stem in {"span", "span-semantic"}:
            return f"golden_writer__rst__common__{stem}", "Keep inline target identifiers."
    if len(parts) == 4 and parts[:2] == ["docx", "docx"]:
        case, part = parts[2:]
        if case in {"language", "direction"} and part in {"document", "footnotes", "comments"}:
            name = f"docx__docx__{case}" if part == "document" else f"docx__{case}__{part}"
            return name, "Preserve language and direction in every document part."
    if len(parts) == 4 and parts[:2] == ["epub", "epub2"]:
        group, stem = parts[2:]
        cases = {
            ("common", "ordered-start"),
            ("common", "span-semantic"),
            ("common", "underline-smallcaps"),
            ("table", "table-caption-blocks"),
            ("table", "table-caption-div"),
            ("table", "table-empty"),
            ("table", "table-foot"),
            ("task-list", "checkboxes"),
        }
        if (group, stem) in cases:
            return f"epub__epub2_body__{group}__{stem}", "Use XHTML-compatible EPUB 2 markup."
    return None


def main():
    mode, label, actual_path = sys.argv[1:]
    expected = expectation(label)
    if expected is None:
        return 2
    name, reason = expected
    root = Path(__file__).resolve().parents[2]
    snapshot = root / "crates/carta/tests/snapshots" / f"{name}.snap"
    body = snapshot.read_bytes().split(b"\n---\n", 1)[1]
    actual = Path(actual_path).read_bytes()
    if mode == "docx":
        actual = actual.replace(b"><", b">\n<")
    if mode == "epub":
        actual = actual.split(b"<body>\n", 1)[1].split(b"</body>", 1)[0]
    if mode == "json":
        matches = json.loads(body) == json.loads(actual)
    else:
        matches = body.removesuffix(b"\n") == actual.removesuffix(b"\n")
    if not matches:
        print(f"Output also differs from the reviewed snapshot: {snapshot}")
        return 1
    print(f"{label}: {reason} Checked {snapshot.name}.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
