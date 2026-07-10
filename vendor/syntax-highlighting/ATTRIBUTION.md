# Vendored syntax highlighting data

This directory records provenance for the syntax-highlighting data bundled with the
`carta-highlight` crate. The working copies the crate compiles against live under
`crates/carta-highlight/data/` (mirroring the two-copy pattern used for the HTML entity
table); the files here are the authoritative attribution record.

## Syntax definitions — `crates/carta-highlight/data/syntax/*.xml`

164 KDE-format (`.xml`) syntax definitions, as bundled in the `xml/` directory of
**skylighting-core 0.14.7**, the highlighting grammar set that pandoc 3.10 ships. Each file
retains its original `<!-- ... -->` header and its upstream `license=` attribute unchanged.

These grammars originate from the KDE Frameworks *syntax-highlighting* project and independent
contributors. They carry a mix of licenses, declared per file in the `license=` attribute of the
`<language>` element. The distribution across the 164 files:

| Declared license      | Files |
| --------------------- | ----- |
| LGPL (incl. v2/v2+/v2.1+) | 68 |
| MIT                   | 43 |
| GPL (incl. v2/v2+/v3+) | 16 |
| FDL                   | 1 |
| BSD / New BSD / BSD3  | 3 |
| WTFPL                 | 1 |
| LLVM Release License  | 1 |
| GPL,BSD               | 1 |
| (unspecified)         | 8 |

The unspecified files declare no `license=` attribute in the `<language>` element; they are
distributed under the same terms as the KDE syntax-highlighting collection they ship in. Per-file
license text is preserved inside each grammar's own comment header.

## Styles — `crates/carta-highlight/data/styles/*.theme`

8 color themes — `pygments`, `tango`, `espresso`, `zenburn`, `kate`, `monochrome`, `breezedark`,
`haddock` — captured verbatim from the observable CLI output of pandoc 3.10
(`pandoc --print-highlight-style=<name>`). They are stored in the same `.theme` JSON layout that
command emits, so `carta --print-highlight-style` can reproduce them byte-for-byte.
