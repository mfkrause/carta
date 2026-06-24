# Plan 008: Finish the HTML reader — extension toggles, footnote reconstruction, and a shared inline-scanner module

> **Executor instructions**: Follow this plan step by step. Run every verification command and
> confirm the expected result before moving on. If anything under "STOP conditions" occurs, stop and
> report — do not improvise. When done, update this plan's status row in `plans/README.md` and the
> two status docs (`README.md`, `docs/STATUS.md`).
>
> **Drift check (run first)**:
> `git diff --stat 7b2ba0f..HEAD -- crates/carta-readers/src/html/ crates/carta-readers/src/commonmark/inline.rs crates/carta-core/src/extensions.rs crates/carta/src/format_spec.rs crates/carta/src/lib.rs`
> If any of these changed since this plan was written, re-verify the "Current state" excerpts (§4)
> against the live code before proceeding; on a material mismatch, treat it as a STOP condition.

## Status

- **Status**: TODO
- **Priority**: P1
- **Effort**: L (one reader, but the shared-scanner extraction touches the CommonMark inline parser
  and must keep its snapshots byte-identical; that refactor is the subtle part)
- **Risk**: MED (the scanner extraction is a refactor of a hot, intricate file; the math/smart text
  pass over an already-tokenized inline stream differs from the CommonMark engine's single-pass model)
- **Depends on**: nothing hard. **Soft**: plan 006 (DONE) built every piece of plumbing this reuses —
  `parse_format_spec`, `ReaderOptions.extensions`, the `corpus/text-ext/` corpus pattern, and the
  `reader-ext` conformance group that loops `corpus/text-ext/*`. This plan adds `html+…`/`html-…`
  directories to that same corpus, so the conformance group picks them up with no shell change.
  **Coordinate with plan 003** (linear delimiter stack, DONE/merged): both touch
  `commonmark/inline.rs`. The scanner extraction here must not regress 003's resolver.
- **Category**: feature (parity)
- **Planned at**: commit `7b2ba0f`, 2026-06-24. The semantics in §3 were derived empirically from the
  pinned oracle (pandoc 3.10) via `pandoc -f html[+ext][-ext] -t native`, which is the sanctioned
  clean-room source (observable CLI behavior). They are reproduced here so the executor does not need
  the oracle to understand the target; the conformance layer re-checks them live.

## 1. Why this matters

`docs/STATUS.md` marks the `html` reader 🚧 with three gaps:

| Gap | STATUS wording | Reality (oracle-verified, §3) |
| --- | --- | --- |
| 1 | `ReaderOptions.extensions` is ignored | **Real.** The reader hardcodes one dialect and takes `_options`. |
| 2 | `<script>`/`<style>` dropped (except math-bearing `<script>`) | **Already at parity** — the oracle's HTML reader also drops them, in every `raw_html` mode. Verify-only + STATUS correction. |
| 3 | no `Note` / `Cite` reconstruction | **`Note` is real** and reconstructable. **`Cite` is a non-goal**: the oracle emits a `Span` with class `citation`, not a `Cite` — already produced by `native_spans`. |

So the genuine work is (1) **honor the extension set** and (3) **reconstruct `Note`**. The HTML
reader already emits `Div`/`Span`/`LineBlock` and generates heading ids exactly as the default
preset prescribes — what it cannot do is *toggle* any of it, because `_options` is discarded. This
plan threads the set through, gates the four default-on structural extensions and adds the
default-off text extensions, reconstructs footnotes, and corrects the two STATUS gaps that are
already at parity. It also extracts the near-pure inline scanners (math, smart punctuation) out of
the CommonMark engine into a shared module both readers consume, so `smart`/`tex_math_*` are
implemented once.

Outcome: the `html` reader moves from 🚧 toward ✅, and a `read-html` conformance-ext group runs in
CI alongside the CommonMark one.

## 2. Scope

**In scope:**

- `carta/src/format_spec.rs`: add the `html`/`html5`/`html4` default extension set
  (`{auto_identifiers, line_blocks, native_divs, native_spans}` — §3.1).
- `carta-readers` HTML: thread `options.extensions` into the converter; **gate** the four
  default-on structural extensions so `-native_divs`/`-native_spans`/`-auto_identifiers`/
  `-line_blocks` take effect (§3.2–3.6); **add** the default-off text extensions `smart`,
  `tex_math_dollars`, `tex_math_single_backslash`, `tex_math_double_backslash` (§3.7–3.10).
- A **shared inline-scanner module** in `carta-readers` holding the near-pure scanners extracted from
  `commonmark/inline.rs` (math delimiters, dash/ellipsis folds, curly-quote glyph + flanking
  predicates), consumed by both readers (§5 Step 4).
- `Note` reconstruction: an always-on post-pass over the assembled document (§3.13).
- Tests: Layer 0 units, Layer 1 golden snapshots over new `corpus/text-ext/html…/` directories,
  Layer 2 parity via the existing `reader-ext` conformance group.
- STATUS corrections: gap 2 (script/style — already at parity) and the `Cite` half of gap 3.

**Out of scope (deliberate, with rationale):**

- **`raw_tex`.** Inert in the HTML reader: `\command{…}` and `\begin{…}…\end{…}` stay literal text
  whether the toggle is on or off (§3.11). No behavior change; the name already parses. Not added to
  the HTML default set.
- **`raw_html` behavior change.** Also inert for `<script>`/`<style>`/comments/unknown tags in the
  HTML reader — all dropped or unwrapped regardless of the toggle (§3.12). carta already matches.
- **`Cite` reconstruction.** The oracle does not emit a `Cite` from HTML; `<span class="citation">`
  round-trips as a `Span`, which `native_spans` already produces. Verify-only (§3.3).
- **`markdown_in_html_blocks`.** That extension governs the *Markdown* engine reparsing HTML islands,
  not the HTML reader. Separate concern.
- **The HTML *writer*.** Already ✅; untouched.
- **Extensions present in the enum but not honored by this reader** (e.g. `footnotes`, `emoji`): they
  parse into the set and are ignored — the established `ReaderOptions.extensions` contract. Only
  names absent from the enum error as `UnknownExtension`.
- **`gfm_auto_identifiers`** is a **stretch** (§5 Step 8): it only swaps the slug algorithm and is
  easy to defer to a follow-up if pinning the exact algorithm proves fiddly.

## 3. Exact semantics (oracle-derived; these are the test oracle)

All confirmed against pandoc 3.10's HTML reader via `… -f html[±ext] -t native`. `☐` = U+2610,
`☒` = U+2612.

### 3.1 HTML default extension set

`pandoc --list-extensions=html` reports exactly four on by default:
`+auto_identifiers +line_blocks +native_divs +native_spans`. So
`default_extensions("html") = {AutoIdentifiers, LineBlocks, NativeDivs, NativeSpans}` (and the same
for the `html5`/`html4` aliases). `smart` and the `tex_math_*` family are **off** by default. `+ext`
inserts, `-ext` removes, applied onto this base (the §006 `parse_format_spec` mechanics, unchanged).

### 3.2 `native_divs` (default on)

- on: `<div class="foo"><p>hi</p></div>` → `[Div ("",["foo"],[]) [Para [Str "hi"]]]`.
- off: the wrapper is dropped and the children are spliced in place → `[Para [Str "hi"]]`.

carta currently **always** wraps (`convert.rs:162`). The change is: when `native_divs` is off, return
the child blocks without the `Div`. The `line-block` div (§3.6) is a separate path.

### 3.3 `native_spans` (default on)

- on: `<p><span class="foo">hi</span></p>` → `[Para [Span ("",["foo"],[]) [Str "hi"]]]`.
- off: `[Para [Str "hi"]]` (span unwrapped to its inlines).

A `<span class="citation">` (and the `cites` data attribute) round-trips as that same `Span` — this
**is** the "Cite reconstruction" of gap 3, already produced by `native_spans`. Confirm
`<span class="citation" data-cites="x">…</span>` → `Span ("",["citation"],[("cites","x")]) […]`; no
`Cite` node is expected.

### 3.4 `auto_identifiers` (default on)

The heading id is generated from its text when on, and **empty** when off; an explicit `id` is always
kept either way.

- on: `<h1>Hello World</h1>` → `Header 1 ("hello-world",[],[]) [Str "Hello", Space, Str "World"]`.
- off: `Header 1 ("",[],[]) […]`.
- explicit, off: `<h1 id="x">Hello World</h1>` → `Header 1 ("x",[],[]) […]`.

The default slug: lowercase; spaces collapse to a single `-`; punctuation other than `-`, `_`, `.`
is removed; letters/marks/digits kept (combining marks survive — `Heĺlo` → `heĺlo`). Example:
`<h2>Hello, World! 1.0</h2>` → `hello-world-1.0`. **carta's `header_attr`/`unique_id`
(`convert.rs:385`/`:401`) already implement this default slug** — verify it matches, then the only
change is to emit an empty id (skip generation) when `auto_identifiers` is off. Do **not**
reimplement the slug.

### 3.5 `gfm_auto_identifiers` (stretch)

Honored only when `auto_identifiers` is also on (it swaps the slug algorithm; with
`auto_identifiers` off, no id is generated regardless). The GitHub slug keeps a hyphen where the
default elides removed punctuation, yielding extra hyphens:
`<h2>Heĺlo Wörld & Stuff!</h2>` → default `heĺlo-wörld-stuff`, GitHub `heĺlo-wörld--stuff` (note the
double hyphen where `& ` was). Pin the exact algorithm against the oracle during implementation, or
defer to the §10 follow-up.

### 3.6 `line_blocks` (default on)

- on: `<div class="line-block">a<br />b</div>` → `[LineBlock [[Str "a"],[Str "b"]]]`.
- off: `[Div ("",["line-block"],[]) [Plain [Str "a", LineBreak, Str "b"]]]`.

carta already produces the `LineBlock` form (`convert.rs:158`, `is_line_block_div`). When
`line_blocks` is off, treat the div as an ordinary div (which, if `native_divs` is also off, then
unwraps per §3.2). The internal `<br>` becomes a `LineBreak` inside a `Plain`.

### 3.7 `smart` (default off)

On, in text content: straight quotes become `Quoted` nodes, `--`→en dash (U+2013), `---`→em dash
(U+2014), `...`→ellipsis (U+2026).

- off: `<p>"a" -- ... ---</p>` → `[Para [Str "\"a\"", Space, Str "--", Space, Str "...", Space, Str "---"]]`.
- on (`+smart`): `[Para [Quoted DoubleQuote [Str "a"], Space, Str "\8211", Space, Str "\8230", Space, Str "\8212"]]`.

In the HTML reader the inline stream is already tokenized (text runs split by tags), so `smart`
runs as a **flat pass over the inline list**: fold dash/ellipsis runs inside each `Str`, and resolve
straight quotes to `Quoted` using flanking eligibility computed from the surrounding text. This is
the shared-scanner reuse (Step 4) — not the CommonMark delimiter stack.

### 3.8 `tex_math_dollars` (default off)

- off: `<p>$x^2$ and $$y$$</p>` → `[Para [Str "$x^2$", Space, Str "and", Space, Str "$$y$$"]]`.
- on: `[Para [Math InlineMath "x^2", Space, Str "and", Space, Math DisplayMath "y"]]`.

### 3.9 `tex_math_single_backslash` (default off)

On: `\(x\)` → `Math InlineMath "x"`, `\[y\]` → `Math DisplayMath "y"`. Off: literal `Str`.

### 3.10 `tex_math_double_backslash` (default off)

On: `\\(x\\)` → `Math InlineMath "x"`, `\\[y\\]` → `Math DisplayMath "y"`. Off: literal `Str`.

The three math scanners already exist in the CommonMark engine (`scan_inline_math`,
`scan_display_math`, `scan_backslash_math`); the dollars scanner is what `commonmark+tex_math_dollars`
uses. Extract them to the shared module (Step 4) and run them over the HTML reader's text runs.

### 3.11 `raw_tex` — inert in the HTML reader

`<p>a \command{b} c and \begin{x}env\end{x}</p>` → identical output with and without `+raw_tex`:
`[Para [Str "a", Space, Str "\\command{b}", Space, Str "c", Space, Str "and", Space, Str "\\begin{x}env\\end{x}"]]`.
No reader change; not in the HTML default set. Document this in STATUS so it isn't mistaken for a gap.

### 3.12 `raw_html` — inert for dropped constructs; gap 2 is already at parity

In the HTML reader, `<script>`, `<style>`, comments, and unknown/unsupported block tags
(`<video>`, `<iframe>`) are dropped — and recognized-but-unmapped inline tags are unwrapped —
**regardless** of the `raw_html` toggle:

- `<p>a</p><script>var x=1;</script>` → `[Para [Str "a"]]` (both `+raw_html` and `-raw_html`).
- `<style>.a{color:red}</style><p>a</p>` → `[Para [Str "a"]]`.
- `<p>a</p><!-- c -->` → `[Para [Str "a"]]`.
- `<p>a <custom-tag>b</custom-tag> c</p>` → `[Para [Str "a", Space, Str "b", Space, Str "c"]]`.

carta already does exactly this (`script_content_is_dropped`, `style_block_is_dropped`). So STATUS
gap 2 is **already at parity** — the entry should be reworded from a gap to documented behavior. The
math-bearing script is the one exception and stays: `<script type="math/tex">x^2</script>` →
`[Plain [Math InlineMath "x^2"]]`, `…; mode=display">` → `DisplayMath` (carta:
`math_script_becomes_inline_math`).

### 3.13 `Note` reconstruction (always on — not extension-gated)

A footnote round-tripped to HTML emits a reference anchor at the cite site and an end-of-document
section:

```html
text<a href="#fn1" class="footnote-ref" id="fnref1" role="doc-noteref"><sup>1</sup></a>
…
<section id="footnotes" class="footnotes footnotes-end-of-document" role="doc-endnotes">
<hr />
<ol>
<li id="fn1"><p>the note<a href="#fnref1" class="footnote-back" role="doc-backlink">↩︎</a></p></li>
</ol>
</section>
```

Reading it back: `[Para [Str "text", Note [Para [Str "the", Space, Str "note"]]]]`. The algorithm:

1. After the tree is built, find the footnotes container — an element whose class list contains
   `footnotes` (tolerate both `<section>` and `<div>`; the `footnotes-end-of-document` /
   `footnotes-end-of-block` modifier is incidental).
2. Index its `<ol><li id="fnN">…</li></ol>` items by `id`. Each note body is the `<li>`'s blocks
   **minus** a trailing back-reference anchor (`<a class="footnote-back">`).
3. Replace every inline `<a class="footnote-ref" href="#fnN">…</a>` with `Note <body of fnN>`.
4. Drop the footnotes container from the block stream.

Unmatched refs (no `<li>` with that id) and an empty/absent container leave the document unchanged.
This post-pass is **always on** — pandoc reconstructs notes without an extension toggle.

### 3.14 Task-list checkbox glyphs (verify-only)

`- [x] done` / `- [ ] todo` round-tripped through HTML read back as
`[BulletList [[Plain [Str "\9746", Space, Str "done"]], [Plain [Str "\9744", Space, Str "todo"]]]]`
— `\9746` = ☒ U+2612, `\9744` = ☐ U+2610. carta already produces these
(`checkbox_in_item_renders_ballot_box`); no change, just a corpus case to lock it.

## 4. Current state (excerpts to build on)

- `crates/carta-readers/src/html/mod.rs` — `Reader::read(&self, input, _options)` calls
  `parse(input)` and **discards `_options`**. `parse` → tokenize → `build_tree` → `locate` head/body
  → `Converter::default()`. This is the single entry point to thread the extension set through.
- `crates/carta-readers/src/html/convert.rs` (~904 LOC) — `Converter` (currently `Default`), `process`
  drops `<script>` unless `is_math_script`, drops blank `<style>`. Key sites:
  - `:158` `is_line_block_div(e)` → `Block::LineBlock(self.line_block_lines(&e.children))`; `:162`
    else → `Block::Div(attr, …)`. (line_blocks + native_divs)
  - `:385` `header_attr`, `:401` `unique_id` — the default-slug id generator. (auto_identifiers)
  - `:452`/`:463`/`:471` `Inline::Span(…)`. (native_spans)
  - `:726` `is_line_block_div`. (line_blocks)
  - `math_script_type` recognizes `<script type="math/tex">`.
  - Existing `#[cfg(test)]`: `script_content_is_dropped`, `style_block_is_dropped`,
    `math_script_becomes_inline_math`, `checkbox_in_item_renders_ballot_box`.
- `crates/carta-readers/src/commonmark/inline.rs` — holds the scanners to extract: `scan_inline_math`,
  `scan_display_math`, `scan_backslash_math` (near-pure over `self.chars`/`self.pos`), the free
  functions `fold_dash_run(len)` / `fold_ellipsis_run(len)`, the curly-quote glyph helper, and the
  flanking-eligibility predicates (`is_smart_delim` ~`:2484`, flanking ~`:2489`/`:3230`). Smart quotes
  resolve through the emphasis delimiter stack (`emphasis_run`). The extraction must keep all of this
  working and the CommonMark golden snapshots byte-identical.
- `crates/carta/src/format_spec.rs` — `default_extensions(base)` has cases for `commonmark`,
  `commonmark_x`, `markdown`, `gfm`; **no `html` case** (so it currently returns empty for `html`,
  which is wrong once the reader honors the set). `parse_format_spec` is otherwise complete.
- `crates/carta/tests/golden_reader.rs` — `reader_ext_ast_snapshots` already loops
  `corpus_cases("text-ext")`, derives the base via `parse_format_spec`, skips bases that aren't a
  compiled reader (`reader_for`), and snapshots `convert(spec → json)`. New `corpus/text-ext/html…/`
  dirs need **no test change** — they are picked up automatically.
- `tools/conformance-suite/surfaces/reader.sh` — after its hardcoded format loop, loops
  `corpus/text-ext/*` using each directory name as the `-f` spec for both carta and the oracle. New
  `html+…`/`html-…` dirs are picked up automatically (this is the parity gate).

Repo conventions that apply: no `unwrap`/`expect`/`panic`/`unreachable`/slice-indexing outside
`#[cfg(test)]` (use `.get()` + `?`); comments only where the *why* is non-obvious; **provenance
rule** — in product source cite the HTML/format syntax and state behavior as the code's own design,
never the upstream tool; deterministic output (`BTreeMap`); Conventional Commits, explicit
`git add` paths, no push.

## 5. Implementation steps

Branch: `feat/008-finish-html-reader` off `main`.

### Step 1 — HTML default extension set

In `crates/carta/src/format_spec.rs`, add to `default_extensions`:
`"html" | "html5" | "html4" => Extensions::from_list(&[Extension::AutoIdentifiers,
Extension::LineBlocks, Extension::NativeDivs, Extension::NativeSpans])`.

**Verify**: unit test `parse_format_spec("html")` → that set; `"html-native_divs"` removes it;
`"html+smart"` adds `Smart`; `"html+bogus"` → `Err(UnknownExtension)`. `cargo nextest run -p carta`.

### Step 2 — Thread the extension set into the converter

- `mod.rs`: `read` → `Ok(parse(input, options.extensions))`. Update the doc comment to state the
  reader honors the extension set (list the honored ones) — no upstream references.
- `convert.rs`: replace `Converter::default()` with a constructor taking `Extensions`; store it on the
  `Converter`. (Keep a `Default` impl for the unit tests, or update them to pass an explicit set.)

**Verify**: `cargo build -p carta-readers`; existing HTML unit tests still pass (they exercise the
default set). `cargo nextest run -p carta-readers`.

### Step 3 — Gate the four structural extensions

In `convert.rs`, branch on the stored set:

- `native_divs` off → at the `Div` site (`:162`) return the child blocks instead of wrapping. The
  `line-block` div is handled first (next bullet), so this only affects ordinary divs.
- `line_blocks` off → `is_line_block_div` path (`:158`) falls through to the ordinary `Div` path
  (which, with `native_divs` also off, then unwraps). The `<br>` inside becomes a `LineBreak` in a
  `Plain` (§3.6).
- `native_spans` off → at the `Span` sites unwrap to the inner inlines.
- `auto_identifiers` off → `header_attr` emits an empty id (skip `unique_id`); an explicit `id`
  attribute is still kept.

**Verify**: focused unit tests for each off-toggle (§3.2–3.6 examples) driving
`HtmlReader::read` with an explicit `ReaderOptions { extensions }`. `cargo nextest run -p carta-readers`.

### Step 4 — Extract the shared inline-scanner module

Create `crates/carta-readers/src/inline_scan.rs` (module name at the executor's discretion; keep it
provenance-neutral and descriptive). Move out of `commonmark/inline.rs`, as free functions over
`&[char]` + index (or `&str`), the near-pure scanners:

- math: `scan_inline_math`, `scan_display_math`, `scan_backslash_math` (single- and double-backslash
  variants) returning the math text and consumed length;
- smart punctuation: `fold_dash_run`, `fold_ellipsis_run` (already free functions), the curly-quote
  glyph chooser, and the flanking-eligibility predicates used to decide whether a straight quote
  opens or closes.

The CommonMark inline parser keeps its delimiter-stack quote resolution but **calls the shared
predicates** instead of private copies. This is a pure refactor for the CommonMark side: its golden
snapshots must stay byte-identical.

**Verify**: `cargo nextest run -p carta-readers`; **`cargo nextest run -p carta`** with **no**
`.snap.new` for any `commonmark…`/`gfm…`/`markdown…`/`commonmark_x…` snapshot. If a CommonMark
snapshot changes, the extraction altered behavior — STOP and reconcile before continuing.

### Step 5 — `smart` in the HTML reader

Add a flat pass over the HTML reader's assembled inline list, enabled when `smart` is in the set:
fold dash/ellipsis runs inside each `Str` (shared folds), and resolve straight `"`/`'` to
`Quoted DoubleQuote`/`Quoted SingleQuote` using the shared flanking predicates over adjacent text.
Apply recursively through inline-bearing blocks.

**Verify**: unit tests for the §3.7 example (on and off). `cargo nextest run -p carta-readers`.

### Step 6 — TeX math in the HTML reader

When the relevant toggle is set, scan each text run for math delimiters using the shared scanners and
split `Str` into `Str`/`Math` pieces:

- `tex_math_dollars`: `$…$` → `InlineMath`, `$$…$$` → `DisplayMath` (§3.8).
- `tex_math_single_backslash`: `\(…\)` → `InlineMath`, `\[…\]` → `DisplayMath` (§3.9).
- `tex_math_double_backslash`: `\\(…\\)`, `\\[…\\]` (§3.10).

`raw_tex` is **not** wired (inert — §3.11); leave `\command` runs as literal text.

**Verify**: unit tests for §3.8–3.10 (on and off). `cargo nextest run -p carta-readers`.

### Step 7 — `Note` reconstruction (always on)

Implement the §3.13 post-pass over the assembled `Document` (block stream + inline walk). Keep it a
self-contained function (no panics, `.get()` indexing). Always run it — it is not extension-gated.
Unmatched refs and an absent container leave the document unchanged.

**Verify**: unit test round-tripping the §3.13 markup; plus an unmatched-ref case (document
unchanged). `cargo nextest run -p carta-readers`.

### Step 8 — `gfm_auto_identifiers` slug (stretch)

If pursued: when both `auto_identifiers` and `gfm_auto_identifiers` are set, generate the GitHub slug
(§3.5) instead of the default. Pin the exact algorithm against the oracle with the conformance group
(Step 11). If the algorithm proves fiddly, drop this step and record it as a §10 follow-up — the rest
of the plan stands without it.

### Step 9 — Layer 0 unit tests

Consolidate the per-step unit tests in `convert.rs` (or a sibling test module), each driving
`HtmlReader::read` with an explicit extension set and asserting the §3 expected blocks. Cover every
on/off pair in §3.2–3.10, the §3.13 `Note` round-trip, and the §3.14 checkbox glyphs.

**Verify**: `cargo nextest run -p carta-readers`.

### Step 10 — Layer 1 golden corpus + snapshots

Create `corpus/text-ext/<spec>/*.html`, one directory per configuration, the directory name being the
**full format spec** (single source of truth for carta and the oracle). Suggested set:

- toggles off: `html-native_divs/`, `html-native_spans/`, `html-auto_identifiers/`, `html-line_blocks/`.
- toggles on: `html+smart/`, `html+tex_math_dollars/`, `html+tex_math_single_backslash/`,
  `html+tex_math_double_backslash/`.
- always-on behavior (use plain `html`): footnote round-trip, checkbox glyphs, dropped script/style,
  the citation `Span`. (Place these under `corpus/text/html/` if a base-format directory already
  exists there, or a `html/` dir under `text-ext` — either is picked up.)
- if Step 8 shipped: `html+gfm_auto_identifiers/`.

`golden_reader.rs::reader_ext_ast_snapshots` needs **no change**. Run `cargo nextest run -p carta`
to produce `.snap.new` — **do not accept yet** (Step 11 proves them against the oracle first).

### Step 11 — Layer 2 conformance (the parity gate, CI-gated)

The `reader-ext` group already loops `corpus/text-ext/*`, so the new `html…` directories are diffed
against the oracle with no shell change.

**Verify (requires `.oracle/` + `jq`)**: `cargo build -p carta-cli && tools/conformance-suite/run.sh
reader` → every `RESULT reader ext-html…` line shows `fail=0 err=0`. Iterate on §5 until clean. The
§3 semantics were verified over a broad probe set; a new divergence is information — capture it, do
not paper over it. **Only once green**, accept the Layer-1 snapshots (`cargo insta accept`) and re-run
`cargo nextest run --workspace`.

### Step 12 — STATUS/README sync + full gate

- `docs/STATUS.md`: update the `### html — 🚧` block. Reword gap 2 (script/style — documented
  behavior at parity, §3.12) and the `Cite` half of gap 3 (round-trips as a `citation` `Span` via
  `native_spans`, §3.3). List the honored toggles. Note `raw_tex`/`raw_html` as inert in this reader.
  If all gaps close, flip the reader to ✅; otherwise keep 🚧 with the residual (e.g.
  `gfm_auto_identifiers` if deferred).
- `README.md`: flip the HTML reader cell if it reached ✅.

**Full gate** (all must pass):

- `cargo nextest run --workspace` → pass; `git status` shows no unexpected `.snap.new`.
- `cargo test --doc --workspace` → pass.
- `cargo clippy --all-targets --all-features` → exit 0, no warnings.
- `cargo fmt --all --check` → clean.
- `cargo build -p carta --no-default-features --features read-html,write-json` and its
  `nextest run` → pass (the reader compiles in a minimal build).
- `tools/conformance-suite/run.sh all` → no `fail`/`err` on any surface (confirms the
  CommonMark refactor and the new corpus leaked into no other surface).
- `cargo llvm-cov clean --workspace && cargo llvm-cov --workspace --summary-only --fail-under-lines 90`
  → ≥ 90%.

## 6. Commands reference

| Purpose | Command | Expected |
|---|---|---|
| Unit/golden tests | `cargo nextest run --workspace` | all pass, no pending snapshots |
| Doctests | `cargo test --doc --workspace` | pass |
| Lint | `cargo clippy --all-targets --all-features` | exit 0, no warnings |
| Format | `cargo fmt --all --check` | clean |
| Snapshots | `cargo insta accept` (only after Step 11 parity is green) | snapshots written |
| Conformance (reader) | `cargo build -p carta-cli && tools/conformance-suite/run.sh reader` | `RESULT reader ext-html… fail=0 err=0` |
| Conformance (all) | `tools/conformance-suite/run.sh all` | no fail/err on any surface |
| Minimal build | `cargo build -p carta --no-default-features --features read-html,write-json` | compiles |
| Coverage | `cargo llvm-cov clean --workspace && cargo llvm-cov --workspace --summary-only --fail-under-lines 90` | ≥ 90% |
| Oracle probe (ad hoc) | `printf '%s' '<in>' \| .oracle/bin/pandoc -f html[±ext] -t native` | the §3 expected |

## 7. Test plan summary

- **Layer 0** — per-extension reader unit tests + the shared-scanner extraction's own units; the
  `Note` post-pass and checkbox glyphs. Offline, §3 expecteds baked in.
- **Layer 1** — golden AST-JSON snapshots over the new `corpus/text-ext/html…/` directories; frozen
  only after Layer 2 proves them correct. The existing CommonMark snapshots act as the refactor's
  byte-identity guard.
- **Layer 2** — the `reader-ext` conformance group diffs carta vs the pinned oracle per spec
  directory, in CI. This is the parity guarantee.

## 8. Done criteria

- [ ] `default_extensions("html"/"html5"/"html4")` = `{auto_identifiers, line_blocks, native_divs,
      native_spans}`, tested.
- [ ] HTML reader honors `options.extensions`: `-native_divs`/`-native_spans`/`-auto_identifiers`/
      `-line_blocks` change output (§3.2–3.6); `+smart`/`+tex_math_dollars`/`+tex_math_single_backslash`/
      `+tex_math_double_backslash` change output (§3.7–3.10); default `html` output is byte-unchanged
      from before.
- [ ] Shared inline-scanner module extracted; CommonMark/GFM/Markdown snapshots byte-identical.
- [ ] `Note` reconstruction works (always on); unmatched refs leave the document unchanged.
- [ ] `corpus/text-ext/html…/` exists; Layer-1 snapshots committed; `reader-ext` group green
      (`fail=0 err=0`) locally with `.oracle/`.
- [ ] `docs/STATUS.md` gap 2 (script/style) and the `Cite` half of gap 3 reworded as
      documented-at-parity; `raw_tex`/`raw_html` noted inert; reader cell updated in both status docs.
- [ ] `cargo nextest run --workspace`, `--doc`, `clippy --all-targets --all-features`, `fmt --check`,
      the minimal-feature build, and coverage ≥ 90% all pass.
- [ ] No `unwrap`/`expect`/`panic`/slice-indexing added outside `#[cfg(test)]`.
- [ ] No upstream provenance in product source.
- [ ] `plans/README.md` status row updated.

## 9. STOP conditions

Stop and report (do not improvise) if:

- A **CommonMark/GFM/Markdown** golden snapshot changes during the Step 4 extraction — the refactor
  altered behavior. Do not `insta accept`; reconcile first.
- A `reader-ext` html case diverges from the oracle and the cause isn't resolved within two attempts
  — capture the input, carta's output, and the oracle's output. The §3 model was verified over a
  broad probe set; a new divergence is information.
- The `Note` post-pass needs the document mutated mid-build in a way that can't be expressed without
  slice indexing or panics — report the design rather than bending the panic rules.
- The minimal-feature build (`read-html`) fails to compile — the reader must not depend on the
  CommonMark feature being enabled (the shared scanner module must be reachable from both, or
  compiled unconditionally within `carta-readers`).
- `gfm_auto_identifiers` (Step 8) cannot be pinned to the oracle within two attempts — drop it to a
  follow-up and continue; this is expected, not a blocker.

## 10. Follow-ups (note in `plans/README.md` if useful)

- `gfm_auto_identifiers` slug in the HTML reader, if deferred from Step 8.
- HTML reader coverage of constructs the oracle keeps that carta still drops (audit `<video>`/
  `<iframe>`/`<audio>` — currently dropped by both; confirm and document, or implement if the oracle
  diverges in a context not probed here).
- Extend the shared inline-scanner module's reuse to any future lightweight reader that needs smart
  punctuation / inline math (rst, asciidoc readers when they land).
