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
`footnotes`, `tex_math_dollars`, `gfm_auto_identifiers`, `raw_html`.

### `markdown` — 🚧
The broad Markdown preset on the CommonMark engine. Missing:

| Gap | Detail |
| --- | --- |
| Unmodeled extensions | `citations`, `raw_tex`, `latex_macros`, `inline_notes`, `native_divs`/`native_spans`, … — [not modeled](#not-modeled) |
| `startnum` | inert — the preset enables it but no code reads it |

### `html` — 🚧

| Gap | Detail |
| --- | --- |
| Extensions | `ReaderOptions.extensions` is ignored |
| `<script>` / `<style>` | dropped (except math-bearing `<script>`) |
| Inline round-trip | no `Note` / `Cite` reconstruction |

### `opml` — 🚧
Outline depth → header level; `_note` parsed as CommonMark; metadata (title, author, date).

| Gap | Detail |
| --- | --- |
| `text` attribute | whitespace-tokenized only — inline markup dropped |

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

### `commonmark` — 🚧

| Gap | Detail |
| --- | --- |
| `Block::Figure` | `todo!` |
| `Inline::Math` | `todo!` |
| Image dimensions | `todo!` — width/height HTML fallback |

### `markdown` — 🚧
Renders every AST node, but emits a fixed dialect (extension output toggles not honored) and the
unmodeled pandoc-markdown extensions are unavailable.

### `rst` — ✅
All blocks and inlines; grid/simple/multiline tables, figure directives, `:math:` role.

### `typst` — 🚧

| Gap | Detail |
| --- | --- |
| `Inline::Math` | TeX emitted verbatim inside `$…$`; not translated to Typst's native math syntax |

### `mediawiki` — ✅
All blocks and inlines; HTML fallback where wiki syntax falls short.

### `dokuwiki` — ✅
All blocks and inlines. Math renders as a placeholder.

### `asciidoc` — ✅
All blocks and inlines. Emits the `asciidoc` flavor only — `asciidoc_legacy` / `asciidoctor` not
implemented.

### `jira` — 🚧

| Gap | Detail |
| --- | --- |
| `Inline::Math` | `todo!` — TeX → Jira markup |

### `man` — 🚧

| Gap | Detail |
| --- | --- |
| `Inline::Math` | `todo!` — TeX → roff |

### `plain` — 🚧

| Gap | Detail |
| --- | --- |
| `Inline::Math` | `todo!` |

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

Reader-side toggles the CommonMark engine recognizes — 32 of the 33 defined extensions are wired.

**Supported:** `smart`, `strikeout`, `superscript`, `subscript`, `pipe_tables`, `footnotes`,
`task_lists`, `autolink_bare_uris`, `tex_math_dollars`, `fenced_divs`, `bracketed_spans`,
`hard_line_breaks`, `raw_html`, `header_attributes`, `fenced_code_attributes`,
`inline_code_attributes`, `link_attributes`, `attributes`, `definition_lists`, `grid_tables`,
`multiline_tables`, `simple_tables`, `table_captions`, `line_blocks`, `fancy_lists`,
`example_lists`, `yaml_metadata_block`, `pandoc_title_block`, `auto_identifiers`,
`gfm_auto_identifiers`, `implicit_header_references`, `implicit_figures`.

**Inert:** `startnum` — defined and in the Markdown preset, but no code branches on it (ordered-list
start numbers are always captured).

### Not modeled

No enum variant yet (notable, non-exhaustive): `citations`, `raw_tex`, `raw_attribute`,
`latex_macros`, `tex_math_single_backslash`, `tex_math_double_backslash`, `inline_notes`,
`native_divs`, `native_spans`, `table_attributes`, `intraword_underscores`,
`markdown_in_html_blocks`, `backtick_code_blocks`, `emoji`, `mark`, `alerts`, `abbreviations`,
`wikilinks_title_after_pipe`, `wikilinks_title_before_pipe`, `ascii_identifiers`, `mmd_title_block`,
`mmd_header_identifiers`, `mmd_link_attributes`, `markdown_attribute`, `short_subsuperscripts`,
`old_dashes`, `east_asian_line_breaks`, `escaped_line_breaks`, `four_space_rule`,
`lists_without_preceding_blankline`, `blank_before_blockquote`, `blank_before_header`,
`space_in_atx_header`, `literate_haskell`, `rebase_relative_paths`, `gutenberg`.
(`shortcut_reference_links` is already covered by the CommonMark engine.)

---

## Cross-cutting features

Document-conversion features independent of any single format.

| Feature | Status | Notes |
| --- | :---: | --- |
| Standalone output + templates (`-s`) | ❌ | writers emit fragments only |
| Table of contents (`--toc`) | ❌ | |
| Text wrapping (`--wrap`, `--columns`) | ❌ | width fixed per writer |
| Section numbering (`--number-sections`) | ❌ | |
| Metadata / variables (`-M`, `-V`, `--metadata-file`) | ❌ | |
| Syntax highlighting (`--highlight-style`) | ❌ | code emitted verbatim |
| Citations / citeproc (`--citeproc`) | ❌ | `Cite` carried in AST, not processed |
| Filters (Lua / JSON) | ❌ | |
| Math output methods (MathML, MathJax, KaTeX, webtex) | ❌ | TeX passed through verbatim where the target accepts it (html, latex, rst, asciidoc, mediawiki, dokuwiki); no TeX→native translation (Typst, plain, man, jira) and no MathML/MathJax/KaTeX/webtex emitters |
| Writer extension toggles | ❌ | each writer emits a fixed dialect |
| Embed resources / extract media | ❌ | |
| Multiple inputs / defaults files (`--defaults`) | ❌ | CLI takes one input |
| CLI introspection (`--list-input-formats`, `--list-extensions`, …) | ✅ | `--list-input-formats`, `--list-output-formats` (canonical names and aliases), `--list-extensions[=FORMAT]` (`+`/`-` per the format's default set; the Markdown dialect when no format is given) |
