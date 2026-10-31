# Changelog

All notable changes to carta are documented in this file.

Version sections below are generated from the Conventional Commit history at release time and
curated in the release pull request, so there is no manually maintained _Unreleased_ section.

## [0.0.14](https://github.com/mfkrause/carta/compare/v0.0.13...v0.0.14) - 2026-10-31

### Fixed

- *(dokuwiki)* avoid exponential parsing of invalid footnotes

## [0.0.13](https://github.com/mfkrause/carta/compare/v0.0.12...v0.0.13) - 2026-10-30

### Added

- *(cli)* accept typed JSON template variables
- *(cli)* scope footnotes and identifiers per input file
- *(html)* support TeX image services
- *(writers)* support literate code without dropping content
- *(readers)* preserve literate program sections
- *(asciidoc)* support modern and legacy output names
- *(ipynb)* preserve raw Markdown cell source
- *(markdown)* support wiki links and preserve metadata
- *(plain)* honor Gutenberg text conventions

### Fixed

- *(html)* place legacy table footers before table bodies
- *(typst)* honor disabled code highlighting
- *(docx)* anchor only isolated inline equations
- *(cli)* preserve defaults variable names
- *(cli)* preserve precision in JSON template numbers
- *(cli)* load defaults files in option order
- *(cli)* let later options replace earlier values
- *(cli)* resolve scoped links relative to their input files
- *(markdown)* rebase relative targets by source directory
- *(epub)* preserve content in XHTML 1.1 fallbacks
- *(epub)* preserve image assets and descriptions
- *(cli)* combine binary inputs without losing images
- *(media)* extract embedded images for TeX output
- *(asciidoc)* include list markers in wrap widths
- *(asciidoc)* wrap inline footnote bodies
- *(writers)* wrap line blocks and preserve verse lines
- *(html)* render simple math with inline markup
- *(markdown)* preserve wrapped footnote definitions
- *(writers)* honor four-column list indentation
- *(cli)* parse metadata files with the selected Markdown variant
- *(core)* apply East Asian line break handling to all readers
- *(highlight)* resolve language aliases and canonical classes
- *(typst)* preserve title spacing and author details
- *(docx)* anchor paragraphs ending in inline equations
- *(docx)* share embedded image resources
- *(docx)* preserve scoped text direction
- *(docx)* preserve scoped language properties
- *(writers)* lay out tables inside quotes
- *(docbook)* preserve author markup and name spacing
- *(docbook)* render structured author names
- *(ipynb)* read version three worksheets and outputs
- *(rtf)* keep punctuation after control words
- *(rtf)* preserve formatted metadata and generator fields
- *(man)* count fallback equations when wrapping lines
- *(man)* keep list markers inside table cells
- *(man)* scope list bodies that start with blocks
- *(man)* render code syntax styles and preserve blank lines
- *(man)* preserve table cells across row and column spans
- *(man)* preserve manual citations and small text macros
- *(man)* classify alphabetic markers and split list styles
- *(latex)* preserve nested table content and boundaries
- *(latex)* preserve cells that span table rows
- *(latex)* separate definition terms from nested lists
- *(latex)* render checkbox markers in bullet lists
- *(latex)* preserve content inside raised boxes
- *(latex)* preserve long table captions and labels
- *(latex)* resolve references to numbered document targets
- *(latex)* preserve raw commands and following paragraph text
- *(markdown)* consume attributes after unresolved bracket groups
- *(markdown)* preserve attributes in reference definitions
- *(markdown)* resolve links through explicit header labels
- *(markdown)* preserve empty script spans and adjacent delimiters
- *(markdown)* keep task and ordinary bullet items together
- *(org)* preserve file URLs and absolute image paths
- *(org)* resolve named links to their declared anchors
- *(org)* preserve property drawers and heading identifiers
- *(rst)* apply table widths using source columns
- *(mediawiki)* preserve HTML flow in nested definition lists
- *(jira)* preserve list depth in definition bodies
- *(jira)* preserve list items across block contexts
- *(dokuwiki)* preserve table structure inside quotes
- *(dokuwiki)* preserve list structure in table cells
- *(asciidoc)* separate block content in table cells
- *(typst)* preserve section boundaries in table captions
- *(typst)* preserve empty cells and list items
- *(cli)* accept the ASCII flag for ODT output
- *(odt)* serialize package XML consistently
- *(odt)* indent table structure and cell content
- *(odt)* indent list items and nested content
- *(odt)* apply explicit syntax highlighting
- *(odt)* render internal link reference fields
- *(math)* keep modulo operands inside their operator group
- *(odt)* order combined text style attributes
- *(odt)* escape quotes in XML text content
- *(mathml)* preserve display and text fraction sizes
- *(odt)* name and reuse numbered list styles
- *(odt)* emit explicit closing tags for empty cells
- *(odt)* keep a literal space before preserved runs
- *(odt)* preserve metadata sequence items
- *(revealjs)* add a slide-relative table of contents
- *(odt)* omit paragraphs for empty definition bodies
- *(odt)* select definition spacing for each entry
- *(odt)* use loose spacing after an empty first list item
- *(odt)* retain delimiters when math conversion fails
- *(odt)* use PNG density when sizing images
- *(odt)* preserve source heading levels
- *(odt)* always include the standard formula styles
- *(odt)* omit the empty scripts element
- *(revealjs)* number slide titles and body headings
- *(org)* place nested bullets after definition terms
- *(org)* separate initial list blocks from their markers
- *(cli)* infer AsciiDoc for AsciiDoc input files
- *(writers)* suppress empty headers in text tables
- *(typst)* keep tight block spacing inside list items
- *(asciidoc)* preserve list markers before nested blocks
- *(writers)* preserve tight nested list spacing
- *(html)* omit empty table headers
- *(docx)* omit text styles from picture runs
- *(dokuwiki)* preserve code block trailing breaks
- *(revealjs)* prefix internal slide links
- *(beamer)* use presentation targets for internal links
- *(epub)* package images from data URLs
- *(media)* decode text payloads in data URLs
- *(typst)* decode embedded image sources
- *(odt)* preserve paragraphs with nonempty inline lists
- *(odt)* require the canonical raw format name
- *(odt)* number outer footnotes before nested notes
- *(odt)* omit empty footnote paragraphs
- *(odt)* omit empty line blocks by default
- *(odt)* omit empty code blocks
- *(odt)* use the figure style without a caption
- *(odt)* preserve block content in captions
- *(docx)* number empty figure and table captions
- *(writers)* preserve wrapped table captions
- *(typst)* preserve code block trailing newlines
- *(typst)* use label targets for internal links
- *(dokuwiki)* preserve widths for ordinary multiline cells
- *(latex)* use tabular for nested tables
- *(ast)* [**breaking**] release deep trees without recursive drop
- *(ipynb)* preserve exact numeric values
- *(ast)* preserve nonfinite column widths in JSON
- *(core)* traverse deep documents with stack growth
- *(ast)* grow the stack while encoding deep trees
- *(media)* preserve percent-encoded binary bytes
- *(markdown)* honor four-column list continuations when reading
- *(odt)* use fallback dimensions for damaged PNG images
- *(odt)* honor East Asian line break settings
- *(toc)* honor unlisted sections and division boundaries
- *(typst)* account for copied data and report resource exhaustion
- *(highlight)* handle deep rule graphs without repeated state scans
- *(highlight)* detect context cycles without fixed step limits
- *(markdown)* expand embedded TeX macros in source order
- *(latex)* keep comments and spacing in macro arguments
- *(markdown)* preserve escapes and comments in raw TeX groups
- *(markdown)* preserve standalone TeX macro definitions
- *(markdown)* preserve raw macro parameter text
- *(latex)* preserve token order during delayed expansion
- *(latex)* read delimited macro arguments as balanced tokens
- *(latex)* preserve token aliases and expand macros in math
- *(readers)* apply ASCII heading identifiers consistently
- *(markdown)* discard soft breaks only when requested
- *(markdown)* honor the angle bracket escape option
- *(markdown)* honor legacy dash conventions
- *(markdown)* preserve raw TeX argument boundaries
- *(markdown)* normalize bare fence language names
- *(markdown)* keep underscore emphasis inside native spans
- *(markdown)* preserve raw span tag spelling
- *(markdown)* bind smart abbreviations in citation affixes
- *(markdown)* preserve indentation in parsed HTML content
- *(markdown)* preserve content after unclosed HTML
- *(markdown)* preserve HTML paragraph boundaries and contents
- *(markdown)* tighten paragraphs before raw text blocks
- *(docx)* space math operators and preserve text boundaries
- *(docx)* use canonical math symbol commands
- *(docx)* preserve breaks and text boxes
- *(markdown)* validate and preserve attribute escapes
- *(markdown)* check alert marker indentation
- *(readers)* honor configured tab stops
- *(ipynb)* validate required fields and execution counts
- *(ipynb)* reject unknown cell and output kinds
- *(rtf)* normalize escaped control characters in prose
- *(rtf)* decode escaped bytes with the declared character set
- *(rtf)* discard optional line wraps
- *(man)* keep argument spacing in verbatim font macros
- *(latex)* keep content before the document body
- *(mediawiki)* preserve literal labels from link targets
- *(mediawiki)* preserve unfinished reference text
- *(jira)* trim initial whitespace in quote macros
- *(dokuwiki)* recognize URI schemes without authority prefixes
- *(dokuwiki)* preserve nested footnote content
- *(typst)* preserve paragraph boundaries in content values
- *(typst)* preserve code-mode math expressions
- *(odt)* preserve nested character style state
- *(odt)* clamp table spans to the available grid
- *(mathml)* parse fenced separators as row operators
- *(odt)* preserve list numbering and identifier state
- *(mathml)* convert measured spacing units
- *(odt)* read math only from referenced objects
- *(odt)* preserve form feeds in text
- *(odt)* restrict inline container content
- *(odt)* share cross-reference anchor targets
- *(odt)* preserve bibliography citations
- *(odt)* omit unsupported block containers
- *(odt)* restrict note bodies to paragraphs
- *(odt)* omit direct tables in list items
- *(odt)* restrict table cell block content
- *(odt)* filter direct link content
- *(odt)* preserve style definition precedence
- *(odt)* require matching list marker punctuation
- *(odt)* normalize package-relative link and image targets
- *(odt)* recognize numeric font weight steps
- *(odt)* omit numbered paragraph containers
- *(odt)* omit ruby wrappers
- *(odt)* omit in-content metadata wrappers
- *(odt)* omit cached list numbering labels
- *(odt)* omit cached field display text
- *(typst)* reject unknown emoji names
- *(odt)* parse integer space counts
- *(odt)* merge whitespace at inline boundaries
- *(odt)* drop images without resource targets
- *(odt)* use bullets for unknown number formats
- *(odt)* preserve large ordered list starts
- *(odt)* preserve large heading levels
- *(odt)* preserve section containers
- *(odt)* flatten inline breaks in code text
- *(odt)* normalize figures after paragraph compaction
- *(odt)* attach leading table captions when reading
- *(odt)* strip figure labels from captions only
- *(odt)* retain table caption classes when reading
- *(typst)* avoid repeated regex prefilter construction
- *(typst)* build math text without copying nested expressions
- *(typst)* charge repeated execution without a parser depth cutoff
- *(typst)* grow the stack for recursive value operations
- *(latex)* share group source without a parser depth cutoff
- *(mediawiki)* share nested source scans without a depth cutoff
- *(mediawiki)* resolve emphasis with linear parsing work
- *(dokuwiki)* preserve nested content without a depth cutoff
- *(jira)* share inline indexes without a nesting cutoff
- *(rst)* track include cycles and charge expansion work
- *(rst)* track expansion cycles without a footnote depth limit
- *(man)* build bodiless tagged lists without recursion
- *(commonmark)* preserve block nesting without a depth cutoff
- *(typst)* share sequence values and defer repeated content
- *(latex)* track macro cycles and share expansion work limits
- *(dokuwiki)* memoize inline searches without a scan budget
- *(jira)* index inline closing markers without scan budgets
- *(docbook)* resolve entity dependencies without repeated passes
- *(commonmark)* index raw TeX groups and environments
- *(man)* retain expansion context for generated macro calls
- *(org)* index inline delimiters without a scan budget
- *(typst)* compile regex patterns without a source length cap
- *(epub)* use a valid language for neutral process locales
- *(gfm)* include quoted table layout in isolated builds
- *(rst)* retain inline reference targets
- *(rst)* preserve figure content and target names
- *(typst)* preserve embedded image byte arrays
- *(beamer)* honor requested output columns
- *(latex)* wrap footnotes with scoped indentation
- *(typst)* wrap footnotes within enclosing blocks
- *(man)* count continuations and raw line widths
- *(docx)* preserve relation operator spacing
- *(docx)* render equation parenthesis marks
- *(docx)* honor explicit equation limit placement
- *(docx)* render invisible math spacing boxes
- *(docx)* include list helpers in minimal builds
- *(rst)* escape literal smart punctuation
- *(ipynb)* preserve large execution counts
- *(rtf)* preserve nested table structure and cell widths
- *(rtf)* alternate automatic numbering by list depth
- *(latex)* preserve line block height and spacing
- *(dokuwiki)* preserve break types across rendering contexts
- *(typst)* preserve paragraph breaks in table cells
- *(odt)* restore paragraph flow inside generic containers
- *(odt)* size AVIF frames from image properties
- *(odt)* use JPEG resolution metadata consistently
- *(odt)* read TIFF image dimensions and resolution
- *(odt)* size PDF image frames from page bounds
- *(odt)* size EPS frames from declared bounds
- *(odt)* resolve SVG intrinsic dimensions consistently
- *(odt)* number captions with native sequence fields
- *(odt)* preserve automatic and relative table column widths
- *(odt)* indent nested block quotes
- *(odt)* start paragraph flow before container content
- *(odt)* preserve paragraph flow after raw blocks and figures
- *(odt)* retain span and container language styles
- *(odt)* render highlighted mark spans
- *(odt)* render map and boolean metadata values
- *(odt)* retain structured metadata text
- *(odt)* preserve missing data image media types
- *(mathml)* keep Greek capitals upright with font overrides
- *(mathml)* classify symbols by their operands
- *(mathml)* emit operator names as identifiers
- *(mathml)* mark named function applications
- *(mathml)* place accents on their operator glyphs
- *(mathml)* keep bare delimiters at a fixed size
- *(odt)* order list definitions by numbering settings
- *(odt)* create alignment styles per table column
- *(odt)* use character offsets for column style names
- *(odt)* clamp row spans to their table section
- *(odt)* reject invalid image dimensions
- *(odt)* omit the extra contents index name
- *(org)* wrap words inside styled inline spans
- *(odt)* suppress empty plain body blocks
- *(docx)* preserve code block trailing breaks
- *(docx)* omit empty verbatim runs in code blocks
- *(docx)* separate keyword lists with commas
- *(typst)* preserve figure reference labels
- *(latex)* avoid repeated page heads in nested tables
- *(org)* preserve heading depth within the output budget
- *(dokuwiki)* measure table padding between line breaks
- *(math)* expand macros through token frames
- *(typst)* preserve deep heading levels with numeric syntax

### Performance

- *(typst)* write collection text into one output buffer
- *(typst)* move owned call arguments into content containers
- *(mediawiki)* scan paragraph note state once
- *(mediawiki)* parse nested table cells from shared source ranges
- *(jira)* index deep list markers by byte
- *(typst)* skip repetition of empty content
- *(commonmark)* index suffix maxima for fence searches
- *(tables)* cache column measurements without a nesting cutoff

### Other

- *(cli)* normalize Windows paths in LaTeX media assertion
- cover missing reader extensions and stabilize coverage checks
- validate document packages against format requirements
- remove snapshots for excluded man cases
- refresh conversion and standalone snapshots
- *(asciidoc)* cover legacy writer output
- keep fixtures compatible with strict lint checks
- *(docbook)* update structured author spacing
- register numbered document reference corpus

## [0.0.12](https://github.com/mfkrause/carta/compare/v0.0.11...v0.0.12) - 2026-10-26

### Added

- *(pdf-writer)* implement PDF writer through external Typst

### Fixed

- *(templates)* supply commands needed for standalone compilation
- resolve push-time lint errors
- *(core)* [**breaking**] make conversion errors non-exhaustive
- *(typst)* validate show selectors before evaluating arguments
- *(man)* bound nesting of bodiless list tags
- *(math)* reject malformed optional arguments without retrying
- *(math)* bound total macro expansion work

## [0.0.11](https://github.com/mfkrause/carta/compare/v0.0.10...v0.0.11) - 2026-10-03

### Added

- *(html)* add MathML output
- *(cli)* infer formats and combine text inputs

### Fixed

- *(typst)* reject non-string regex patterns
- *(rst)* reuse cell measurements for table layouts
- *(writers)* bound horizontal rules during table measurement

## [0.0.10](https://github.com/mfkrause/carta/compare/v0.0.9...v0.0.10) - 2026-08-27

### Fixed

- *(carta)* make fetch independently selectable
- *(highlight)* stop nonconsuming context cycles
- *(typst)* reject missing regex patterns
- *(typst)* bound regular expression compilation
- *(rst)* bound recursive footnote expansion
- *(mathml)* avoid repeated nested rendering
- *(typst)* bound sequence repetition
- *(markdown)* honor raw html toggle

### Other

- *(carta)* derive ipynb extensions from markdown
- *(highlight)* remove unused grammar state
- *(readers)* reuse shared parsing helpers
- *(readers)* remove direct serde dependency

## [0.0.9](https://github.com/mfkrause/carta/compare/v0.0.8...v0.0.9) - 2026-08-11

### Added

- *(docbook)* add reader and writer
- *(typst)* add reader

### Fixed

- *(writers)* bound output growth so self-nesting tables cannot exhaust memory
- *(commonmark)* bound citation affix re-parsing so nested brackets cannot hang the reader
- *(latex)* bound parse nesting depth so deeply nested input cannot
- *(writers)* lay out each typst table cell once per row

### Performance

- *(commonmark)* memoize bracketed-citation resolutions so nesting costs linear work

## [0.0.8](https://github.com/mfkrause/carta/compare/v0.0.7...v0.0.8) - 2026-07-26

### Fixed

- *(readers)* allow the unused xml descendant helper in an epub-only build
- *(writers)* resolve fractional column widths against the fill column, not the sizing width

### Performance

- *(highlight)* load a grammar directory's definitions on demand, not at startup
- *(highlight)* gate every matcher kind on its leading byte, not just regexes
- *(highlight)* reject a regex rule on its leading byte before compiling it
- reserve inline capacity up front and measure ASCII width by length
- *(writers)* keep filled text pieces inline instead of allocating per word
- *(html)* reflow in measured chunks instead of char by char

### Other

- split oversized source files into focused submodules
- resolve clippy and typos findings in the new highlight and writer code
- *(highlight)* cover non-ASCII lead bytes in the first-byte soundness sweep

## [0.0.7](https://github.com/mfkrause/carta/compare/v0.0.6...v0.0.7) - 2026-07-23

### Added

- *(cli)* add cargo-binstall metadata and completion/man-page

### Fixed

- *(writers)* correct table cell boundary-space rendering and sizing
- *(cli)* exit cleanly when stdout pipe is closed early
- *(readers)* compile inline_scan under budget-only feature builds

### Performance

- *(ast)* validate interchange input once and slice strings without re-checking
- *(ast)* hand-rolled JSON interchange codec
- *(highlight)* cache compiled rules and skip capture extraction on group-less patterns
- *(readers)* vectorize delimiter scans with memchr
- *(commonmark)* defer raw-TeX close-delimiter scans until a group scan needs them
- *(readers)* accumulate text runs as slices instead of characters

### Security

- *(cli)* add --sandbox to disable remote resource fetching
- *(media)* confine --extract-media writes to the target directory
- *(readers)* bound man macro expansion by a document-wide budget
- *(writers)* keep nested tables and link labels from compounding renders
- *(writers)* bound AST-controlled table sizing against overflow
- *(writers)* bound heading marker length against absurd header levels
- *(writers)* clamp table column spans to the table's own width
- *(writers)* bound ordered-list numeral length and marker padding

### Other

- *(cli)* cover the flag surface with offline subprocess tests
- *(carta)* add registry-driven format wiring guards
- *(fuzz)* improve fuzz target names
- *(fuzz)* fuzz writers ([#92](https://github.com/mfkrause/carta/pull/92))
- *(commonmark)* split the two largest reader files along module seams
- *(readers)* share the forward-scan budget helper; cap org's budget
- *(readers)* co-locate duplicated reader helpers
- *(writers)* dedupe image, escape, and transliteration helpers
- *(readers)* consolidate smart-typography folds in smart_fold
- *(writers)* gate dead-code allows per feature and remove dead helpers

## [0.0.6](https://github.com/mfkrause/carta/compare/v0.0.5...v0.0.6) - 2026-07-21

### Fixed

- *(writers)* render table-cell boundary spaces, multiline cells, and blank cells consistently
- *(formats)* declare per-format extension sets and report reader defaults
- *(mediawiki)* clamp table cell spans to bound grid allocation ([#83](https://github.com/mfkrause/carta/pull/83))

### Other

- *(golden)* add offline end-to-end reader-to-writer snapshots
- *(fuzz)* build once and iterate targets instead of a 20-way matrix
- *(golden)* partition reader and writer snapshot suites per format
- feature-gate the serde stack behind the format features
- *(docs)* cleanup documentation
- *(readers)* consolidate the ellipsis fold and co-locate the transliteration helpers

### Performance

- *(highlight)* borrow context rules instead of cloning per step
- *(readers)* bound org and jira inline forward scans
- *(mediawiki)* make unterminated-tag scanning linear
- *(commonmark)* bound fence and raw-TeX close lookahead scans
- *(writers)* add streaming fill_into and single-render group placement
- *(math)* key the typst symbol tables by name

## [0.0.5](https://github.com/mfkrause/carta/compare/v0.0.4...v0.0.5) - 2026-07-17

### Added

- *(odt)* implement ODT reader and writer ([#80](https://github.com/mfkrause/carta/pull/80))
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
