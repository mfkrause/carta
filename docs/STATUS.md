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

### `html` — 🚧

| Gap | Detail |
| --- | --- |
| Extensions | `ReaderOptions.extensions` is ignored |
| `<script>` / `<style>` | dropped (except math-bearing `<script>`) |
| Inline round-trip | no `Note` / `Cite` reconstruction |

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

**Not started:** `asciidoc`, `biblatex`, `bibtex`, `bits`, `creole`, `csljson`, `djot`, `docbook`,
`docx`, `endnotexml`, `epub`, `fb2`, `haddock`, `ipynb`, `jats`, `jira`, `latex`, `man`,
`markdown_strict`, `markdown_mmd`, `markdown_phpextra`, `markdown_github`, `mdoc`, `mediawiki`,
`muse`, `odt`, `org`, `pod`, `pptx`, `ris`, `rst`, `rtf`, `t2t`, `textile`, `tikiwiki`, `twiki`,
`typst`, `vimwiki`, `xlsx`, `xml`.

---

## Writers

Writers render the full AST but do not branch on extensions — each emits a fixed dialect (see
[cross-cutting features](#cross-cutting-features)).

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
Renders every AST node, but emits a fixed dialect (extension output toggles not honored) and the
unmodeled pandoc-markdown extensions are unavailable.

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

**Not started:** `ansi`, `asciidoc_legacy`, `asciidoctor`, `bbcode` (+ `_fluxbb`, `_hubzilla`,
`_phpbb`, `_steam`, `_xenforo`), `biblatex`, `bibtex`, `chunkedhtml`, `commonmark_x`, `context`,
`csljson`, `docbook` (+ `4`, `5`), `docx`, `dzslides`, `epub` (+ `2`, `3`), `fb2`, `haddock`,
`icml`, `ipynb`, `jats` (+ `_archiving`, `_articleauthoring`, `_publishing`), `markdown_strict`,
`markdown_mmd`, `markdown_phpextra`, `markdown_github`, `markua`, `ms`, `muse`, `odt`,
`opendocument`, `org`, `pdf`, `pptx`, `s5`, `slideous`, `slidy`, `tei`, `texinfo`, `textile`,
`vimdoc`, `xml`, `xwiki`, `zimwiki`.

---

## Extensions

Reader-side toggles the CommonMark engine recognizes. The enum defines 48 extensions, all of which the
reader honors. `raw_html` is always on — the engine preserves raw HTML regardless of the toggle — and
the other 47 are branched on per toggle.

**Supported:** `smart`, `strikeout`, `superscript`, `subscript`, `pipe_tables`, `footnotes`,
`task_lists`, `autolink_bare_uris`, `tex_math_dollars`, `fenced_divs`, `bracketed_spans`,
`hard_line_breaks`, `raw_html`, `header_attributes`, `fenced_code_attributes`,
`inline_code_attributes`, `link_attributes`, `attributes`, `definition_lists`, `grid_tables`,
`multiline_tables`, `simple_tables`, `table_captions`, `line_blocks`, `fancy_lists`,
`example_lists`, `startnum`, `yaml_metadata_block`, `pandoc_title_block`, `auto_identifiers`,
`gfm_auto_identifiers`, `implicit_header_references`, `implicit_figures`, `raw_attribute`,
`inline_notes`, `native_divs`, `native_spans`, `markdown_in_html_blocks`, `raw_tex`, `citations`,
`table_attributes`, `blank_before_blockquote`, `blank_before_header`, `mark`, `emoji`, `alerts`,
`tex_math_single_backslash`, `tex_math_double_backslash`.

### Known parity gaps

Constructs the supported extensions read, each with one narrow case that still diverges. Every entry
is verified against the pinned oracle and tracked for a follow-up.

| Extension(s) | Gap |
| --- | --- |
| `raw_tex`, `native_divs`, `markdown_in_html_blocks` | A raw-TeX environment or block-level HTML element that interrupts an open paragraph with no blank line leaves that paragraph as `Para` rather than tightening it to `Plain`. The free-standing form — a blank line before the construct — is exact. |
| `markdown_in_html_blocks` | An HTML block left open at end of input — a `<!-- …` comment or a `<table>`/`<div>` with no close tag — is reparsed as ordinary paragraphs by the dialect; carta keeps the whole run as one raw block. |
| `native_spans` | An emphasis run that opens before a `<span>` and whose closing marker sits just inside the matching `</span>` can leave both tags raw instead of forming a span. |
| `raw_tex` | Inline `\command{…}[…]` consumes every group that follows it; commands that take a fixed number of arguments and leave the rest as text are not modeled. A `\begin{env}…\end{env}` is recognized only as a whole paragraph (block level); inline, each `\begin`/`\end` is an ordinary command. |
| `citations` | An abbreviation-led citation suffix such as `p. 5` is a single string in the dialect (a non-breaking space follows the period); carta splits it into separate tokens. |
| `attributes` | An attribute spec `{…}` containing a backslash escape is void in the dialect — it stays literal text — whereas carta accepts the backslash into the id, class, or value. |
| `alerts` | An alert marker indented two or more columns inside its blockquote (e.g. `>  [!NOTE]`) is still read as an alert; the dialect treats only a marker at column 0 or 1 as one. |

### Not modeled

No enum variant yet (notable, non-exhaustive): `latex_macros`, `intraword_underscores`,
`backtick_code_blocks`, `abbreviations`, `wikilinks_title_after_pipe`,
`wikilinks_title_before_pipe`, `ascii_identifiers`, `mmd_title_block`, `mmd_header_identifiers`,
`mmd_link_attributes`, `markdown_attribute`, `short_subsuperscripts`, `old_dashes`,
`east_asian_line_breaks`, `escaped_line_breaks`, `four_space_rule`,
`lists_without_preceding_blankline`, `space_in_atx_header`, `literate_haskell`,
`rebase_relative_paths`, `gutenberg`.
(`shortcut_reference_links` is already covered by the CommonMark engine.)

---

## Cross-cutting features

Document-conversion features independent of any single format.

| Feature | Status | Notes |
| --- | :---: | --- |
| Standalone output + templates (`-s`, `--template`) | ✅ | a built-in template engine (conditionals, loops, partials, pipes) drives a default template per writer; title/author/date and the format's identity variables are populated from metadata. User templates via `--template`. Each writer's scaffold (CSS, preamble) is authored independently and not byte-identical across tools |
| Table of contents (`--toc`) | ❌ | default templates carry an inert `$toc$` slot; TOC generation itself not yet implemented |
| Text wrapping (`--wrap`, `--columns`) | 🚧 | `--wrap=auto\|none\|preserve` honored by every writer that lays out lines; `--columns` (configurable fill width) not yet — width fixed at 72 |
| Section numbering (`--number-sections`) | ❌ | template body-numbering slot exists but is inert; the header transform itself not yet implemented |
| Metadata / variables (`-M`, `-V`, `--metadata-file`) | ✅ | `-M`/`--metadata-file` set document metadata, `-V` sets template variables; effective precedence is `-V` over `-M` over the document's front matter over `--metadata-file` defaults |
| Syntax highlighting (`--highlight-style`) | ❌ | code emitted verbatim |
| Citations / citeproc (`--citeproc`) | ❌ | `Cite` carried in AST, not processed |
| Filters (Lua / JSON) | ❌ | |
| Math output methods (MathML, MathJax, KaTeX, webtex) | ❌ | TeX is passed through verbatim where the target accepts raw TeX (html, latex, rst, asciidoc, mediawiki, dokuwiki) and otherwise translated to the target's native math — Typst's native syntax for `typst`, a Unicode-text approximation for `commonmark`/`plain`/`man`/`jira`; no MathML/MathJax/KaTeX/webtex emitters. The standalone HTML template carries an inert math-method slot for when those emitters land |
| Writer extension toggles | ❌ | each writer emits a fixed dialect |
| Embed resources / extract media | ❌ | |
| Multiple inputs / defaults files (`--defaults`) | ❌ | CLI takes one input |
| CLI introspection (`--list-input-formats`, `--list-extensions`, …) | ✅ | `--list-input-formats`, `--list-output-formats` (canonical names and aliases), `--list-extensions[=FORMAT]` (`+`/`-` per the format's default set; the Markdown dialect when no format is given) |
