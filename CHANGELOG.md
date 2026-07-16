# Changelog

All notable changes to carta are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). While the
version is below `0.1.0`, anything may change at any time.

Version sections below are generated from the Conventional Commit history at release time and
curated in the release pull request, so there is no manually maintained _Unreleased_ section.

## [0.0.5](https://github.com/mfkrause/carta/compare/v0.0.4...v0.0.5) - 2026-07-16

### Added

- *(readers)* implement DOCX and EPUB readers ([#79](https://github.com/mfkrause/carta/pull/79))
- *(rtf)* implement RTF reader and writer ([#75](https://github.com/mfkrause/carta/pull/75))
- *(html-writer)* inline external resources for self-contained HTML ([#74](https://github.com/mfkrause/carta/pull/74))
- *(cli)* add --resource-path and a resolver-driven media embed helper
- *(writers)* implement syntax highlighting ([#72](https://github.com/mfkrause/carta/pull/72))

### Fixed

- *(commonmark)* keep a raw image tag whole when reflowing inline HTML
- *(latex)* box links and use dollar math inside underline and strikeout
- *(rst)* dedupe image substitution names and normalize length dimensions
- *(mediawiki)* size images in pixels from physical dimensions

### Other

- cover byte-container readers with a corpus/binary tree ([#76](https://github.com/mfkrause/carta/pull/76))

### Performance

- *(standalone)* eliminate whole-document copies in template rendering
- *(container)* compress zip entries at level 6
- *(container)* speed up crc32, sha-1, base64, xml escaping, and media dedup
- *(commonmark)* scan pipe-table rows in one pass
- *(commonmark)* sweep per-line allocations out of the block phase
- *(html)* tokenize by byte offsets and decode entities in a single pass

## [0.0.4](https://github.com/mfkrause/carta/compare/v0.0.3...v0.0.4) - 2026-07-06

### Added

- *(writers)* implement DOCX (OOXML) writer ([#71](https://github.com/mfkrause/carta/pull/71))

### Performance

- *(commonmark)* resolve bracket closes without label copies
- *(commonmark)* resolve emphasis without per-match splicing
- *(writers)* make static fill tokens allocation-free
- *(writers)* bulk-copy clean runs in writer escapers
- *(html-writer)* cut whole-document passes and per-element allocations

## [0.0.3](https://github.com/mfkrause/carta/compare/v0.0.2...v0.0.3) - 2026-07-04

### Added

- *(cli)* add JSON filters and a user data directory ([#64](https://github.com/mfkrause/carta/pull/64))
- *(writers)* implement EPUB writer ([#57](https://github.com/mfkrause/carta/pull/57))
- media bag for embedded resources ([#51](https://github.com/mfkrause/carta/pull/51))
- byte-capable reader/writer seam ([#46](https://github.com/mfkrause/carta/pull/46))

### Fixed

- *(epub-writer)* derive the default package language from the locale
- *(readers)* guard latex #0 and roman enumerator arithmetic ([#65](https://github.com/mfkrause/carta/pull/65))
- *(readers)* expand latex macros through input frames ([#61](https://github.com/mfkrause/carta/pull/61))
- *(readers)* bound commonmark block nesting and dokuwiki delimiter re-scanning ([#50](https://github.com/mfkrause/carta/pull/50))
- *(writers)* downgrade links nested inside an anchor fallback to spans
- *(writers)* render decoded-label links in bare form in dokuwiki, asciidoc, man, and org
- *(writers)* entity-encode single quotes in html attribute output
- *(writers)* align markdown-family link-title, code-span, and autolink output ([#59](https://github.com/mfkrause/carta/pull/59))

### Other

- document the public API and enable docs.rs feature banners
- *(corpus)* cover quoted titles, padded code spans, and decoded autolink labels
- *(readers)* dedup the scheme table, entity scanner, and heading-id disambiguation ([#49](https://github.com/mfkrause/carta/pull/49))
- *(writers)* split the shared helper module into cohesive submodules ([#63](https://github.com/mfkrause/carta/pull/63))
- *(writers)* share the identical markdown-family block helpers ([#48](https://github.com/mfkrause/carta/pull/48))

### Performance

- *(man-reader)* queue macro expansions instead of splicing ([#53](https://github.com/mfkrause/carta/pull/53))
- *(ast)* store textual payloads in a small-string type ([#47](https://github.com/mfkrause/carta/pull/47))
- *(ast)* box the attribute-bearing node payloads ([#45](https://github.com/mfkrause/carta/pull/45))
- *(cli)* number sections in place when no standalone wrapper runs ([#42](https://github.com/mfkrause/carta/pull/42))
- *(commonmark-reader)* migrate the inline layer to byte offsets ([#62](https://github.com/mfkrause/carta/pull/62))
- *(commonmark-reader)* index backtick runs for code-span close searches ([#60](https://github.com/mfkrause/carta/pull/60))
- *(readers)* reuse the heading pre-parse when contexts agree ([#56](https://github.com/mfkrause/carta/pull/56))
- *(readers)* resume heading-id suffix probing from the last issued suffix ([#55](https://github.com/mfkrause/carta/pull/55))
- *(commonmark-reader)* remember failed code-span close searches ([#54](https://github.com/mfkrause/carta/pull/54))
- *(commonmark-reader)* skip autolink scanning for text without trigger substrings ([#44](https://github.com/mfkrause/carta/pull/44))
- *(html-reader)* append words without a scratch allocation ([#43](https://github.com/mfkrause/carta/pull/43))
- *(readers)* bulk-scan plain inline text ([#39](https://github.com/mfkrause/carta/pull/39))
- *(html-writer)* escape text runs into the output buffer ([#66](https://github.com/mfkrause/carta/pull/66))

## [0.0.2](https://github.com/mfkrause/carta/compare/v0.0.1...v0.0.2) - 2026-07-02

### Fixed

- *(readers)* bound dokuwiki inline re-parsing and clamp html table spans
- *(readers)* compile the dokuwiki and jira features standalone

## [0.0.1] - 2026-07-02

Initial alpha release.
