# carta status

Per-format detail behind the [README support matrix](../README.md#status); the README grid is the
at-a-glance roster. Measured against pinned pandoc **3.10** (`pandoc-api-version 1.23.1`).

✅ usable (basically done, any remaining parity gaps are minor and unlikely to affect regular use) · 🚧 in development (large parity gaps or breaking issues, not recommended for use yet) · ❌ not started · ➖ not applicable (pandoc has no such direction)

Each entry lists only what is still missing or known to diverge. An entry with no list has no tracked
gap.

---

## Readers

### `commonmark` — ✅
### `commonmark_x` — ✅
### `gfm` — ✅

### `markdown` — ✅
- `latex_macros` not modeled: `\newcommand` / `\def` are neither collected nor expanded.
- Narrow per-extension divergences — see [known parity gaps](#known-parity-gaps).

The four dialect readers below share the `markdown` reader engine, gating each construct on the
dialect's own default extension set, so the `markdown` notes above apply to them as well.

### `markdown_strict` — ✅
- The sparsest dialect: raw HTML plus the shortcut and spaced reference-link forms, and nothing else.

### `markdown_github` — ✅
- A task-list item followed by an ordinary bullet item splits into two lists rather than staying in
  one.

### `markdown_phpextra` — ✅
- An inline attribute block that trails a bracketed group which is not a link (`[text]{.class}`) is
  kept as literal text; the dialect consumes and discards it.

### `markdown_mmd` — ✅
- An empty sub/superscript delimiter pair (`x^^y`, `a~~b`) is not read as an empty span.
- A reference definition's trailing attribute tail (after the URL and optional title) is not parsed.
- An implicit header reference does not resolve against a header whose identifier was set by the
  trailing `[id]` syntax.

### `html` — ✅
### `opml` — ✅
### `json` — ✅
### `native` — ✅
### `csv` — ✅
### `tsv` — ✅

### `rst` — ✅
- The `contents` (table-of-contents) directive emits nothing.
- The `table` directive's `:widths:` is not applied to the built table.
- A definition-list classifier (`term : classifier`) stays part of the term.
- Doctest blocks (`>>>`) read as ordinary paragraphs.

### `ipynb` — ✅
- nbformat v3 (worksheets) is reported as an unsupported format rather than read.
- Lenient where the format is strict: a stream output with no `name`, a null `execution_count`, or a
  missing top-level `nbformat` are accepted rather than rejected.
- Unknown cell and output kinds are silently dropped.
- Extreme-magnitude numbers may render rounded or in scientific form.

### `mediawiki` — ✅
- Block `<math display=block>` is emitted as inline math.
- The `Media:` namespace, leading-colon `[[:File:…]]` links, and interwiki prefixes read as ordinary
  wikilinks rather than links to the media file or remote wiki.
- A mid-paragraph `<pre>`/`<source>` falls through to HTML passthrough rather than a code block.
- Block structures nested past a fixed depth degrade to flat text to bound recursion.
- An unterminated `<ref>` (no `</ref>` before end of input) is read as a raw HTML inline; pandoc
  reads it as literal text. The unterminated-tag corpus case therefore pins only the open-tag
  failure shapes, and the `<ref>` path is pinned by unit tests instead.

### `dokuwiki` — ✅
- A footnote closes at the first `))`, so nested parentheses are unbalanced.
- Bare-URL autolinking requires an explicit `scheme://`.

### `jira` — ✅
- When a `{quote}` macro's content shares the line with its fences, the leading whitespace of its
  first paragraph is kept rather than trimmed.

### `man` — ✅
- A single ambiguous list-marker letter (`i.`/`c.`/`v.`/…) classifies as a roman numeral rather than
  lower-alpha.
- `.MR`/`.SM`/`.SB` are dropped.
- Verbatim regions flatten embedded font macros (literal tabs are preserved).
- A `tbl` table using row or column spans degrades to a placeholder paragraph.

### `latex` — ✅
- Only the `\begin{document}`…`\end{document}` body is rendered; the preamble contributes metadata
  (`\title`, `\author`, `\date`, `\subtitle`, `abstract`) and macro definitions but is otherwise
  dropped. A file with no `document` environment is read whole. `\institute` is dropped entirely.
- With `raw_tex` off (the default), an unknown command is dropped together with its bracket and brace
  arguments and an unknown environment becomes a classed `Div`; under `raw_tex` both are preserved
  verbatim as raw LaTeX.
- Macro expansion (`\newcommand`, `\renewcommand`, `\providecommand`, `\DeclareRobustCommand`, `\def`,
  `\let`) handles numbered parameters and one optional-argument default and is bounded to a fixed
  nesting depth; delimited parameters, `\csname`/`\expandafter`, catcode changes, and recursive or
  multi-optional macros are unsupported. With `latex_macros` off, each definition is left verbatim as a
  raw block.
- Cross-reference commands (`\ref`, `\eqref`, `\autoref`, `\cref`, `\pageref`) resolve to a link whose
  visible text is a bracketed `[label]` placeholder rather than the target's counter number.
- Multi-line math environments (`align`, `gather`, `multline`, `eqnarray`, `alignat`, `flalign`, …) are
  kept as a single display-math inline holding the whole `\begin…\end` text rather than split into rows
  and cells.
- `\includegraphics` keeps only `width` and `height`; `\textwidth`/`\linewidth`/`\textheight` lengths
  convert to percentages and absolute units pass through, but `\columnwidth`/`\paperwidth` and
  leading-dot decimal values are omitted.
- Table support covers header detection, per-column alignment, and `\multicolumn` colspan; `\multirow`,
  partial `\cline` rules, nested tables, and `\caption` placement may flatten or drop.
- `\raisebox` in its optional depth/height form drops the box content; purely visual commands
  (font-size macros, `\bfseries` scoping, spacing macros) drop styling and keep only the inner content.

### `org` — ✅
- Drawers: a headline's property drawer is consumed and supplies its `CUSTOM_ID` as the heading
  identifier; the bookkeeping drawers `:PROPERTIES:` and `:LOGBOOK:` are dropped wherever they appear;
  every other named drawer becomes a `Div` classed with the drawer name.
- A property drawer's `:ID:` is not used as a fallback heading identifier when no `:CUSTOM_ID:` is
  present, and a file-level property drawer's keys are not promoted to document metadata.
- An internal `[[target]]` radio link resolves to a bare destination rather than an anchor.

### `rtf` — ✅
- A `\mac`, `\pc`, or `\pca` document's `\'xx` bytes decode as Windows-1252 rather than the declared
  character set.
- A control byte delivered through a `\'xx` or `\uN` escape (tab, line feed) is kept as a literal
  character rather than normalized to a space or line break.
- `\softline` renders as a hard line break rather than being dropped.
- Formatting inside an `\info` field is flattened to plain text, and `\generator` is not captured as
  document metadata.

### `docx` — ✅
- Page and column breaks are dropped rather than converted to line breaks, and a VML textbox's text
  (including an `mc:AlternateContent` fallback) is not extracted.
- Office Math sets binary and relational operators tight, with no surrounding thin space
  (`a^{2}+b^{2}=c^{2}` rather than `a^{2} + b^{2} = c^{2}`).
- A few math symbols take a less idiomatic TeX spelling (`→` becomes `\to` rather than `\rightarrow`;
  `·` is kept literal rather than `\cdot`).

### `epub` — ✅

### `odt` — ✅
- A field element with cached display text (`text:date`, `text:page-number`, `text:page-count`,
  `text:author-name`, `text:title`, `text:chapter`, `text:file-name`, `text:sequence-ref`,
  `text:note-ref`) keeps its cached text as `Str` runs rather than being dropped.
- A `text:number` cached list-numbering label inside a list-item paragraph leaks into the item text
  rather than being dropped.
- A `text:meta` in-content metadata wrapper leaks its displayed prose rather than dropping it.
- A block element misnested in inline context (`text:list`, a nested `text:p`, a nested `text:a`)
  leaks its text into the surrounding prose rather than being dropped.
- A `text:bibliography-mark` inline citation flattens to its display text rather than becoming a
  `Cite`.
- `text:ruby` glues the ruby base and ruby text into one run rather than dropping the annotation.
- `text:continue-numbering="true"` on a following ordered list is ignored, so its numbering restarts
  at 1 rather than continuing.
- A `text:numbered-paragraph`'s inner paragraph is lifted into the body rather than the whole
  numbered paragraph being dropped.
- An unrecognized `style:num-format` on a numbered list level yields a decimal `OrderedList` rather
  than falling back to a `BulletList`.
- An `OrderedList` `text:start-value` that overflows `i32` falls back to start 1 rather than the
  parsed value.
- A caption-styled paragraph adjacent to a table is emitted as a plain `Para` rather than a `Div`
  classed `caption`.
- A captioned figure keeps the leading `Figure N:` / `Illustration N:` label rather than stripping
  it.
- A `text:section`'s content is lifted in place rather than wrapped in a `Div`.
- Block content (a list, table, or heading) inside a footnote/endnote body is kept rather than
  reduced to paragraph flow.
- A `text:h` heading inside a table cell is kept as a `Header` rather than dropped.
- A table nested inside a table cell is kept rather than dropped.
- A `table:table` nested directly in a `text:list-item` is lifted into the item rather than dropped.
- Oversized `table:number-columns-spanned` / `number-rows-spanned` is not clamped, so the grid width
  is taken as the sum of colspans and rows go ragged.
- A `text:line-break` inside a preformatted paragraph becomes a newline in the code block rather than
  a single space.
- A void inline element (`text:soft-page-break`, `text:bookmark-end`, an image-less `draw:frame`)
  flanked by whitespace leaves both flanking spaces uncollapsed.
- A form feed (U+000C) in paragraph text collapses to a break rather than being preserved.
- A `text:s` with a malformed `text:c` count falls back to one space rather than none.
- A `draw:image` with `office:binary-data` and no `xlink:href` emits an `Image` with an empty URL
  rather than being dropped.
- An image wrapped in a hyperlink (`draw:frame` inside `text:a`) keeps the `Image` inside the `Link`
  rather than dropping it.
- A package-relative hyperlink target with a leading `../` is kept verbatim rather than stripped by
  one level.
- Nested `text:span` character styles are not normalized: identical nested styles are not coalesced
  and ancestor styles are not re-applied to inner spans (renders identically).
- `fo:font-weight` step weights 100–600 are not read as `Strong` (only `bold` and ≥700 are),
  diverging on the 500/600 semibold steps.
- An auto-generated heading id for a `text:h` inside a `text:list-item` differs in its dedup suffix.
- A `text:h` `outline-level` that overflows `i32` falls back to level 1 rather than the parsed value.
- A `content.xml` automatic style colliding by name with a `styles.xml` shared style overrides it
  rather than the shared definition winning.
- `ReaderOptions` are ignored, so `east_asian_line_breaks` has no effect (a soft break between CJK
  runs is kept).
- Deeply nested (~62+) `json`→`odt` input is rejected by the shared recursion guard rather than
  converted.
- MathML `math:mfenced` loses its visible open/close delimiters (no `\left`/`\right`).
- A `mathvariant` on `math:mi` / `math:mstyle` is ignored, dropping the styled variant.
- A function-name script base is wrapped in redundant braces (`{\sin}^{2}` rather than `\sin^{2}`).
- MathML embedded directly as an inline math child of `draw:object` is read as `Math` rather than
  ignored.
- Document metadata (`meta.xml`) is not captured.
- A cross-reference or other unrecognized inline field degrades to its display text; the dynamic
  target or number is not resolved.
- A reference-mark point and its end emit nothing; only a reference-mark start becomes an empty
  `Span` carrying the mark name.
- A stray block-level `text:note` (a note definition anchored in the body with no reference) is
  dropped.
- An unknown block container is made transparent (its children are lifted); soft page breaks,
  sequence declarations, forms, and tracked-changes markers are dropped.
- A frame with no image child and no embedded formula (a text box or other embedded object) is
  dropped rather than represented.
- Images carry empty alt text; `svg:title` (slugged) becomes the title, `svg:desc` is unused, and
  width and height pass through as raw length strings.
- Underline is represented as `Emph` (no `Underline` node); small caps and text-transform are not
  represented.
- Tables degrade: column widths and cell alignment are always default, output is a single body with
  an empty foot and caption, and merged (covered) cells are skipped rather than represented as spans.
- List numbering outside the recognized set (`i`/`I`/`a`/`A`; suffix `)`/`.`; prefix-and-suffix
  `()`) falls back to decimal with the default delimiter, and a number level with no `num-format` is
  treated as a bullet.
- An `<mspace>` width given in neither an `em` length nor a named math space (`ex`, `px`, and the
  like) contributes no spacing.

**Not started:** `asciidoc`, `biblatex`, `bibtex`, `bits`, `creole`, `csljson`, `djot`, `docbook`,
`endnotexml`, `fb2`, `haddock`, `jats`, `mdoc`, `muse`, `pod`, `pptx`, `ris`,
`t2t`, `textile`, `tikiwiki`, `twiki`, `typst`, `vimwiki`, `xlsx`, `xml`.

---

## Writers

### `html` (+ `html5`, `html4`) — ✅
### `latex` — ✅
### `beamer` — ✅

### `revealjs` — ✅
- No slide-relative table of contents.
- Headings are not section-numbered.

### `gfm` — ✅
### `commonmark` — ✅

### `markdown` — ✅
- The reader-only and `mmd_*` pandoc-markdown extension families are not modeled.

### `rst` — ✅
### `typst` — ✅
### `mediawiki` — ✅
### `dokuwiki` — ✅

### `asciidoc` — ✅
- Emits the `asciidoc` flavor only (`asciidoc_legacy` / `asciidoctor` not implemented).

### `jira` — ✅
### `man` — ✅
### `plain` — ✅

### `opml` — ✅
- Lossy by the format's nature: the body is serialized to Markdown inside `_note`.

### `org` — ✅
- A `Div` marked as a drawer is written back as a `:NAME:` … `:END:` drawer.

### `json` — ✅
### `native` — ✅

### `ipynb` — ✅
- Nested metadata keys (e.g. `kernelspec`) emit in sorted order rather than the format's hash order.
- Standalone (`-s`), TOC, and section numbering are no-ops.

### `epub` (+ `epub2`, `epub3`) — ✅
- EPUB 2 wraps content in XHTML 1.1, so a few constructs (a list `start` attribute, a `mark` or `u`
  element, block content in a table caption, an empty table, a task-list checkbox) are represented
  only under EPUB 3's XHTML5 content model.
- A resource that cannot be fetched offline — a remote image, an absent local image, or a link to a
  nonexistent target — yields a dangling reference.

### `docx` — ✅
- A metadata keywords list is joined with `; ` in the core properties rather than the `, ` that
  part's convention uses.
- A `lang` value carried by a span or div is dropped rather than set as a run language property.
- A `dir=rtl` carried by a span or div is dropped rather than set as a run right-to-left property,
  with a paragraph bidi property added when it sits on a div.
- An image wrapped in a link takes the hyperlink character style on its picture run, which a bare
  picture run does not carry.
- The same image file referenced several times is embedded once per reference — a separate media
  part and relationship each — rather than deduplicated to a single shared part.
- An empty fenced code block emits a stray empty verbatim run inside its paragraph rather than
  leaving the paragraph with no run.
- A paragraph that ends in an inline equation — whether the equation renders to Office Math or, when
  it cannot be parsed, falls back to its raw dollar-delimited text — omits the trailing
  zero-width-space run that anchors such a paragraph.
- Under `native_numbering`, a figure or table whose caption is present but empty (a `Plain` or `Para`
  holding no inline content, which non-Markdown readers can produce) emits no caption at all rather
  than the numbered caption label.
- `\phantom` renders as its raw dollar-delimited text rather than an invisible phantom spacing box.
- `\overparen` and `\underparen` render as their raw dollar-delimited text rather than a
  group-character parenthesis set over or under the base.
- The `\limits` and `\nolimits` modifiers on a big operator or integral are ignored, so limit
  placement stays at the operator's default (an integral keeps side scripts, a sum keeps under/over
  scripts).
- `\mathrel` wraps its argument in a plain run rather than an operator-emulation box, so the
  surrounding spacing differs.

### `odt` — ✅
- Table cell alignment styles are keyed only by (header, alignment), so columns sharing an alignment
  collapse to one style rather than one per column per section.
- A table or figure caption whose top-level block is a list, `CodeBlock`, or `Header` is dropped;
  only `Para`, `Plain`, `Div`, and `BlockQuote` content is emitted.
- A cell with a row span greater than 1 emits `table:number-rows-spanned` even when a single row
  remains, extending the span past the table body.
- With an empty header row the unreferenced heading-parented alignment style is not emitted (only the
  referenced content-parented one is).
- Table column style names beyond 26 columns use spreadsheet AA/AB naming rather than a raw ASCII
  offset past Z.
- An empty table cell is serialized self-closed rather than as an explicit open/close pair.
- Table structural markup is emitted compact with no whitespace between structural tags rather than
  indented.
- List structural markup is emitted compact with no inter-tag whitespace rather than indented.
- Tightness is decided from the first list item, so a list whose first item is empty is wrongly
  detected as tight.
- A non-default ordered or example list reuses the generic `L1` auto-style name rather than a
  distinct numbering-style name.
- With two or more non-default numbered lists the automatic list-style definitions are emitted in
  forward creation order rather than reverse (the references are equivalent).
- The ordered-list start value is placed on the first `text:list-item` rather than on the
  `text:list`.
- `DefinitionList` tightness is decided from the first item and applied to the whole list rather than
  per item.
- A `DefinitionList` item with an empty definition body is wrongly detected as tight, giving the
  term a tight style and a spurious empty definition paragraph.
- Nested `BlockQuote`s reuse the base `Quotations` style with no incremental margin, so deeper levels
  are not progressively indented.
- A paragraph whose inlines are all whitespace or all non-opendocument raw is dropped entirely rather
  than kept as an empty paragraph.
- With `empty_paragraphs` on, an empty `Plain []` emits an extra empty paragraph rather than being
  suppressed.
- A paragraph following a dropped non-opendocument `RawBlock`, or a flowing paragraph after a
  `Figure`, is styled `First_20_paragraph` rather than `Text_20_body` (renders identically).
- The first flowing paragraph inside a table cell is not treated as section-opening, so after a
  `DisplayMath` split its leading text gets `Text_20_body` rather than `First_20_paragraph`.
- When a `DisplayMath` breaks a paragraph inside a fixed-style container (blockquote, table cell,
  definition), the split formula paragraph uses the flowing body style rather than the container
  style.
- An empty `CodeBlock` emits a stray empty preformatted paragraph rather than nothing.
- A `LineBlock` whose only content is empty lines emits a spurious empty paragraph rather than
  nothing.
- An empty `Note []` emits a stray empty footnote paragraph rather than a self-closed note body.
- A footnote nested inside another footnote is numbered sequentially rather than reusing the outer
  counter.
- A `Figure` with no caption uses the `FigureWithCaption` style rather than the caption-less `Figure`
  style.
- Header levels outside 1–6 clamp the style name (level 7/8 → `Heading_20_6`, level 0 →
  `Heading_20_1`) while keeping the outline level.
- `native_numbering` is not implemented, so figure and table captions emit no `text:sequence`
  numbering field.
- A `Link` with an empty target gets a `../` prefix rather than an empty `xlink:href`.
- `xrefs_name` / `xrefs_number` are unmodeled, so requesting either aborts the conversion rather than
  rewriting the internal link to a reference field.
- The `--toc` `text:table-of-content` carries a spurious `text:name="Table of Contents1"` attribute.
- A `Span` or `Div` carrying a `lang` does not emit an `fo:language` text style; the run is emitted
  plain.
- A `Span` with class `mark` is not mapped to the highlighted character style; its text is emitted
  plain.
- A `RawInline`/`RawBlock` with the non-canonical format token `odt` is passed through (only
  `opendocument` should pass).
- An internal or trailing run of two or more verbatim spaces in `Code`/`CodeBlock` is emitted as a
  single `text:s` covering the whole run rather than one literal space plus a `text:s` for the
  remainder.
- A double-quote in XML text content is emitted literally rather than as `&quot;`.
- A `pHYs` density chunk is ignored, so a non-72-dpi PNG renders at the wrong physical size (only
  JPEG JFIF density is read).
- A degenerate 1×1 PNG is sized at its intrinsic 1pt rather than the 100pt fallback.
- A pathological (negative or non-finite) image width or height is emitted verbatim rather than
  falling back to the intrinsic size.
- A `data:` image with an empty or absent media type is stored with a derived `.plain` extension
  rather than none, and an undecodable-base64 or `text/plain` data URI drops the image.
- A title-block author given as a YAML map is dropped rather than stringified into an author
  paragraph.
- A title-block metadata field authored as a YAML sequence joins its items with a space rather than
  concatenating them.
- Package XML serialization differs: the XML-declaration encoding case, the manifest file-entry
  order, and attribute order across the manifest, metadata, and formula parts.
- An empty `<office:scripts/>` element is always emitted rather than omitted.
- Attribute ordering inside a combined-decoration text style groups per decoration rather than all
  `fo:*` attributes first.
- When math fails to convert, the fallback emits the bare TeX and drops its `$` delimiters.
- Bare fence delimiters `( ) [ ] { } |` are emitted as plain `math:mo` with no `stretchy="false"`,
  so they grow to the enclosed height.
- Math accents duplicate `accent="true"` onto the `mover` and use a combining mark rather than the
  spacing accent glyph on the `mo` alone.
- A named math function (`\sin`, `\log`, and the like) emits a bare `mi` with no
  `mathvariant="normal"` and no function-application operator.
- `\operatorname{…}` is emitted as `mtext mathvariant="normal"` rather than `mi mathvariant="normal"`.
- Some math symbols are placed in the wrong MathML element class (`mi` vs `mo`) for logic, binary,
  and relation operators.
- An uppercase Greek letter emits a bare `mi` with no `mathvariant="normal"`, rendering slanted
  rather than upright.
- The `fr1`/`fr2` formula graphic styles are emitted only when a formula is present rather than
  always.
- `\dfrac` and `\tfrac` embed as a plain fraction without the display-style wrapper that would set
  their size apart.
- The infix `\bmod`/`\mod` operand sits beside the `mod` operator group rather than inside it.
- `--number-sections` is a no-op: headings carry no computed section numbers.
- Syntax highlighting is not applied: code blocks emit plain preformatted lines with no per-token
  styling.
- Image intrinsic sizing reads only PNG, GIF, and JPEG pixel dimensions; other formats fall back to
  attribute-supplied or default sizing under a fixed DPI.
- Table relative column widths are approximated as a percentage rel-width plus rounded integer
  proportions.
- Custom styles (via the `styles` extension) reference the named style only; their definitions must
  come from an external reference document.
- `styles.xml` is authored with a neutral style-name scheme; no reference-document engine ships.
- Metadata mapping is limited to title, subtitle, author, and date; arbitrary keys are not emitted.
- Writer options with no ODT analogue (wrap and column width, highlight theme, ascii-only) are
  silently no-ops.

### `rtf` — ✅
- A table nested inside another table's cell carries a single `\intbl` on its cell paragraphs
  regardless of nesting depth, so the inner table is not set off from its container.
- A nested ordered list with default style is numbered with decimal markers at every level rather
  than cycling the marker style by depth.

**Not started:** `ansi`, `asciidoc_legacy`, `asciidoctor`, `bbcode` (+ `_fluxbb`, `_hubzilla`,
`_phpbb`, `_steam`, `_xenforo`), `biblatex`, `bibtex`, `chunkedhtml`, `context`,
`csljson`, `docbook` (+ `4`, `5`), `dzslides`, `fb2`, `haddock`,
`icml`, `jats` (+ `_archiving`, `_articleauthoring`, `_publishing`), `markua`, `ms`, `muse`,
`opendocument`, `pdf`, `pptx`, `s5`, `slideous`, `slidy`, `tei`, `texinfo`, `textile`,
`vimdoc`, `xml`, `xwiki`, `zimwiki`.

---

## Extensions

Reader-side toggles on the CommonMark engine. The enum defines 77 names; the reader branches on the
**Supported** set below and treats every other name within a format's accepted set as a recorded
no-op toggle, so a format spec naming one parses and records it rather than aborting. Each format
declares the set of extensions it accepts — exactly the names `--list-extensions=<format>` prints,
with the sign each carries when the format is read (or written, where it only writes) — and a `+`/`-`
toggle naming anything outside that set is rejected.

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
`intraword_underscores`, `backtick_code_blocks`, `fenced_code_blocks`, `escaped_line_breaks`,
`space_in_atx_header`, `all_symbols_escapable`, `spaced_reference_links`, `short_subsuperscripts`,
`mmd_title_block`, `mmd_header_identifiers`, `abbreviations`, `markdown_attribute`.

### Known parity gaps

Constructs the supported extensions read, each with one narrow case that still diverges.

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
reader does not yet branch on the construct (notable, non-exhaustive): `latex_macros`,
`wikilinks_title_after_pipe`, `wikilinks_title_before_pipe`, `ascii_identifiers`, `mmd_link_attributes`,
`old_dashes`, `east_asian_line_breaks`, `four_space_rule`, `literate_haskell`, `rebase_relative_paths`,
`gutenberg`, `angle_brackets_escapable`, `ignore_line_breaks`, `raw_markdown`.
(`shortcut_reference_links` is already covered by the CommonMark engine.)

---

## Cross-cutting features

Notes list gaps and limitations only.

| Feature | Status | Notes |
| --- | :---: | --- |
| Standalone output + templates (`-s`, `--template`) | ✅ | Each writer's scaffold (CSS, preamble) is authored independently and is not byte-identical across tools. |
| Table of contents (`--toc`) | ✅ | `revealjs` does not yet emit a slide-relative contents structure. |
| Text wrapping (`--wrap`, `--columns`) | ✅ | A few constructs still account for width incorrectly when reflowed at narrow columns: line blocks, footnote bodies, the AsciiDoc list-marker indent, and roff line-continuation. |
| Section numbering (`--number-sections`) | ✅ | Inert in the Markdown family and `plain` (no heading-number syntax); `revealjs` is not yet numbered. |
| Metadata / variables (`-M`, `-V`, `--metadata-file`) | ✅ | — |
| Syntax highlighting | ✅ | Language classes resolve as written (no alias canonicalization), so a mixed-case spelling of an alias may color differently from its lowercase form. |
| Citations / citeproc (`--citeproc`) | ❌ | `Cite` carried in the AST, not processed. |
| Filters (JSON) | ✅ | `-F`/`--filter` pipes the document as JSON through external programs (in order), each receiving the output format name as its argument. A bare name resolves under the data directory's `filters/`, then the working directory, then `PATH`; a file without the executable bit runs through an interpreter chosen from its extension (`.py`, `.js`, `.rb`, `.php`, `.pl`, `.hs`, `.r`). |
| Filters (Lua) | ❌ | |
| Data directory (`--data-dir`) | ✅ | Overrides for filters (`filters/`) and templates (`templates/`), defaulting to `$XDG_DATA_HOME/carta` (or `~/.local/share/carta`). A `templates/default.<ext>` overrides a format's built-in template; `--template NAME` falls back to `templates/NAME`; template partials fall back to `templates/`. |
| Math output methods (MathJax, KaTeX; MathML, webtex, plain HTML) | 🚧 | `--mathjax` / `--katex` select the HTML renderer. With no method given, HTML math keeps the verbatim TeX in a math span rather than rendering to plain HTML; no MathML, webtex, or default plain-HTML renderer yet. |
| Writer extension toggles | ✅ | `rst` does not yet backslash-escape literal ASCII `--`/`...` under the non-default `+smart`. |
| Embed resources / extract media | ✅ | — |
| Multiple inputs / defaults files (`--defaults`) | ❌ | CLI takes one input. |
| CLI introspection (`--list-input-formats`, `--list-extensions`, …) | ✅ | — |
