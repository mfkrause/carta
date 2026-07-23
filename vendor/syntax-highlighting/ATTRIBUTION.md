# Vendored syntax highlighting data

This directory records provenance for the syntax-highlighting data bundled with the
`carta-highlight` crate. The working copies the crate compiles against live under
`crates/carta-highlight/data/` (mirroring the two-copy pattern used for the HTML entity
table); the files here are the authoritative attribution record.

## Syntax definitions: `crates/carta-highlight/data/syntax/` and `data/syntax-copyleft/`

164 KDE-format (`.xml`) syntax definitions, as bundled in the `xml/` directory of
**skylighting-core 0.14.7**, the highlighting grammar set that pandoc 3.10 ships. Each file
retains its original `<!-- ... -->` header and its upstream `license=` attribute unchanged.

These grammars originate from the KDE Frameworks *syntax-highlighting* project and independent
contributors. They carry a mix of licenses, declared per file in the `license=` attribute of the
`<language>` element. The set is split by license into two directories:

- `data/syntax/`: the 48 permissively licensed definitions (MIT, BSD variants, WTFPL, the
  NCSA-style LLVM Release License, and `markdown.xml`, whose declared `GPL,BSD` dual license is
  exercised under its BSD grant). These are compiled into `carta-highlight`.
- `data/syntax-copyleft/`: the 116 definitions under copyleft licenses or with no license
  statement. These are not compiled in by default; they load at runtime (see the directory's
  README) or embed via the `embed-copyleft-grammars` feature.

The distribution across the 164 files:

| Declared license      | Files |
| --------------------- | ----- |
| LGPL (incl. v2/v2+/v2.1+) | 68 |
| MIT                   | 42 |
| GPL (incl. v2/v2+/v3+) | 15 |
| FDL                   | 1 |
| BSD / New BSD / BSD3  | 3 |
| WTFPL                 | 1 |
| LLVM Release License  | 1 |
| GPL,BSD               | 1 |
| (none declared)       | 32 |

The 32 files with no declared license either omit the `license=` attribute entirely (24 files) or
leave it empty (8 files); they are distributed here on the same implied terms as the KDE
syntax-highlighting collection they ship in, and are treated as non-permissive: all 32 are in
`data/syntax-copyleft/`. Per-file license text, where present, is preserved inside each grammar's
own comment header.

Checked against KDE upstream (2026-07): the upstream files carry the same declared licenses
(KDE's MIT policy applies to new submissions only), so re-vendoring newer copies would not change
this split (sole exception: upstream `tlaplus.xml` is now MIT).

## Styles: `crates/carta-highlight/data/styles/*.theme`

8 color themes — `pygments`, `tango`, `espresso`, `zenburn`, `kate`, `monochrome`, `breezedark`,
`haddock` — captured verbatim from the observable CLI output of pandoc 3.10
(`pandoc --print-highlight-style=<name>`). They are stored in the same `.theme` JSON layout that
command emits, so `carta --print-highlight-style` can reproduce them byte-for-byte.
