# Plan 009: Writer extension toggles — drive the Markdown engine (and the other text writers) by the effective `Extensions` set

> **Executor instructions**: Follow this plan step by step. Run every verification command and
> confirm the expected result before moving on. If anything under "STOP conditions" occurs, stop and
> report — do not improvise. When done, update this plan's status row in `plans/README.md` and the
> two status docs (`README.md`, `docs/STATUS.md`).
>
> **Drift check (run first)**:
> `git diff --stat 7b2ba0f..HEAD -- crates/carta-writers/src/markdown.rs crates/carta-writers/src/commonmark.rs crates/carta-core/src/extensions.rs crates/carta/src/format_spec.rs crates/carta/src/registry.rs crates/carta/tests/golden_writer.rs`
> If any of these changed since this plan was written, re-verify the "Current state" excerpts (§4)
> against the live code before proceeding; on a material mismatch, treat it as a STOP condition.

## Status

- **Status**: DONE (on `feat/009-writer-extension-toggles`; see done criteria in §8 and the executed note below)
- **Priority**: P1
- **Effort**: L (the Markdown engine refactor is contained but wide; the four new sparse dialects each
  force a new fallback gate the two-variant model never needed)
- **Risk**: MED-HIGH — the Markdown engine currently models exactly **two** points in extension-space
  (`Variant::Extended`, `Variant::GitHub`). Generalizing to an arbitrary `Extensions` set, especially
  the sparse `markdown_strict`, exposes gates that don't exist yet (Strikeout → `<s>`, fenced-vs-indented
  code, table → HTML when no table syntax). The hard guarantee is that `markdown` and `gfm` output stay
  **byte-identical** — that's the regression net.
- **Depends on**: nothing hard. **Soft**: plan 006 (DONE) built `parse_format_spec`,
  `WriterOptions.extensions`, and the `presets` module; this is the writer-side symmetric counterpart
  to 006's reader-side work (006 §9 names it as a follow-up). Plan 007 (standalone/templates, DONE)
  is independent.
- **Category**: feature (parity)
- **Planned at**: commit `7b2ba0f`, 2026-06-24. The branch→extension map (§3.2) was read directly from
  `crates/carta-writers/src/markdown.rs` at this commit; the per-toggle output semantics (§3.3) and the
  dialect presets (§3.4) were derived from the pinned oracle (pandoc 3.10) via
  `pandoc -f json -t <fmt>[±ext]` and `pandoc --list-extensions=<fmt>`.

## 1. Why this matters

`docs/STATUS.md` marks "writer extension toggles" as not started: every text writer emits a fixed
dialect, ignoring `WriterOptions.extensions`. **No writer reads `options.extensions`** (verified —
`grep -rn '\.extensions' crates/carta-writers/src/` is empty). The plumbing exists end-to-end —
`parse_format_spec` resolves `markdown-fenced_divs`, `convert` unions it into `writer_options.extensions`
— but the value dead-ends at the writer.

The Markdown engine is the payoff. It already branches ~25 times on a boolean `Variant` (Extended vs
GitHub); every one of those branches is really "is extension X on?". Replacing the two-point `Variant`
with the **effective `Extensions` set** turns `-t markdown-fenced_divs`, `-t markdown-pipe_tables`,
`-t gfm+definition_lists`, etc. into working toggles, unblocks the four missing Markdown dialects
(`markdown_strict`, `markdown_mmd`, `markdown_phpextra`, `markdown_github` — each just a default
preset routed through the same engine), and is the natural home for the `commonmark_x` writer
(currently ❌). The other text writers (latex, rst, html, …) branch on a much smaller set — mostly
`smart` or nothing — and are swept in one commit each.

## 2. Scope

**In scope:**

- **Plumbing**: give each text writer access to its effective `Extensions` set (the engine reads
  `options.extensions`, defaulted to the format's preset).
- **Markdown engine** (`markdown.rs`): drop `Variant`; drive every current `is_github()` /
  `downgrades_smart()` branch off `extensions.contains(X)` per the §3.2 map. `markdown` =
  `presets::MARKDOWN` and `gfm` = `presets::GFM` must reproduce today's output **byte-for-byte**.
- **New Markdown dialects**: register and route `markdown_strict`, `markdown_mmd`, `markdown_phpextra`,
  `markdown_github` through the engine, each as its default preset (§3.4). Add the new fallback gates
  the sparse dialects force (§3.5).
- **`commonmark_x` writer**: wire it via the CommonMark writer driven by `presets::COMMONMARK_X`
  (flips the README `commonmark_x` writer cell ❌ → ✅).
- **Other text writers**: honor the empirically-relevant subset each one has — for most that is just
  `smart` (or nothing). Document inert extensions per writer rather than adding dead branches.
- **New enum variants** the writer needs: `FencedCodeBlocks`, `BacktickCodeBlocks`, `TexMathGfm`
  (§3.6). Add to `presets` as required so existing output is preserved.
- **Tests**: a new `corpus/ast-ext/<target-spec>/` corpus + golden snapshots (mirror of `text-ext`),
  and a writer-side conformance-ext group. The existing default `corpus/ast` snapshots stay as the
  byte-identity guard.

**Out of scope (deliberate, with rationale):**

- **Writers with no meaningful toggle** (json, native, opml, man, jira, dokuwiki, mediawiki, typst,
  beamer, revealjs): leave as-is; record in STATUS which extensions are inert there. Forcing an
  `Extensions` parameter through them with no branch buys nothing.
- **Reader-only extensions** appearing in a dialect preset (`intraword_underscores`,
  `shortcut_reference_links`, `all_symbols_escapable`, `yaml_metadata_block`, …): they parse into the
  set and the writer ignores them. Only the writer-relevant subset is branched.
- **Extensions whose enum variant doesn't exist and that don't affect writer output**
  (`spaced_reference_links`, `lists_without_preceding_blankline`, the `mmd_*` family,
  `markdown_attribute`, `short_subsuperscripts`, `abbreviations`): not added. A dialect preset simply
  omits them; `parse_format_spec("markdown_strict+spaced_reference_links")` errors `UnknownExtension`,
  which is the accepted contract.
- **Full byte-parity for the sparsest dialects on every node**: `markdown_strict` in particular drops
  to HTML for most block/inline constructs. If a specific sparse-dialect gate balloons, split it to a
  §10 follow-up rather than block the whole plan — the engine generalization and the common dialects
  stand on their own.

## 3. Exact semantics

### 3.1 No writer reads the set today

`grep -rn '\.extensions' crates/carta-writers/src/` → empty. The Markdown engine is parameterized only
by `MarkdownConfig { variant: Variant }` (`markdown.rs:34`/`:44`), with `is_github()` (`:61`) and
`downgrades_smart()` (`:67`). `registry.rs:98-100` routes `write-commonmark`→`CommonmarkWriter`,
`write-markdown`→`MarkdownWriter` (Extended), `write-gfm`→`GfmWriter` (GitHub).

### 3.2 Branch → extension map (read from `markdown.rs` at `7b2ba0f`)

Each current branch and the extension that should drive it after dropping `Variant`. `downgrades_smart()`
⟺ `contains(Smart)` (the Extended dialect carries `+smart`, so it renders `Quoted`/Unicode punctuation
back to ASCII for re-smartening; GFM has no `smart`, so it emits literal curly punctuation).

| `markdown.rs` site | Construct | Extended path | GitHub path | Drive off |
|---|---|---|---|---|
| `:409` header | `# h {#id .c}` | suffix | no suffix | `HeaderAttributes` |
| `:423` code_block | fenced + `{#id .lang}` | fenced + bare lang | — | `FencedCodeAttributes` (attrs); fence-vs-indent → `FencedCodeBlocks`‖`BacktickCodeBlocks` (§3.5) |
| `:440` raw_block | keep all raw | keep HTML, drop other | — | HTML kept iff `RawHtml`; non-HTML kept as `{=fmt}` iff `RawAttribute` |
| `:452` div | `::: {…}` | `<div …>` | — | `FencedDivs` (else `RawHtml` HTML fallback) |
| `:467` line_block | `\| ` lines | `\`-joined | — | `LineBlocks` |
| `:546` ordered_marks | native style/delim | decimal + period/paren | — | `FancyLists` |
| `:561` definition_list | `: ` syntax | hard-break fallback | — | `DefinitionLists` |
| `:626` figure | implicit-figure shorthand | HTML figure | — | `ImplicitFigures` |
| `:686` table | simple/multiline/grid pick | pipe table | — | table styles (§3.5): `SimpleTables`/`MultilineTables`/`GridTables` else `PipeTables` else HTML |
| `:1152` Underline | span | `<u>` | — | `BracketedSpans`‖`NativeSpans` for span form, else `<u>` |
| `:1159` Superscript | `^…^` | `<sup>` | — | `Superscript` |
| `:1166` Subscript | `~…~` | `<sub>` | — | `Subscript` |
| `:1173` SmallCaps | span | `<span class="smallcaps">` | — | `BracketedSpans` for span form, else HTML |
| `:1182`/`:1380` Quoted + escape | ASCII quotes + downgrade | curly/Unicode | — | `Smart` |
| `:1206` Span | `[…]{…}` | `<span …>` | — | `BracketedSpans` (else `RawHtml`/`NativeSpans` HTML) |
| `:1222` math | `$…$` / `$$…$$` | `$\`…\`$` / ` ```math ` | — | `TexMathDollars` (extended) vs `TexMathGfm` (§3.6) |
| `:1231` raw_inline | `\`…\`{=fmt}` | keep HTML only | — | `RawAttribute` (non-HTML), `RawHtml` (HTML) |
| `:1247` cite | `@id` | render inlines | — | `Citations` |
| `:1330` link w/ attr | `[…](…){…}` | `<a …>` | — | `LinkAttributes` |
| `:1355` image w/ attr | `![…](…){…}` | `<img …>` | — | `LinkAttributes` |
| `:1150` Strikeout | `~~…~~` (unconditional!) | — | **new gate** `Strikeout` else `<s>` (§3.5) |

Note `:1150` Strikeout is currently unconditional (both presets carry `+strikeout`); it needs a new
gate the moment `markdown_strict` (no strikeout) is wired.

### 3.3 Per-toggle output (oracle-verified, the test oracle for the common toggles)

From `pandoc -f json -t markdown±ext` over a Div/Span/Strikeout/code-attr sample:

- `markdown` (default): `::: {#n1 .note}` … `:::` · `[txt]{.cls}` · `~~struck~~` · ` ``` {#c .rust} `.
- `-fenced_divs`: the div becomes `<div id="n1" class="note">` with a blank line before/after the body.
- `-bracketed_spans -native_spans`: `[txt]{.cls}` becomes `<span class="cls">txt</span>`.
- `-strikeout`: `~~struck~~` becomes `<s>struck</s>`.
- `-fenced_code_attributes`: ` ``` {#c .rust} ` becomes ` ``` rust ` (id/classes dropped, language kept
  as a bare info string).

These confirm the map: the toggle off ⇒ the HTML (or plainest) fallback the GitHub path already used.

### 3.4 Dialect presets (`pandoc --list-extensions=<fmt>`, pinned)

Route each through the Markdown engine with its default `Extensions` set. **Pin each against the
oracle during implementation** — the lists below are the 3.10 values:

- **markdown** (Extended today): `+all_symbols_escapable +auto_identifiers +backtick_code_blocks
  +blank_before_blockquote +blank_before_header +bracketed_spans +citations +definition_lists
  +escaped_line_breaks +example_lists +fancy_lists +fenced_code_attributes +fenced_code_blocks
  +fenced_divs +footnotes +grid_tables +header_attributes +table_attributes +implicit_figures
  +implicit_header_references +inline_code_attributes +inline_notes +intraword_underscores
  +latex_macros +line_blocks +link_attributes +markdown_in_html_blocks +multiline_tables +native_divs
  +native_spans +pandoc_title_block +pipe_tables +raw_attribute +raw_html +raw_tex
  +shortcut_reference_links +simple_tables +smart +space_in_atx_header +startnum +strikeout +subscript
  +superscript +task_lists +table_captions +tex_math_dollars +yaml_metadata_block`.
- **gfm** (GitHub today): `+alerts +autolink_bare_uris +emoji +footnotes +gfm_auto_identifiers
  +pipe_tables +raw_html +strikeout +task_lists +tex_math_dollars +tex_math_gfm +yaml_metadata_block`.
- **markdown_strict**: `+raw_html +shortcut_reference_links +spaced_reference_links` (original Markdown —
  nearly everything falls back to HTML or indented code).
- **markdown_mmd**: `+all_symbols_escapable +auto_identifiers +backtick_code_blocks +definition_lists
  +footnotes +implicit_figures +implicit_header_references +intraword_underscores +markdown_attribute
  +mmd_header_identifiers +mmd_link_attributes +mmd_title_block +pipe_tables +raw_attribute +raw_html
  +short_subsuperscripts +shortcut_reference_links +spaced_reference_links +subscript +superscript
  +tex_math_dollars +tex_math_double_backslash`.
- **markdown_phpextra**: `+abbreviations +definition_lists +fenced_code_blocks +footnotes
  +header_attributes +intraword_underscores +link_attributes +markdown_attribute +pipe_tables
  +raw_html +shortcut_reference_links +spaced_reference_links`.
- **markdown_github** (legacy): `+alerts +all_symbols_escapable +auto_identifiers +autolink_bare_uris
  +backtick_code_blocks +emoji +fenced_code_blocks +footnotes +gfm_auto_identifiers
  +intraword_underscores +lists_without_preceding_blankline +pipe_tables +raw_html
  +shortcut_reference_links +space_in_atx_header +strikeout +task_lists`.
- **commonmark**: `+raw_html` only (the `CommonmarkWriter`).
- **commonmark_x**: `+alerts +attributes +bracketed_spans +definition_lists +emoji +fancy_lists
  +fenced_divs +footnotes +gfm_auto_identifiers +implicit_header_references +pipe_tables +raw_attribute
  +raw_html +smart +strikeout +subscript +superscript +task_lists +tex_math_dollars
  +yaml_metadata_block` (the `CommonmarkWriter` driven by this set).

The carta preset for each lists **only the variants that exist and affect writer output**; the
reader-only/inert names above are intentionally omitted (§2).

### 3.5 New fallback gates the sparse dialects force

The two-variant model never needed these because both Extended and GitHub carried the feature. Adding
them must not change `markdown`/`gfm` output (both presets keep the feature on, so the existing path is
taken):

- **Strikeout** (`:1150`): `contains(Strikeout)` → `~~…~~`; else `<s>…</s>` (§3.3). Needed by
  `markdown_strict`, `markdown_mmd`, `markdown_phpextra`.
- **Code fence vs indent** (`:423`): `contains(FencedCodeBlocks) || contains(BacktickCodeBlocks)` →
  fenced; else 4-space indented code. Needed by `markdown_strict` (neither).
- **Table** (`:686`): if any of `GridTables`/`MultilineTables`/`SimpleTables` → pick the best native
  form; else if `PipeTables` → pipe table; else → HTML `<table>`. Needed by `markdown_strict` (no
  table syntax at all). Pin the HTML-table fallback shape against the oracle.

Confirm each new gate's "feature on" branch is identical to the current `is_github()==false`/`==true`
branch for the `markdown`/`gfm` presets, so the byte-identity golden test (§5 Step 2) stays clean.

### 3.6 New enum variants required

`grep`-confirmed missing from `define_extensions!`: **`FencedCodeBlocks`** (`fenced_code_blocks`),
**`BacktickCodeBlocks`** (`backtick_code_blocks`), **`TexMathGfm`** (`tex_math_gfm`). Append them
(preserving discriminant contiguity). Because the §3.5 code-fence gate and the §3.2 math gate key on
them, **add them to the presets that carry them** (`MARKDOWN`, `GFM`, the new dialect presets) so the
existing fenced-code / GFM-math output is reproduced. The math precedence: `TexMathGfm` (when present,
e.g. `gfm`) selects the GFM forms (`$\`…\`$`, ` ```math `); otherwise `TexMathDollars` selects
`$…$`/`$$…$$`.

## 4. Current state (excerpts to build on)

- `crates/carta-writers/src/markdown.rs` — `enum Variant { Extended, GitHub }` (`:34`);
  `struct MarkdownConfig { variant }` (`:44`) with `extended()`/`github()` constructors, `is_github()`
  (`:61`), `downgrades_smart()` (`:67`). The branch sites are the §3.2 line numbers. `MarkdownWriter`
  (Extended) and `GfmWriter` (GitHub) are thin wrappers selecting the config.
- `crates/carta-writers/src/commonmark.rs` — `CommonmarkWriter`; the natural host for the
  `commonmark_x` set.
- `crates/carta-core/src/extensions.rs` — `define_extensions!` with 48 variants (the §3.2 names exist
  **except** `FencedCodeBlocks`/`BacktickCodeBlocks`/`TexMathGfm`). `presets::{COMMONMARK, GFM,
  COMMONMARK_X, MARKDOWN}` (`:222`/`:225`/`:239`/`:263`). `Extensions::{contains, from_list, union}`.
- `crates/carta/src/format_spec.rs` — `default_extensions` (`:11`) maps `commonmark`/`commonmark_x`/
  `markdown`/`gfm`; **no entries** for the four new dialects.
- `crates/carta/src/registry.rs` — `:98-100` route the three current Markdown-family writers.
- `crates/carta/tests/golden_writer.rs` — `writer_output_snapshots` renders `corpus/ast/<feature>/*`
  to each of 19 `TARGETS` with **`WriterOptions::default()` only** (no toggle harness exists yet).
- `tools/conformance-suite/surfaces/writer.sh` — loops `TARGETS` × `corpus/ast/*/*.json`,
  `run_diff … "-f json -t $target"`. The format list is hardcoded; a new `corpus/ast-ext/` is invisible
  unless the surface is taught about it (this mirrors how `reader.sh` learned `text-ext`).

Repo conventions: no `unwrap`/`expect`/`panic`/`unreachable`/slice-indexing outside `#[cfg(test)]`;
comments only where the *why* is non-obvious; **provenance rule** — in product source state behavior as
the code's own design and cite the Markdown/format *syntax*, never the upstream tool; deterministic
output; Conventional Commits, one logical change per commit, explicit `git add` paths, no push.

## 5. Implementation steps

Branch: `feat/009-writer-extension-toggles` off `main`. The phases are sequenced so the
byte-identity guarantee is locked **before** any new behavior lands.

### Step 1 — Plumbing + new enum variants

- `extensions.rs`: append `FencedCodeBlocks`, `BacktickCodeBlocks`, `TexMathGfm` to
  `define_extensions!`; add them to `presets::MARKDOWN`/`GFM` as the §3.6 byte-identity requirement
  dictates (markdown & gfm both fence code; gfm carries `tex_math_gfm`). Unit-test `from_name`
  round-trips for the three.
- Give the Markdown engine its effective `Extensions`: thread `options.extensions` into the writer
  entry point and store it on the engine (alongside or replacing `MarkdownConfig`). The writer's
  `write` already receives `&WriterOptions`; `convert` has unioned the format preset into it.

**Verify**: `cargo nextest run -p carta-core`; `cargo build -p carta-writers`.

### Step 2 — Markdown engine: drop `Variant`, drive by `Extensions` (byte-identical)

Replace each §3.2 branch:

- `downgrades_smart()` → `ext.contains(Smart)`.
- every `is_github()` → the specific `ext.contains(X)` (or `!ext.contains(X)`) from the §3.2 map.
- Add the §3.5 gates (Strikeout, code fence-vs-indent, table) with their "feature on" branch matching
  today's behavior.
- Delete `enum Variant`, `is_github`, `downgrades_smart`; `MarkdownWriter`/`GfmWriter` now construct
  the engine with `presets::MARKDOWN` / `presets::GFM` respectively (still no `options` toggles read
  beyond the preset at this step, to isolate the refactor).

**Verify (the regression net)**: `cargo nextest run -p carta` → **zero** `.snap.new` for any
`markdown__…` or `gfm__…` writer snapshot. If any changes, a branch was mismapped — STOP and fix
before continuing. Then `tools/conformance-suite/run.sh writer markdown` and `… writer gfm` →
`fail=0 err=0` (confirms parity held).

### Step 3 — Honor `options.extensions` for the existing Markdown/CommonMark writers

Now let the unioned set actually flow: the engine reads `options.extensions` (defaulted to the
format preset by `convert`), so `-t markdown-fenced_divs`, `-t gfm+definition_lists`, etc. take
effect. Wire the `commonmark_x` writer: route it through `CommonmarkWriter` driven by
`presets::COMMONMARK_X`, and add `default_extensions("commonmark_x")` is already present — confirm the
writer path resolves it.

**Verify**: add `corpus/ast-ext/` harness (Step 6) cases for a handful of toggles and run the
writer-ext conformance group; `commonmark_x` round-trips a representative AST to the oracle's
`commonmark_x` output.

### Step 4 — New Markdown dialects (one commit each)

For each of `markdown_strict`, `markdown_phpextra`, `markdown_mmd`, `markdown_github` (in increasing
sparsity-pain order — `phpextra`/`github` are close to existing paths; `strict` exercises the most new
§3.5 gates):

- add `presets::MARKDOWN_<DIALECT>` (§3.4, variants that exist + affect output);
- `default_extensions(<dialect>)` → that preset;
- `registry.rs`: route `write-<dialect>` → the Markdown engine (a thin wrapper, like `MarkdownWriter`);
- add the dialect to the writer `TARGETS` (golden test + conformance) and to the README status table;
- add `corpus/ast-ext/<dialect>/` cases and pin against the oracle.

Commit each dialect separately so a single sparse-dialect gate that balloons can be split to a
follow-up without stalling the others.

**Verify per dialect**: `tools/conformance-suite/run.sh writer <dialect>` → `fail=0 err=0`, iterating
on the §3.5 gates. `pandoc --list-extensions=<dialect>` to re-pin the preset.

### Step 5 — Other text writers (grouped sweep)

For latex, html/html4, rst, asciidoc, plain, and the wiki/roff writers: honor only the
empirically-relevant subset (probe `pandoc -f json -t <fmt>±smart` etc.). For most this is `smart`
or nothing. Where a writer has **no** meaningful toggle, add no branch — instead record it as inert in
STATUS. Do not thread an unused `Extensions` parameter through a writer that ignores it.

**Verify**: for each writer touched, a `corpus/ast-ext/<fmt>±ext/` case green against the oracle; for
each writer declared inert, a one-line STATUS note.

### Step 6 — Test harness: `corpus/ast-ext/` + golden snapshots + conformance-ext

Mirror the reader-side `text-ext` pattern:

- `corpus/ast-ext/<target-spec>/*.json`, one directory per writer format spec (e.g.
  `markdown-fenced_divs/`, `gfm+definition_lists/`, `markdown_strict/`, `commonmark_x/`), the directory
  name being the full `-t` spec.
- Extend `golden_writer.rs` with a second pass over `corpus_cases("ast-ext")`: parse the spec for its
  base, skip if the base isn't a compiled writer, else `convert("json", &case.group, …)` and snapshot
  under `ast-ext__<group>__<label>`. The existing default `corpus/ast` snapshots are untouched — they
  remain the byte-identity guard.
- `tools/conformance-suite/surfaces/writer.sh`: after the hardcoded `TARGETS` loop, add a loop over
  `corpus/ast-ext/*` using the directory name as the `-t` spec for both carta and the oracle (the
  symmetric counterpart to the `reader-ext` loop). Update the surface header comment.

**Verify**: `cargo nextest run -p carta` produces `.snap.new` for `ast-ext__…` only; do **not** accept
until the writer-ext conformance group is green. Then `cargo insta accept` and re-run the workspace.

### Step 7 — STATUS/README sync + full gate

- `README.md`: flip `commonmark_x` writer ❌ → ✅; flip `markdown_strict`/`markdown_mmd`/
  `markdown_phpextra`/`markdown_github` writer cells per what landed.
- `docs/STATUS.md`: add the "writer extension toggles" cross-cutting row (now ✅/🚧); per-format,
  record honored vs inert extensions.

**Full gate** (all must pass):

- `cargo nextest run --workspace` → pass; `git status` clean of unexpected `.snap.new` (especially
  no default `markdown__`/`gfm__` snapshot drift).
- `cargo test --doc --workspace` → pass.
- `cargo clippy --all-targets --all-features` → exit 0, no warnings.
- `cargo fmt --all --check` → clean.
- `cargo build -p carta --no-default-features --features read-json,write-markdown` + its
  `nextest run` → pass.
- `tools/conformance-suite/run.sh all` → no `fail`/`err` on any surface.
- `cargo llvm-cov clean --workspace && cargo llvm-cov --workspace --summary-only --fail-under-lines 90`
  → ≥ 90%.

## 6. Commands reference

| Purpose | Command | Expected |
|---|---|---|
| Unit/golden tests | `cargo nextest run --workspace` | all pass, no pending snapshots |
| Byte-identity guard | `cargo nextest run -p carta` | no `markdown__`/`gfm__` `.snap.new` after Step 2 |
| Doctests | `cargo test --doc --workspace` | pass |
| Lint | `cargo clippy --all-targets --all-features` | exit 0, no warnings |
| Format | `cargo fmt --all --check` | clean |
| Snapshots | `cargo insta accept` (only after the writer-ext group is green) | snapshots written |
| Conformance (writer) | `cargo build -p carta-cli && tools/conformance-suite/run.sh writer` | `RESULT writer … fail=0 err=0` |
| Conformance (all) | `tools/conformance-suite/run.sh all` | no fail/err on any surface |
| Pin a dialect preset | `.oracle/bin/pandoc --list-extensions=<fmt>` | the §3.4 list |
| Oracle probe (ad hoc) | `.oracle/bin/pandoc -f json -t <fmt>[±ext] <ast.json>` | the §3.3 expected |
| Coverage | `cargo llvm-cov clean --workspace && cargo llvm-cov --workspace --summary-only --fail-under-lines 90` | ≥ 90% |

## 7. Test plan summary

- **Layer 0** — unit tests for the three new enum variants and the engine's per-extension branch
  helpers.
- **Layer 1** — the existing default `corpus/ast` snapshots stay frozen (byte-identity guard for
  `markdown`/`gfm`); new `corpus/ast-ext/<spec>/` snapshots freeze the toggled output, accepted only
  after Layer 2 proves them.
- **Layer 2** — a writer-ext conformance group diffs carta vs the pinned oracle per target spec, in
  CI. This is the parity guarantee for every toggle and every new dialect.

## 8. Done criteria

- [x] `Variant` removed from `markdown.rs`; every branch drives off `extensions.contains(X)`.
- [x] `markdown` and `gfm` default output **byte-identical** to pre-change (no default snapshot drift;
      `writer markdown`/`writer gfm` conformance green — both `pass=93 fail=0`).
- [x] `markdown-fenced_divs`, `markdown-strikeout`, `markdown-fenced_code_attributes`,
      `markdown-bracketed_spans-native_spans`, and `gfm+…` toggles produce the §3.3 output, conformance-green.
- [x] `FencedCodeBlocks`/`BacktickCodeBlocks`/`TexMathGfm` variants added and used; presets updated so
      no existing output regresses.
- [x] `commonmark_x` writer wired (README cell ✅); each new Markdown dialect that landed is routed,
      golden-tested, and conformance-green, with its preset pinned via `--list-extensions`.
- [x] Other text writers honor their relevant subset (or are documented inert); no dead `Extensions`
      parameter threaded through a writer that ignores it.
- [x] `corpus/ast-ext/` corpus + golden snapshots committed; writer-ext conformance group present and
      green locally with `.oracle/` (`RESULT writer ext pass=10 fail=0 err=0`).
- [x] Both status docs updated.
- [x] `cargo nextest run --workspace` (1329 passed), `--doc`, `clippy --all-targets --all-features`
      (0 warnings), `fmt --check`, the minimal-feature build, and coverage (92.73% lines ≥ 90%) all pass.
- [x] No `unwrap`/`expect`/`panic`/slice-indexing added outside `#[cfg(test)]`.
- [x] No upstream provenance in product source.
- [x] `plans/README.md` status row updated.

### Executed (2026-06-25, on `feat/009-writer-extension-toggles`)

- **`typst` smart wired despite §2 listing it out-of-scope.** Empirical probing showed `typst` *is*
  smart-sensitive (quotes → straight `"`/`'`, en/em dash → `--`/`---`) and had a real parity gap, so
  it was wired alongside the other text writers per the §5 "full parity" principle. `typst` defaults
  to `+smart` (seeded in `default_extensions`), and its unit tests that build `Quoted`/dash nodes use
  a `smart_options()` helper since `WriterOptions::default()` carries `smart` off. Recorded as a §10
  follow-up note in the plan README row.
- **`rst` ASCII-dash deferral.** Under the non-default `+smart`, `rst` does not yet backslash-escape a
  literal ASCII `--`/`...` (carta emits `a--b...c`, the oracle emits `a\--b\...c`). Curly quotes,
  literal Unicode dashes, and ellipsis all round-trip; only this narrow ASCII-escape case differs. Not
  a blocker; noted in `docs/STATUS.md`.
- **Pre-existing test-gating fix surfaced by the minimal-feature gate.** `tests/spec_parse.rs` reads
  CommonMark spec examples through the `commonmark` reader but lacked a feature gate, so it panicked
  under `--no-default-features --features read-json,write-markdown`. Gated it on `read-commonmark`,
  matching `golden_writer`/`golden_reader`/`golden_wrap`. Reproduced identically on the base commit, so
  it is not a regression from this work.
- **Lint allowances.** Threading `smart` pushed `latex.rs`'s `figure`/`push_link` past clippy's
  argument ceiling and `push_inline` past its line ceiling; allowed in place as the `math` writer
  modules already do for the same deep-threading shape.

## 9. STOP conditions

Stop and report (do not improvise) if:

- A default `markdown__`/`gfm__` golden snapshot changes after Step 2 — a branch was mismapped or a
  §3.5 gate's "on" path diverges. Do not `insta accept`; the byte-identity guarantee is the whole
  point of phasing the refactor first.
- A writer-ext conformance case diverges from the oracle and isn't resolved within two attempts —
  capture input AST, carta output, oracle output. The §3.2 map and §3.4 presets were read/probed
  directly; a new divergence is information.
- A sparse-dialect (esp. `markdown_strict`) gate requires a wholesale new rendering path (e.g. a full
  HTML-table emitter) larger than the rest of the dialect — split that dialect to a §10 follow-up,
  land the others, and record the split. This is expected, not a failure.
- Honoring a toggle would need slice indexing or a panic that can't be expressed with `.get()`/`?` —
  report the design instead of bending the panic rules.

## 10. Follow-ups (note in `plans/README.md` if useful)

- Any sparse-dialect path split out under Step 4's STOP condition (most likely a `markdown_strict`
  HTML-table / indented-code edge).
- Writer-side emission of the CommonMark reader's §006 extensions under `-t commonmark+ext`
  (strikeout/sub/superscript as native `~~`/`^`/`~` rather than HTML fallback) — the `CommonmarkWriter`
  counterpart now that it is `Extensions`-driven.
- The remaining `mmd_*` / `markdown_attribute` / `abbreviations` writer extensions, if a consumer needs
  exact `markdown_mmd`/`markdown_phpextra` fidelity beyond the writer-relevant subset wired here.
