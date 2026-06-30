# carta status

Per-format detail behind the [README support matrix](README.md#status). Each format carries its own
status; the README grid is the exhaustive at-a-glance roster. Measured against pinned pandoc
**3.10** (`pandoc-api-version 1.23.1`).

✅ supported · 🚧 partial · ❌ not started · ➖ not applicable (pandoc has no such direction)

---

## Readers

### `commonmark` — ✅
Full CommonMark spec.

### `commonmark_x` — ✅
CommonMark plus the broad [extension](#extensions) preset.

### `gfm` — ✅
CommonMark plus the GFM preset: `strikeout`, `pipe_tables`, `task_lists`, `autolink_bare_uris`,
`footnotes`, `tex_math_dollars`, `gfm_auto_identifiers`, `raw_html`, `emoji`, `alerts`.

### `markdown` — 🚧
The broad Markdown preset on the CommonMark engine. Most of the preset's extensions are honored (see
[Extensions](#extensions)); the remaining gaps are narrow — the per-extension ones are listed under
[known parity gaps](#known-parity-gaps).

| Gap | Detail |
| --- | --- |
| `latex_macros` | not modeled — `\newcommand` / `\def` definitions are neither collected nor expanded |

### `html` — ✅
All block and inline structure. The `html`/`html5`/`html4` [extension](#extensions) defaults
(`auto_identifiers`, `line_blocks`, `native_divs`, `native_spans`) and any `ReaderOptions.extensions`
toggles are honored — including `smart`, the `tex_math_*` families, and `gfm_auto_identifiers`.
Footnotes are reconstructed into `Note` inlines; a `<span class="citation">` round-trips as a
citation `Span` (the same shape the dialect's own reader yields — there is no `Cite` node). The
`raw_tex` and `raw_html` toggles are inert here — inline raw HTML is always preserved regardless. A
body-level `<style>` is kept verbatim as a `RawInline`-bearing paragraph once any sibling — even
whitespace — precedes it; a `<style>` with no preceding sibling is document metadata and is dropped,
as are `<script>` blocks and comments.

### `opml` — ✅
Outline depth → header level; the `text` attribute's inline HTML markup is parsed; `_note` parsed as
CommonMark; metadata (title, author, date).

### `json` — ✅
Pandoc-AST JSON ↔ AST.

### `native` — ✅
Pandoc native AST (full document, block list, or inline list).

### `csv` — ✅
Single table; cells are plain text — the format's full scope.

### `tsv` — ✅
As `csv`, tab-delimited.

### `rst` — 🚧
reStructuredText blocks and inlines: sections, bullet/enumerated/definition/field lists, literal and
line blocks, block quotes, footnotes and citations, hyperlink targets and substitutions, interpreted
roles, and the common directives. A bullet list runs through a change of bullet character; an
enumerated list disambiguates an ambiguous single-letter enumerator (a lone roman-numeral letter
continues a roman list, otherwise it is alphabetic, and a lone `i` opens a roman list), and a
two-line item whose second line is an under-indented run-on reads as a paragraph rather than a list.
Directives carry their common options: `:name:` becomes the identifier, `:class:` adds classes, and
any remaining options become attributes; the `line-block` directive builds a `LineBlock`, `table`
takes its caption from the argument, `math` emits one `Math` per equation (wrapped in a labelled span
when a `:label:`/`:nowrap:` option is set), a `code` block's `:number-lines:` adds the `numberLines`
class and a `startFrom` attribute, a `figure`'s legend paragraphs join its caption, and the
document-level directives (`meta`, `title`, `header`, `footer`, `sectnum`, `target-notes`, …) become
classed divisions. The `image`/`figure` directives derive image attributes from their options:
`:width:`/`:height:` carry a length whose unit decides its rendering (a pixel length truncates to a
whole number, a percentage keeps one decimal, any other unit prints the shortest exact value),
`:scale:` folds into the width and height as a factor, `:align:` becomes an `align-<value>` class, and
the directive's own `:class:` list is repeated with the alignment suffix attached to the last entry;
a `figure` keeps the outer division's classes separate from the inner image's, and its `:name:`
becomes the image identifier. The `role` directive defines a custom interpreted role — with an
optional base role, its own classes, and the `:format:` a `raw` base emits under or the `:language:`
a `code` base highlights as; a chain of custom roles accumulates the classes each link contributes —
and `default-role` sets the role applied to unqualified interpreted text, restoring the standard role
when given no argument. The `include` directive splices
a referenced file's parsed blocks in place, and the substitution directives build replacement text:
`replace` from literal text, `image` from an image, `unicode` from `0x…`/`U+…`/decimal/escaped code
points, and `date` from the current date rendered through an strftime-style pattern. A substitution
reference (`|name|_`), a phrase or simple hyperlink reference, and an indirect hyperlink target
resolve through to their destination in a final pass, so a reference to a name defined later in the
document still resolves and the last definition of a repeated name wins; a reference resolves even
mid-word, a multi-element replacement is wrapped in a span, and a destination URL is percent-encoded.
Every section title is an implicit target referenceable by its text; an internal hyperlink target
carries its identifier onto the block that follows it, a run of such targets all attaching to one
section title with the last taking the identifier and the rest becoming empty spans; a phrase
reference with an embedded destination also defines its label as a target; and a target name may
hold a backslash-escaped colon. Emphasis, strong,
interpreted-text, and reference markup wrapped in matching quotes or angle brackets stays literal
text. Grid/simple tables — including grid cells that span rows or columns, which merge into single
multi-span cells, and a single-column simple table opened by a too-short section overline — and the
`csv-table` and `list-table` directives build a `Table`. `auto_identifiers`
and `gfm_auto_identifiers` supply header slug ids, `ascii_identifiers` folds those ids to ASCII, and
`smart` renders typographic quotes and dashes.
Gaps: the `contents` (table-of-contents) directive emits nothing; the `table` directive's `:widths:`
is not applied to the built table; a definition-list classifier (`term : classifier`) stays part of
the term; doctest blocks (`>>>`) read as ordinary paragraphs.

### `ipynb` — 🚧
Jupyter notebooks (nbformat v4): markdown cells are parsed in the greedy Markdown dialect — a
paragraph absorbs the lines that follow it, except that a list-shaped line opens a fresh list or
paragraph (the `lists_without_preceding_blankline` preset) — ordered lists collapse to a default
number style, and bare URIs and email addresses autolink, each carrying a `uri` or `email` class (the
cell preset also turns on `auto_identifiers`, `gfm_auto_identifiers`, `tex_math_dollars`,
`pipe_tables`, `task_lists`, `strikeout`, `raw_html`, `autolink_bare_uris`, `fenced_code_blocks`,
`backtick_code_blocks`, `intraword_underscores`). Code cells become code blocks carrying their stream / `execute_result` /
`display_data` / `error` outputs; an `application/json` output bundle becomes a `json`-classed code
block whose payload is re-serialized in canonical numeric form. Notebook and cell metadata become attributes, with a scalar value
quoted when a string is number- or boolean-shaped and left bare when it is an actual number or
boolean. An image output is named by the hash of its decoded bytes (the raw payload when it is not
valid base64) and carries its `metadata` entries as sorted image attributes; a raw cell's target
format is taken from its `raw_mimetype`, falling back to `format`. A markdown-cell link reference
definition whose unbracketed destination carries internal spaces is accepted, with each space
percent-encoded. `attachment:` image references and base64 image payloads are decoded.
Gaps: nbformat v3 (worksheets) is reported as an unsupported format rather than read; the reader is lenient where the format is
strict (a stream output with no `name`, a null `execution_count`, or a missing top-level `nbformat`
are accepted rather than rejected); unknown cell and output kinds are silently dropped;
extreme-magnitude numbers may render in rounded or scientific form.

### `mediawiki` — 🚧
MediaWiki wikitext: headings, paragraphs, apostrophe bold/italic emphasis, bullet/numbered/definition
and indent lists, HTML `<ul>`/`<ol>` lists, preformatted and `<source>`/`<syntaxhighlight>` code
blocks, block quotes, horizontal rules, tables (`{| … |}`), internal and external links,
`[[File:…]]`/`[[Image:…]]` embeds, `[[Category:…]]` links, `<ref>` notes, behavior switches,
`<nowiki>`, HTML passthrough, entities, and inline `<math>`. `auto_identifiers` supplies header ids
(lowercased, with punctuation dropped); `gfm_auto_identifiers` switches to the GitHub slug with
hyphen separators, where each emoji contributes its shortname (`🎉` → `tada`); `ascii_identifiers`
reduces the finished id to pure ASCII, stripping accents (`é` → `e`) and dropping non-Latin letters
(`ß`, `Œ`) while leaving the separators around a dropped letter intact. `smart` curls quotation
marks; `east_asian_line_breaks` drops a soft line break that falls between two East Asian wide
characters. A behavior switch (`__NOTOC__`, `__FORCETOC__`, `__NOEDITSECTION__`, …) is recorded as a
boolean document-metadata entry and removed from the text; one that begins a line is removed with the
spaces that follow it so the line is not mistaken for preformatted text, and one inside `<nowiki>`
stays literal. A `[[Category:…]]` link is gathered and rendered as a trailing paragraph of category
links in document order, while a leading-colon `[[:Category:…]]` is an ordinary inline wikilink. A
bare URL autolinks when its scheme is one of the registered URI schemes. A `<ref>` becomes a `Note`
whose body is captured whole even when it spans a blank line or block markup. A `File:`/`Image:`
embed becomes an `Image` inline: the namespace is stripped and spaces become underscores to form the
target, `NNpx`/`NNxNNpx` parameters set width/height attributes, placement and framing keywords
(`thumb`, `frame`, `left`, `border`, …) and `key=value` options are consumed, and the last free
parameter is the caption (the target name when none is given); an option the embed syntax does not
define (a `thumbtime` value, an `upright=` value) makes the embed fall back to an ordinary wikilink.
A lone embed in a block or list item becomes a `Figure` with that caption (`implicit_figures`).
Tables read as a `Table`: rows are separated by `|-`, data cells split on `||`, and header cells
(`!`, split on `!!`) make up the head when the first row begins with a header. A `|+` line is the
caption. A cell's leading attribute list is honored — `align` sets the cell alignment, `colspan`/
`rowspan` set the spans, `id`/`class` populate the cell id and classes, and any other `name="value"`
pair is kept as a cell attribute; a quoted attribute value may contain a `|` without ending the
cell. The table's own and each row's attribute lists are dropped. The
first row fixes the column count: a cell that would overflow it is dropped, a span is clamped to the
remaining columns and rows, a column still covered by a `rowspan` is skipped, and a short row is
padded with empty cells. A nested table is matched by depth so it does not close the outer one early.
Under `smart`, a pair of straight double quotes becomes curly quotation marks (single quotes and
apostrophes are left straight, since `''`/`'''` already mark emphasis).
Gaps: block `<math display=block>` is emitted as inline math;
the `Media:` namespace, leading-colon `[[:File:…]]` links, and interwiki prefixes read as ordinary
wikilinks rather than links to the media file or remote wiki; a mid-paragraph `<pre>`/`<source>`
falls through to HTML passthrough rather than a code block; block structures nested past a fixed
depth degrade to flat text to bound recursion.

### `dokuwiki` — 🚧
DokuWiki markup: headings (any content after the closing `=` run is re-parsed as its own following
block), paragraphs, bold/italic/underline/monospace, bullet and ordered lists, code and file blocks,
quotes, tables, internal/external/interwiki links, media embeds, footnotes (`((…))`),
`<nowiki>`/`%%` escapes, smart quotes, and entities. Tabs expand to four-column tab stops, so a
tab-indented line reads as indented code. List depth follows the indent as `floor(columns/2)`, so a
two- and three-column marker share a level; a first item indented four or more columns, and a line
that jumps more than one level past its predecessor, read as indented code instead. A `<code>` or
`<file>` region occurring mid-paragraph splits the paragraph around a code block (an inline `<HTML>` /
`<PHP>` fragment does not); an unclosed `<code>`/`<file>`/`<HTML>`/`<PHP>` at the start of a line
stays literal text. A monospace `''…''` run requires non-blank content flanked by its markers, and
under smart typography its interior folds straight quotes to curly glyphs when paired, while an
unpaired apostrophe makes the run parse as a quote span. Entities decode in running text, monospace,
and `<sub>`/`<sup>`/`<del>` spans, but stay literal inside link labels, `<nowiki>`, and verbatim
code/file blocks. Overlapping emphasis markers (`**a //b** c//`) pair greedily from the outside, and
a smart quote does not open a second quote of the same kind while one is already open. An empty or
blank `''`, an empty `[[…]]`/`{{…}}`/`((…))`, an empty or whitespace-only `<sub>`/`<sup>`/`<del>`, a
lone `|`/`^` line (a table needs at least one cell), and a `----` carrying any trailing character all
stay literal; an explicit but empty link or media label (`[[page|]]`, `{{img.png|}}`) falls back to
the automatic display text. The reader admits exactly its documented extension set
(`smart`, `tex_math_dollars`, `auto_identifiers`, `gfm_auto_identifiers`, `ascii_identifiers`,
`east_asian_line_breaks`, `raw_html`) and rejects any other toggle. Extensions: `smart` (default on);
`tex_math_dollars` (`$…$` inline and `$$…$$` display math, with a backslash-escaped `\$` kept
literal); the heading-identifier family `auto_identifiers`/`gfm_auto_identifiers` with the
`ascii_identifiers` fold; and `east_asian_line_breaks` (a soft break between two wide characters is
dropped).
Gaps: a footnote closes at the first `))`, so nested parentheses are unbalanced; bare-URL autolinking
requires an explicit `scheme://`.

### `jira` — 🚧
Jira wiki markup: headings, paragraphs, the text effects (strong, emphasis, citation, deleted,
inserted, superscript, subscript, monospace), colored and anchored spans, bullet/numbered lists, the
`{code}`/`{noformat}`/`{quote}`/`{panel}` block macros, tables, links, images, and emoji. Emphasis is
resolved with a flanking delimiter stack (same-marker nesting up to depth two, nearest-opener
pairing), so `*a**b*`, `**x**`, and `--x--` lower to nested spans; smart dashes fold a run hugged by a
following space into en-/em-dashes. A `{color:VALUE}` whose matching close is a line holding only
`{color}` becomes a block-level colored division wrapping its content; otherwise it stays an inline
colored span. A color value may be a name, a `#`-prefixed six-digit hex, or a bare six-digit hex with
a leading decimal digit (normalized to `#`). Inline constructs — colored and monospaced spans, links,
citations — carry across a soft line break, which renders as a line break inside them. A symbol or
emoticon token is recognized even when it abuts a preceding word. A brace block macro that opens
partway through a paragraph ends that paragraph and starts the macro's block. A table cell carries
block structure — bullet and numbered lists and the brace macros nest inside it, while a line whose
prefix names a heading, blockquote, or rule stays paragraph text, and the cell's own paragraphs are
trimmed of surrounding whitespace. A heading or blockquote line whose content holds a bare block
macro degrades to a paragraph that the block layer then splits at the macro. An image's bracketed
property list disqualifies the image when it has leading whitespace or when `thumbnail` is surrounded
by whitespace; attribute values keep their surrounding whitespace verbatim. A `|` nested inside a
bracketed link, a brace span, or an image's property list does not split a table cell. The
`east_asian_line_breaks` extension is recognized (off by default).
Gaps: when a `{quote}` macro's content shares the line with its fences, the leading whitespace of its
first paragraph is kept rather than trimmed.

### `man` — 🚧
roff man pages: section and subsection headings (`.SH`/`.SS`), paragraphs, indented and
tagged-paragraph lists (`.IP`/`.TP`, with `.TQ` attaching further terms to one item) folded into
bullet/ordered/definition lists, font macros (`\fB`, `.B`, `.BR`, …) mapped to strong/emphasis with
constant-width families (`\f(CW`, `\f(CB`, …) becoming inline code, `.SY`/`.OP`/`.YS` command
synopses, `.nf`/`.EX` verbatim regions as code blocks, `tbl` tables (`.TS`/`.TE`) including
`T{ … T}` multi-line cells, hyperlinks (`.UR`/`.MT`), and `.RS`/`.RE` nesting. String definitions
(`.ds`) interpolate through `\*`, alongside the predefined strings and a broad set of composite and
accented special characters (`\(:a`, `\('e`, `\(ss`, `\(la`, …). User macros (`.de`/`.de1`) are
expanded on each call, substituting `\$N` arguments and reducing copy-mode `\\` escapes;
conditionals (`.if`/`.ie`/`.el`) keep the branch whose condition matches the nroff target (`n`) or
the constant `1` and discard the rest; and a no-op control line (a bare control character, `.`, `..`,
or `'`) passes through without breaking fill. Escape handling covers tab, zero-width, and half-line
motion escapes, odd-trailing-backslash line continuation, color/font-family/register/environment
escapes (whose name is discarded), `\C'…'` named glyphs, and the no-output `\p`/`\a`.
`auto_identifiers` supplies header ids; `ascii_identifiers` folds them to ASCII (Latin-1 through
Latin Extended Additional), and `east_asian_line_breaks` is accepted.
Gaps: a single ambiguous list-marker letter (`i.`/`c.`/`v.`/…) classifies as a roman numeral rather
than lower-alpha; `.MR`/`.SM`/`.SB` are dropped; verbatim regions flatten embedded font macros
(though literal tabs are preserved); and a `tbl` table using row or column spans degrades to a
placeholder paragraph.

**Not started:** `asciidoc`, `biblatex`, `bibtex`, `bits`, `creole`, `csljson`, `djot`, `docbook`,
`docx`, `endnotexml`, `epub`, `fb2`, `haddock`, `jats`, `latex`, `markdown_strict`, `markdown_mmd`,
`markdown_phpextra`, `markdown_github`, `mdoc`, `muse`, `odt`, `org`, `pod`, `pptx`, `ris`, `rtf`,
`t2t`, `textile`, `tikiwiki`, `twiki`, `typst`, `vimwiki`, `xlsx`, `xml`.

---

## Writers

Writers render the full AST. The Markdown family branches on the effective `Extensions` set, and the
text writers that have a meaningful toggle honor it (today that is `smart`); the rest emit a fixed
dialect (see [writer extension toggles](#cross-cutting-features)).

### `html` (+ `html5`, `html4`) — ✅
All blocks and inlines. `html4` uses presentational attributes and `div.float` figures.

### `latex` — ✅
All blocks and inlines.

### `beamer` — ✅
LaTeX slides: frames, columns, incremental lists, fragility detection.

### `revealjs` — ✅
HTML slide deck; sections nested by header level.

### `gfm` — ✅
GFM dialect; HTML fallback for non-GFM constructs (divs, citations, attributes).

### `commonmark` — ✅
All blocks and inlines. Figures (and tables) fall back to an HTML block; an image carrying a width or
height falls back to an HTML `<img>`. Math is translated to a Unicode-text approximation, with the
verbatim `$…$` / `$$…$$` source kept only for expressions that cannot be linearized.

### `markdown` — 🚧
Renders every AST node and branches on the effective `Extensions` set, so `+`/`-` toggles and the
sibling dialect presets change output; the unmodeled pandoc-markdown extensions (the reader-only and
`mmd_*` families) remain unavailable.

### `rst` — ✅
All blocks and inlines; grid/simple/multiline tables, figure directives, `:math:` role.

### `typst` — ✅
All blocks and inlines. Math is translated to Typst's native math syntax inside `$…$`, falling back to
the verbatim TeX source only for expressions that cannot be translated.

### `mediawiki` — ✅
All blocks and inlines; HTML fallback where wiki syntax falls short.

### `dokuwiki` — ✅
All blocks and inlines. Math is emitted verbatim as `$…$` / `$$…$$`, the form this wiki passes through
to its TeX plugin.

### `asciidoc` — ✅
All blocks and inlines. Emits the `asciidoc` flavor only — `asciidoc_legacy` / `asciidoctor` not
implemented.

### `jira` — ✅
All blocks and inlines. Math is translated to a Unicode-text approximation, keeping the verbatim TeX
source only where an expression cannot be linearized.

### `man` — ✅
All blocks and inlines. Math is translated to a Unicode-text approximation rendered with roff escapes,
keeping the verbatim TeX source only where an expression cannot be linearized.

### `plain` — ✅
All blocks and inlines. Math is translated to a Unicode-text approximation, keeping the verbatim TeX
source only where an expression cannot be linearized.

### `opml` — ✅
Header outline; body serialized to Markdown in `_note`. Lossy by the format's nature.

### `json` — ✅
AST → Pandoc JSON.

### `native` — ✅
AST → native literals.

### `ipynb` — 🚧
AST → Jupyter notebook (nbformat v4): the document is split into markdown and code cells, code cells
carrying their outputs, with document and cell metadata serialized from attributes. A raw cell (a
`[cell, raw]` div wrapping a raw block) emits a `raw_mimetype` derived from the raw block's format,
including `asciidoc` → `text/asciidoc`, so a raw cell round-trips through the reader. Cell ids are
derived deterministically from cell content so output stays byte-reproducible.
Gaps: an image output references its payload by file name (the document model carries no embedded
bytes), so its base64 `data` bundle cannot be reconstructed — such an output is reported as
unrepresentable rather than emitted as a broken bundle; nested metadata keys (e.g. `kernelspec`)
emit in sorted order rather than the format's hash order; standalone (`-s`), TOC, and section
numbering are no-ops.

**Not started:** `ansi`, `asciidoc_legacy`, `asciidoctor`, `bbcode` (+ `_fluxbb`, `_hubzilla`,
`_phpbb`, `_steam`, `_xenforo`), `biblatex`, `bibtex`, `chunkedhtml`, `context`,
`csljson`, `docbook` (+ `4`, `5`), `docx`, `dzslides`, `epub` (+ `2`, `3`), `fb2`, `haddock`,
`icml`, `jats` (+ `_archiving`, `_articleauthoring`, `_publishing`), `markua`, `ms`, `muse`,
`odt`, `opendocument`, `org`, `pdf`, `pptx`, `s5`, `slideous`, `slidy`, `tei`, `texinfo`, `textile`,
`vimdoc`, `xml`, `xwiki`, `zimwiki`.

---

## Extensions

Reader-side toggles the CommonMark engine recognizes. The enum defines 77 extension names. The reader
branches on the behavioral set listed under **Supported**; every other name is still accepted as a
toggle, so a format spec naming one parses and records it rather than aborting, even where the
construct's behavior is not yet modeled (see **Recognized, behavior not yet modeled**). In the bare
CommonMark engine `raw_html` is always honored — raw HTML is part of the core grammar — whereas in the
Markdown dialect the toggle is live: with it off, an HTML block degrades to a paragraph of its literal
text and an inline tag is kept as ordinary text.

**Supported:** `smart`, `strikeout`, `superscript`, `subscript`, `pipe_tables`, `footnotes`,
`task_lists`, `autolink_bare_uris`, `tex_math_dollars`, `fenced_divs`, `bracketed_spans`,
`hard_line_breaks`, `raw_html`, `header_attributes`, `fenced_code_attributes`,
`inline_code_attributes`, `link_attributes`, `attributes`, `definition_lists`, `grid_tables`,
`multiline_tables`, `simple_tables`, `table_captions`, `line_blocks`, `fancy_lists`,
`example_lists`, `startnum`, `yaml_metadata_block`, `pandoc_title_block`, `auto_identifiers`,
`gfm_auto_identifiers`, `implicit_header_references`, `implicit_figures`, `raw_attribute`,
`inline_notes`, `native_divs`, `native_spans`, `markdown_in_html_blocks`, `raw_tex`, `citations`,
`table_attributes`, `blank_before_blockquote`, `blank_before_header`, `mark`, `emoji`, `alerts`,
`tex_math_single_backslash`, `tex_math_double_backslash`, `lists_without_preceding_blankline`,
`intraword_underscores`, `backtick_code_blocks`, `fenced_code_blocks`, `escaped_line_breaks`.

In the Markdown dialect `escaped_line_breaks` is off by default: a backslash before a line ending is
literal text and the break is soft. With it on, that backslash makes a hard line break. `auto_identifiers`
is the master switch for header numbering — with it off, a header keeps an empty identifier even when
`gfm_auto_identifiers` is on (the latter only selects the slug algorithm). Under
`lists_without_preceding_blankline`, a definition marker (`:`/`~`) or an example marker (`(@)`) on the
line after a paragraph opens a fresh block even when `definition_lists` / `example_lists` are off, so
the greedy paragraph ends there.

### Known parity gaps

Constructs the supported extensions read, each with one narrow case that still diverges. Every entry
is verified against the pinned oracle and tracked for a follow-up.

| Extension(s) | Gap |
| --- | --- |
| `raw_tex` | A raw-TeX environment (`\begin{…}…\end{…}`) that interrupts an open paragraph with no blank line leaves that paragraph as `Para` rather than tightening it to `Plain`. The free-standing form — a blank line before the environment — is exact. |
| `markdown_in_html_blocks` | A block-level HTML element (`<div>`, `<section>`, `<table>`, …) that interrupts an open paragraph tightens it to `Plain`. Two narrower forms still diverge: a raw-text element (`<pre>`, `<script>`) interrupting a paragraph leaves it as `Para` rather than `Plain`, and an inline-level construct (`<style>`, a comment, a doctype, or a processing instruction) interrupting a paragraph is folded into it as raw inline by the dialect, whereas carta opens a separate raw block. |
| `markdown_in_html_blocks` | An HTML block left open at end of input — a `<!-- …` comment or a `<table>`/`<div>` with no close tag — is reparsed as ordinary paragraphs by the dialect; carta keeps the whole run as one raw block. |
| `native_spans` | An emphasis run that opens before a `<span>` and whose closing marker sits just inside the matching `</span>` can leave both tags raw instead of forming a span. |
| `raw_tex` | Inline `\command{…}[…]` consumes every group that follows it; commands that take a fixed number of arguments and leave the rest as text are not modeled. A `\begin{env}…\end{env}` is recognized only as a whole paragraph (block level); inline, each `\begin`/`\end` is an ordinary command. |
| `citations` | An abbreviation-led citation suffix such as `p. 5` is a single string in the dialect (a non-breaking space follows the period); carta splits it into separate tokens. |
| `attributes` | An attribute spec `{…}` containing a backslash escape is void in the dialect — it stays literal text — whereas carta accepts the backslash into the id, class, or value. |
| `alerts` | An alert marker indented two or more columns inside its blockquote (e.g. `>  [!NOTE]`) is still read as an alert; the dialect treats only a marker at column 0 or 1 as one. |

### Recognized, behavior not yet modeled

These names have an enum variant, so a format spec may toggle them and the toggle is recorded, but the
reader does not yet branch on the construct (notable, non-exhaustive): `latex_macros`, `abbreviations`,
`wikilinks_title_after_pipe`, `wikilinks_title_before_pipe`, `ascii_identifiers`, `mmd_title_block`,
`mmd_header_identifiers`, `mmd_link_attributes`, `markdown_attribute`, `short_subsuperscripts`,
`old_dashes`, `east_asian_line_breaks`, `four_space_rule`, `space_in_atx_header`, `literate_haskell`,
`rebase_relative_paths`, `gutenberg`, `all_symbols_escapable`, `angle_brackets_escapable`,
`ignore_line_breaks`, `raw_markdown`, `spaced_reference_links`.
(`shortcut_reference_links` is already covered by the CommonMark engine.)

---

## Cross-cutting features

Document-conversion features independent of any single format.

| Feature | Status | Notes |
| --- | :---: | --- |
| Standalone output + templates (`-s`, `--template`) | ✅ | a built-in template engine (conditionals, loops, partials, pipes) drives a default template per writer; title/author/date and the format's identity variables are populated from metadata. User templates via `--template`. Each writer's scaffold (CSS, preamble) is authored independently and not byte-identical across tools |
| Table of contents (`--toc`) | ✅ | `--toc`/`--table-of-contents` builds a nested contents list from the headings, limited by `--toc-depth` (default 3, valid range 1–6). HTML and Markdown render the list into the `$toc$` slot with `#`-anchored back-reference links; GFM and CommonMark link without the anchor, since their dialect cannot carry a link id. LaTeX, Beamer, Typst, reStructuredText, and AsciiDoc instead set a boolean `toc` flag and let the format assemble its own contents (`\tableofcontents`, `#outline`, `.. contents::`, `:toc:`). Slide decks (`revealjs`) do not yet emit the slide-relative contents structure |
| Text wrapping (`--wrap`, `--columns`) | ✅ | `--wrap=auto\|none\|preserve` and `--columns` (the fill width, default 72) are honored by every writer that lays out lines. A few constructs still account for width incorrectly when reflowed at narrow columns — line blocks, footnote bodies, the AsciiDoc list-marker indent, and roff line-continuation — and grid-table column widths split the budget evenly rather than proportionally |
| Section numbering (`--number-sections`) | ✅ | `-N`/`--number-sections` numbers headings `1`, `1.1`, …, anchored at the document's shallowest heading level (a deeper heading appearing first reads as `0.1`). HTML numbers in the rendered body (`<span class="header-section-number">`); LaTeX, Beamer, Typst, and reStructuredText switch on the format's own numbering. The Markdown family and `plain` have no heading-number syntax, so the flag is inert there, and slide decks (`revealjs`) are not yet numbered |
| Metadata / variables (`-M`, `-V`, `--metadata-file`) | ✅ | `-M`/`--metadata-file` set document metadata, `-V` sets template variables; effective precedence is `-V` over `-M` over the document's front matter over `--metadata-file` defaults |
| Syntax highlighting (`--highlight-style`) | ❌ | code emitted verbatim |
| Citations / citeproc (`--citeproc`) | ❌ | `Cite` carried in AST, not processed |
| Filters (Lua / JSON) | ❌ | |
| Math output methods (MathJax, KaTeX; MathML, webtex, plain HTML) | 🚧 | `--mathjax` and `--katex` select the HTML math renderer: MathJax wraps inline and display TeX in `\(…\)` / `\[…\]` inside `<span class="math">`, KaTeX emits the bare TeX; standalone output pulls in the matching loader script. With no method given, HTML math keeps the verbatim TeX in a math span rather than rendering it to plain HTML markup. Elsewhere TeX passes through verbatim where the target accepts raw TeX (latex, rst, asciidoc, mediawiki, dokuwiki) and is otherwise translated to the target's native math — Typst's native syntax for `typst`, a Unicode-text approximation for `commonmark`/`plain`/`man`/`jira`. No MathML, webtex, or default plain-HTML renderer yet |
| Writer extension toggles | ✅ | the effective `Extensions` set (`default_extensions(base)` ± `+`/`-` toggles, unioned in by `convert`) drives output. The Markdown engine is fully extension-driven: `markdown`/`gfm` reproduce their prior output byte-for-byte, the `commonmark_x`/`markdown_strict`/`markdown_mmd`/`markdown_phpextra`/`markdown_github` dialects are their default presets, and per-extension toggles (`-fenced_divs`, `-strikeout`, `+definition_lists`, …) take effect. `smart` is honored by `latex`, `beamer`, `rst`, `plain`, and `typst` (quotes and dashes render as the format's ligature/straight spellings under `+smart`, as literal Unicode under `-smart`); the per-format default lives in `default_extensions` (`latex`/`beamer`/`typst` default on, `rst`/`plain` off). Inert where the toggle changes nothing or the format rejects it: `html`/`html4`, `mediawiki`, `dokuwiki`, `opml`, `native`, `asciidoc`, `jira`, `man`, `revealjs`. One deferral: `rst` does not yet backslash-escape literal ASCII `--`/`...` under the non-default `+smart` |
| Embed resources / extract media | ❌ | |
| Multiple inputs / defaults files (`--defaults`) | ❌ | CLI takes one input |
| CLI introspection (`--list-input-formats`, `--list-extensions`, …) | ✅ | `--list-input-formats`, `--list-output-formats` (canonical names and aliases), `--list-extensions[=FORMAT]` (`+`/`-` per the format's default set; the Markdown dialect when no format is given) |
