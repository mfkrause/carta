# carta status

Per-format detail behind the [README support matrix](../README.md#status); the README grid is the
at-a-glance roster. Measured against pinned pandoc **3.10** (`pandoc-api-version 1.23.1`).

✅ usable — basically done; any remaining parity gaps are minor and unlikely to affect regular use · 🚧 in development — large parity gaps or breaking issues (e.g. panics), not recommended for use yet · ❌ not started · ➖ not applicable (pandoc has no such direction)

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

**Not started:** `asciidoc`, `biblatex`, `bibtex`, `bits`, `creole`, `csljson`, `djot`, `docbook`,
`docx`, `endnotexml`, `epub`, `fb2`, `haddock`, `jats`, `mdoc`, `muse`, `odt`, `pod`, `pptx`, `ris`,
`rtf`, `t2t`, `textile`, `tikiwiki`, `twiki`, `typst`, `vimwiki`, `xlsx`, `xml`.

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

**Not started:** `ansi`, `asciidoc_legacy`, `asciidoctor`, `bbcode` (+ `_fluxbb`, `_hubzilla`,
`_phpbb`, `_steam`, `_xenforo`), `biblatex`, `bibtex`, `chunkedhtml`, `context`,
`csljson`, `docbook` (+ `4`, `5`), `dzslides`, `fb2`, `haddock`,
`icml`, `jats` (+ `_archiving`, `_articleauthoring`, `_publishing`), `markua`, `ms`, `muse`,
`odt`, `opendocument`, `pdf`, `pptx`, `s5`, `slideous`, `slidy`, `tei`, `texinfo`, `textile`,
`vimdoc`, `xml`, `xwiki`, `zimwiki`.

---

## Extensions

Reader-side toggles on the CommonMark engine. The enum defines 77 names; the reader branches on the
**Supported** set below and accepts every other name as a recorded no-op toggle, so a format spec
naming one parses and records it rather than aborting.

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
| Syntax highlighting (`--highlight-style`) | ❌ | Code emitted verbatim. |
| Citations / citeproc (`--citeproc`) | ❌ | `Cite` carried in the AST, not processed. |
| Filters (JSON) | ✅ | `-F`/`--filter` pipes the document as JSON through external programs (in order), each receiving the output format name as its argument. A bare name resolves under the data directory's `filters/`, then the working directory, then `PATH`; a file without the executable bit runs through an interpreter chosen from its extension (`.py`, `.js`, `.rb`, `.php`, `.pl`, `.hs`, `.r`). |
| Filters (Lua) | ❌ | |
| Data directory (`--data-dir`) | ✅ | Overrides for filters (`filters/`) and templates (`templates/`), defaulting to `$XDG_DATA_HOME/carta` (or `~/.local/share/carta`). A `templates/default.<ext>` overrides a format's built-in template; `--template NAME` falls back to `templates/NAME`; template partials fall back to `templates/`. |
| Math output methods (MathJax, KaTeX; MathML, webtex, plain HTML) | 🚧 | `--mathjax` / `--katex` select the HTML renderer. With no method given, HTML math keeps the verbatim TeX in a math span rather than rendering to plain HTML; no MathML, webtex, or default plain-HTML renderer yet. |
| Writer extension toggles | ✅ | `rst` does not yet backslash-escape literal ASCII `--`/`...` under the non-default `+smart`. |
| Embed resources / extract media | ✅ | — |
| Multiple inputs / defaults files (`--defaults`) | ❌ | CLI takes one input. |
| CLI introspection (`--list-input-formats`, `--list-extensions`, …) | ✅ | — |
