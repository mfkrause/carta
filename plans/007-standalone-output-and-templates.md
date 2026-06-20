# Plan 007: Standalone output + template engine (and the metadata/variable context it requires)

> **Executor instructions**: Follow this plan step by step. Run every verification command and
> confirm the expected result before moving on. If anything under "STOP conditions" occurs, stop and
> report — do not improvise. When done, update this plan's status row in `plans/README.md`.
>
> **Drift check (run first)**:
> `git diff --stat afb98a2..HEAD -- crates/carta-core/src/lib.rs crates/carta/src/lib.rs crates/carta-cli/src/main.rs crates/carta-writers/src/lib.rs crates/carta-readers/src/commonmark/frontmatter.rs crates/carta-ast/src/ast.rs`
> If any of these changed since this plan was written, re-verify the "Current state" excerpts and the
> integration points in §4 against the live code before proceeding; on a material mismatch, treat it
> as a STOP condition.

## Status

- **Status**: TODO
- **Priority**: P1 (the single feature that turns fragment output into usable documents; gates several
  other cross-cutting rows)
- **Effort**: L (engine + context model + per-writer integration + **13** default templates + a new
  conformance surface). Lands as a sequence of PRs/commits; see §6.
- **Risk**: MED–HIGH. The engine's whitespace/indentation rules and "metadata rendered through the
  target writer" are the subtle parts. Mitigated by a byte-exact differential gate on the engine
  (§7.1) and exhaustive Layer-0 tests on the whitespace rules (§6 Step 1).
- **Depends on**: nothing hard. Soft: cleaner to land after, not before, in-flight writer work, since
  it adds provided methods to the `Writer` trait (§4.3).
- **Category**: feature (cross-cutting)
- **Planned at**: commit `afb98a2`, 2026-06-19. The template-language and metadata semantics in §3 and
  §5 were derived empirically from the pinned oracle (pandoc 3.10) via `--template`, `-V`, `-M`,
  `--metadata-file`, and `-s` probes — the sanctioned clean-room source (observable CLI behavior). They
  are reproduced here so the executor needs no oracle to understand the target; the conformance layer
  (§7) re-checks them live.

## Decisions resolved (design review, 2026-06-19)

These were settled with the maintainer; do not relitigate without cause.

1. **Completeness = all 13 templates.** The `docs/STATUS.md` *Standalone* row flips to `✅` only when
   every writer pandoc wraps under `-s` has a carta-authored default template (§4.5). Staged across
   PRs (§6) but not declared done until all 13 land.
2. **Default flavor = same structure, carta's own CSS/preamble.** Reproduce the *documented scaffold
   structure* (same variable slots, same head/preamble layout, complete+valid documents) but author
   carta's own CSS and LaTeX package selection. **Guardrail**: author the CSS/preamble from what a good
   standalone document needs — do **not** observe pandoc's CSS and reword it; transcribe-and-tweak is
   still derivative (§4.1).
3. **Default-template gate = engine byte-diff + own snapshots + structural oracle checks** (§7).
4. **Metadata flow = merge into `Document.meta` before write.** The facade merges `--metadata-file`
   (lowest) → reader's `document.meta` → `-M` (highest) into one `Document.meta`; `-V` is a separate
   raw-variable overlay applied on top at context-build (§4.4).
5. **`--metadata-file` = full parity, reuse commonmark.** Its values are Markdown-parsed (→
   `MetaInlines`); reuse the commonmark reader's existing YAML→`MetaValue` machinery via a new public
   entry point. Gated so it can never produce a broken build (decision 6).
6. **Feature taxonomy = layered.** `carta-core` gets a `template` feature (the engine). `carta` gets
   `standalone = ["carta-core/template"]` (engine + wrapping + `-s`/`--template`/`-V`/`-M`; works with
   any reader/writer) and `metadata-file = ["standalone", "read-commonmark"]` (adds `--metadata-file`,
   transitively pulling commonmark so the broken combination is unrepresentable). `full`/`default`
   include both (§4.7).

## Scope and STATUS impact

This plan **completes two `docs/STATUS.md` cross-cutting rows**, because they are one indivisible unit:

- **Standalone output + templates (`-s`)** — the headline deliverable.
- **Metadata / variables (`-M`, `-V`, `--metadata-file`)** — a *hard prerequisite*. A template engine
  with no variable/metadata context renders nothing useful; `-V` supplies template variables and the
  title/author/date come from metadata. There is no coherent way to ship standalone without it.

Two adjacent rows are **out of scope** and remain `❌`, but their template **slots are wired and left
inert**:

- **Table of contents (`--toc`)** — default templates carry a `$if(toc)$ … $toc$ … $endif$` slot; with
  no `--toc` flag the `toc` variable is unset and output matches `pandoc -s` *without* `--toc`. TOC
  *generation* is the TOC row's job. **Do not** add the `--toc` flag here.
- **Section numbering (`--number-sections`)** — a body-level header transform; its own plan. **Do not**
  add the `--number-sections` flag here.

Also out of scope (slots left inert): syntax-highlighting CSS (`$highlighting-css$`), math output
methods, filters, `--data-dir`, multiple-input/defaults files.

Rationale for the cut line: TOC and number-sections are *consumers* that plug into standalone, not
prerequisites; folding them in would conflate three rows. The prerequisite (metadata/variables) is
folded in because standalone genuinely cannot exist without it.

**On completion**, update `docs/STATUS.md`: flip **Standalone** and **Metadata / variables** to `✅`,
and note on the TOC / Section-numbering / Math rows that their template slots now exist. (`README.md`
Status table is format-level and has no row for these features.)

## 1. Why this matters

Every writer emits a **document fragment** today. `echo '# Hi' | carta -f commonmark -t html` yields
`<h1>Hi</h1>` — not an openable file (no doctype, `<head>`, charset, `<title>`); `-t latex` yields a
body with no `\documentclass`/`\begin{document}`, so it will not compile. Fragments are only useful for
embedding or piping. Standalone output (`-s`) wraps the fragment in the boilerplate that makes a
complete, valid document, with metadata interpolated into the right slots.

It cannot be hardcoded per writer, because the boilerplate must be user-overridable (`--template`),
filled from document metadata and CLI variables, and conditional/iterative (`$if(author)$`,
`$for(author)$`). That is a small templating language plus a variable context — the two things this
plan builds. It also unblocks `--toc`, section numbering, highlighting CSS, and math-method `<head>`
injection, which all flow through the same context once it exists.

## 2. Current state

- `WriterOptions` carries only `extensions` and is `#[non_exhaustive]`, so it can gain fields without a
  breaking change (`crates/carta-core/src/lib.rs`):

  ```rust
  #[derive(Debug, Clone, Default)]
  #[non_exhaustive]
  pub struct WriterOptions {
      /// Format extensions to enable.
      pub extensions: Extensions,
  }
  ```

- The `Writer` trait is fragment-only with **no** hook to render a `Vec<Inline>`/`MetaValue`; each
  writer's inline/block rendering is private (only `impl Writer for X` is public, e.g.
  `crates/carta-writers/src/html.rs:22`, `latex.rs:27`). This is the central integration challenge
  (§4.3). The trait's doc states: *"The returned string carries no trailing newline; the CLI appends
  exactly one."*

- `convert()` is a linear read→write pipeline with no standalone branch
  (`crates/carta/src/lib.rs:36`): `let document = reader.read(...)?; writer.write(&document, …)`.

- `Document` **derives `Default`** (`crates/carta-ast/src/ast.rs:9`) — the §4.3 wrap trick can use
  `Document { blocks: …, ..Default::default() }`.

- Metadata exists end-to-end: readers populate `Document.meta: BTreeMap<Text, MetaValue>`
  (`ast.rs:57`); `MetaValue` variants are `MetaMap`, `MetaList`, `MetaBool`, `MetaString`,
  `MetaInlines`, `MetaBlocks` (`ast.rs:136`). **No writer reads `document.meta` today** — this plan is
  the first consumer. `BTreeMap` is sorted, which **matches** pandoc's metadata ordering (§5).

- The commonmark reader already converts YAML → `MetaValue` **with Markdown inline parsing** of scalars
  in `crates/carta-readers/src/commonmark/frontmatter.rs` (`yaml_to_meta`/`scalar_to_meta`/
  `parse_meta_inlines`) and tokenizes YAML in `commonmark/yaml.rs` (`pub(crate) fn parse`,
  `yaml.rs:48`). This is exactly what `--metadata-file` needs (§4.6) — reuse it, do not reimplement.

- The CLI (`crates/carta-cli/src/main.rs:22`) has six flags; `convert_document` always passes
  `WriterOptions::default()` (`main.rs:79`); `write_output` **unconditionally appends `\n`** at
  `main.rs:127` (a trap for standalone — §4.6).

- Reusable helpers: `carta_ast::to_plain_text(&[Inline]) -> String` and `carta_ast::slug(&str)`
  (`ast.rs:296`) — `to_plain_text` builds the plain-text `pagetitle` (§4.4).

- Tests: Layer-1 `insta` snapshots in `crates/carta/tests/golden_writer.rs`; Layer-2 conformance
  surfaces `reader|writer|e2e|roundtrip|commands` in `tools/conformance-suite/`, each emitting
  `RESULT <surface> <group> pass=N fail=N err=N skip=N`. carta has an **HTML reader**, reused for
  structural checks (§7.2).

## 3. The template language (empirical spec — reproduced; conformance re-checks live)

Delimiter `$…$`. All confirmed against the pinned oracle.

### 3.1 Tokens and values

| Construct | Syntax | Behavior (confirmed) |
| --- | --- | --- |
| Literal dollar | `$$` | emits a single `$` |
| Comment | `$-- text` | consumes to end of line; the line's **newline is preserved** |
| Variable | `$x$` | value, or empty if absent |
| Nested field | `$x.y.z$` | walks `MetaMap`s; empty if any hop missing |
| Conditional | `$if(x)$…$elseif(y)$…$else$…$endif$` | truthiness in §3.2 |
| Loop | `$for(x)$…$sep$…$endfor$` | iterates a list; a scalar acts as a 1-element list |
| Loop item | inside `$for(x)$`: `$x$`, `$x.field$`, **`$it$`**, `$it.field$` | current element |
| Map → pairs | `$for(x/pairs)$$it.key$=$it.value$$endfor$` | iterate a map as key/value records; **sorted** by key (§5) |
| Pipe | `$x/uppercase$`, chained `$x/uppercase/reverse$` | §3.3 |
| Partial | `$name()$` | include partial file (§3.4) |
| Mapped partial | `$xs:name()$` | apply partial to each element of `xs`; element is `$it$` inside |
| Mapped partial + sep | `$xs:name()[, ]$` | join mapped results with the literal in `[…]` |

A list variable interpolated directly (`$xs$`, `xs=[a,b]`) concatenates with **no** separator (`ab`).
Missing variable → empty. With a `--template`, output is **exactly** the rendered template — pandoc
adds **no** implicit trailing newline (§4.6).

### 3.2 Truthiness (for `$if$`)

Falsy: absent; empty string `""`; empty list `[]`; `MetaBool(false)`. Everything else truthy.

### 3.3 Pipes / filters

**Must-have** (confirmed or documented; cover before "done"): `uppercase`, `lowercase`, `length`
(list→count), `reverse`, `first`, `last`, `rest`, `allbutlast`, `pairs` (map→sorted list of
`{key,value}`), `alpha` (3→`c`), `roman` (3→`iii`), `chomp` (strip trailing newlines), `nowrap`.
**Nice-to-have** (implement if cheap, else record as a known gap): `left`/`right`/`center` with a width
arg and optional borders.

**Oracle-confirmed pipe edge semantics** (baked into the engine + unit tests):
- `first`/`last`/`rest`/`allbutlast` operate on **lists only**; a string (or any non-list) passes
  through unchanged. `first`/`last` on an empty list select the empty string.
- `alpha` is **single-letter cyclic**, not spreadsheet-style: `n` → the lowercase letter at
  `chr(96 + n mod 26)`, so `1`→`a` … `25`→`y`, and the cycle boundary `0`/`26`/`52`/… lands on
  `` ` `` (the character just before `a`); `27`→`a`. Negative or non-integer values pass through as
  their own text.
- `roman`: `0`→`""` (empty); `1..=3999`→standard lowercase numeral; negative or non-integer values
  pass through unchanged. **Known divergence (out of domain):** inputs `≥4000` are not standard Roman
  numerals; the engine continues the greedy expansion (`4000`→`mmmm`) rather than reproducing the
  pinned binary's overflow artifacts (`4000`→`cmmmmc`). No authored template uses `roman` with such
  values, so the differential surface never exercises this.

### 3.4 Partials

`$name()$` resolves to a file `name.<ext>` where `<ext>` is the **enclosing template's extension**,
searched in the enclosing template's directory. carta's own default templates **avoid partials**
(inline everything) so the embedded set is self-contained; partials must still work for **user**
`--template` files. `$xs:name()$` maps the partial over `xs` (`$it$` inside); `$xs:name()[SEP]$` joins
with `SEP`.

### 3.5 Whitespace and indentation (highest-risk; test exhaustively)

1. **A control directive alone on its line consumes the whole line.** If `$if$`/`$elseif$`/`$else$`/
   `$endif$`/`$for$`/`$sep$`/`$endfor$` is the only non-whitespace on a line, that line **and its
   trailing newline** vanish. Confirmed:

   ```
   template:  START\n$if(a)$\nLINE-A\n$endif$\nEND\n
   a set   →  START\nLINE-A\nEND\n
   a unset →  START\nEND\n
   ```

2. **Indentation before a variable is applied to every line it expands to.** Confirmed:

   ```
   template:  "body below:\n    $body$\nDONE\n"   (body = two paragraphs)
   output:    "body below:\n    <p>line1</p>\n    <p>line2</p>\nDONE\n"
   ```

   Same rule for an indented `$for$` body.

## 4. Architecture

Three layers: a format-agnostic **engine** (`carta-core`), a **context builder** that merges metadata
and renders it through the target writer (`carta` facade), and **per-writer default templates** plus a
small `Writer`-trait extension.

### 4.1 Clean-room boundary (the design's spine — do not cross it)

pandoc's default templates embed pandoc's own CSS (~3.9 KB in the HTML default) and LaTeX preamble
(~1.8 KB) — upstream data files. **Reproducing them byte-for-byte would commit upstream-derived
content**, which the clean-room rule forbids. Therefore:

- carta **authors its own** default templates and default CSS/preamble (decision 2: same *structure*,
  own *content*). Consequently `carta -s -t html` will **not** be byte-identical to `pandoc -s` — **and
  that is correct**; it must never be a gate. **Guardrail**: write the CSS/preamble from first
  principles (what a clean standalone document needs). Do **not** open pandoc's CSS and reword it.
- What carta *does* reproduce faithfully (documented public interface, same category as the JSON-AST
  contract — allowed): (a) the **template language** (§3); (b) the **variable-name vocabulary**
  (`title`, `pagetitle`, `author`, `date`, `subtitle`, `abstract`, `lang`, `body`, `toc`,
  `header-includes`, `include-before`, `include-after`, `css`, …) so user `--template`/`-V` are
  portable; (c) the **CLI flags**.
- **Never** run `pandoc --print-default-template …` and copy/reword its output. **Never** commit pandoc
  output as golden values (§8).

This boundary makes verification split cleanly (§7): **engine + context** are gated **byte-exactly**
against the oracle by feeding the *same carta-authored template* to both binaries (templates are inputs
we own → no provenance issue, and the hard logic is fully pinned); the **default templates** are gated
by carta's **own** snapshots plus structural oracle assertions, never a byte diff.

### 4.2 Engine (`carta-core::template`, behind feature `template`)

New module, pure, no I/O. Public surface roughly:

```rust
pub enum Value {
    Str(String),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),   // BTreeMap: deterministic + matches pandoc's sorted maps (§5)
    Bool(bool),                     // renders bare as "true"/"false" (§5); falsy-when-false in $if$
}

pub struct Template(/* parsed nodes */);

impl Template {
    pub fn parse(src: &str) -> Result<Template, TemplateError>;
    /// `resolve_partial` loads a partial by name; pass a no-op for templates without partials.
    pub fn render(&self, ctx: &Value, resolve_partial: &dyn Fn(&str) -> Option<String>) -> String;
}
```

- Parser → node AST: `Literal`, `Var(path, pipes)`, `If(branches, else)`, `For(path, body, sep)`,
  `Partial { name, map_over: Option<path>, sep: Option<String> }`, with §3.5 line-level whitespace
  decisions baked in at parse time (mark directives alone on a line).
- Renderer applies §3.2 truthiness, §3.3 pipes, §3.5 indentation (track the column where a `Var`/`For`
  begins; prefix each produced newline-led line with that indent). `Value::Bool` stringifies to
  `"true"`/`"false"`.
- Partial loading is injected (callback) so the engine stays I/O-free and unit-testable. The facade
  supplies a resolver reading from the template file's directory.
- Dependency-light (hand-rolled parser; no new crates). Feature `template` on `carta-core`.

### 4.3 Rendering metadata through the target writer (the hard part)

`MetaInlines`/`MetaBlocks` must render in the **target** format before insertion (`title: Hi *there*`
→ `$title$` is `Hi <em>there</em>` for HTML, `Hi \emph{there}` for LaTeX); `MetaString` is
**target-escaped** (`a & b` → `a &amp; b` / `a \& b`); raw `-V` values are inserted **verbatim,
unescaped**. Writers expose no inline/block render entry today, so extend the `Writer` trait with
**provided methods** (default impls ⇒ zero churn for writers that accept them):

```rust
pub trait Writer {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String>;

    /// Render inline metadata in this writer's format (for template variables).
    fn render_meta_inlines(&self, inlines: &[Inline], options: &WriterOptions) -> Result<String> {
        // Plain block ⇒ no paragraph chrome; HTML/LaTeX `Plain` emit bare inlines.
        let doc = Document { blocks: vec![Block::Plain(inlines.to_vec())], ..Document::default() };
        self.write(&doc, options).map(|s| s.trim_end_matches('\n').to_string())
    }

    /// Render block metadata (e.g. an `abstract:` authored as Markdown blocks).
    fn render_meta_blocks(&self, blocks: &[Block], options: &WriterOptions) -> Result<String> {
        let doc = Document { blocks: blocks.to_vec(), ..Document::default() };
        self.write(&doc, options).map(|s| s.trim_end_matches('\n').to_string())
    }

    /// carta's own standalone template; `None` ⇒ standalone == fragment.
    fn default_template(&self) -> Option<&'static str> { None }
}
```

- The `Block::Plain` wrap renders inlines with no paragraph wrapper across writers, so the default is
  correct for most; a writer overrides only if its `Plain` diverges. The differential surface (§7.1)
  catches per-format divergence.
- `MetaString s` ⇒ `render_meta_inlines(&[Inline::Str(s)])` (target escaping).
- Additive (provided methods ⇒ no downstream breakage).

### 4.4 Context builder + orchestration (`carta` facade)

`WriterOptions` gains (behind `standalone`; struct stays `#[non_exhaustive]`):

```rust
pub standalone: bool,
pub template: Option<String>,        // template text (CLI reads the file)
pub template_dir: Option<PathBuf>,   // partial-resolution root
pub variables: Vec<(String, String)>, // -V, raw, ordered, repeatable → lists; highest precedence
```

`-M`/`--metadata-file` do **not** ride here — they are merged into `Document.meta` (decision 4). The
orchestration (extend `convert`, options-driven; public signature unchanged):

1. `document = reader.read(...)`.
2. **Merge metadata into `document.meta`** by precedence (lowest→highest): `--metadata-file` values
   (§4.6), then the reader's `document.meta`, then `-M`. (Confirmed: `-M` and document meta both
   override `--metadata-file`; `-M` overrides document meta. Merge is key-level replace; nested-map
   deep-merge is an open question, §10.)
3. `body = writer.write(&document, &fragment_options)`.
4. If not standalone (no `-s`, no `--template`): return `body` (today's behavior).
5. **Build the context** `Value::Map`:
   - each `document.meta` entry → `Value`: `MetaString`→target-escaped `Str`; `MetaInlines`→
     `render_meta_inlines`; `MetaBlocks`→`render_meta_blocks`; `MetaBool`→`Bool`; `MetaList`→`List`
     (recurse); `MetaMap`→`Map` (recurse);
   - inject `body`; inject `pagetitle` = `to_plain_text` of the title inlines (HTML `<title>` cannot
     hold markup — carta's HTML template uses `$pagetitle$` there and `$title$` in the visible header);
   - overlay `-V` variables as raw `Str`/`List` (repeated key → `List`), **highest precedence**,
     unescaped;
   - leave `toc`/`highlighting-css` unset (their plans fill them).
6. Resolve template: `--template` text if given, else `writer.default_template()` (`None` ⇒ return
   `body`). Parse, render with the context + a partial resolver rooted at `template_dir`, return
   **verbatim** (no appended newline — §4.6).

Precedence (highest first): **`-V` > `-M` > document metadata > `--metadata-file`.**

### 4.5 Default templates to author (carta's own — all 13)

- **Wrapping** (need a real template): `html`, `html4`, `latex`, `beamer`, `revealjs`, `typst`, `man`,
  `opml`, `rst`, `asciidoc`, `markdown`, `gfm`, `plain`.
- **Body-only** (`-s` == fragment): `commonmark`, `mediawiki`, `dokuwiki`, `jira` — `default_template`
  returns `None`.

Title-block shapes to reproduce *structurally* (not byte-copied): markdown/gfm → YAML metadata block
(keys **sorted**) then body; rst → over/underlined title + `:Author:`/`:Date:` fields; asciidoc →
`= Title`, authors joined by `; `, date; man → `.TH "title" "" "date" ""`; html/latex/typst/slides →
the document scaffold with metadata in head/preamble/title. Empty-title behavior is carta's choice
(its own templates) — pick a sensible default (e.g. empty `<title>` rather than a placeholder) and
snapshot it.

Embed via `include_str!` next to each writer (e.g. `crates/carta-writers/src/templates/default.html`),
returned from `default_template()`; each rides its writer's existing feature flag.

### 4.6 CLI (`crates/carta-cli/src/main.rs`)

Add to `Cli`, thread into `WriterOptions`/metadata in `convert_document`:

- `-s, --standalone` (bool).
- `--template <FILE>` (read file → `template` + `template_dir`; **implies `-s`** — confirmed).
- `-V, --variable <KEY[=VAL]>` (repeatable; **bare `-V key` ⇒ `"true"`** — confirmed). Raw template
  vars; do **not** imply `-s`.
- `-M, --metadata <KEY[=VAL]>` (repeatable; `true`/`false` ⇒ `MetaBool`, else `MetaString`). Merged
  into `document.meta`; does **not** imply `-s`.
- `--metadata-file <FILE>` (repeatable; YAML/JSON → `MetaValue` via the reused commonmark machinery,
  §4.6.1). Merged into `document.meta`.
- `-D, --print-default-template <FORMAT>` (prints **carta's** embedded default and exits; joins the
  `--list-*` early-exit family).

**Newline (critical for byte-parity).** pandoc emits the rendered template **verbatim** — it neither
adds nor strips a trailing newline (confirmed: a template with no final newline → output with none;
with one → exactly one). carta's CLI appends `\n` unconditionally at `main.rs:127`. **Fix**: in
standalone mode (the output came from a template) the CLI writes the converted string **verbatim**; it
appends exactly one `\n` only in fragment mode (preserving the `Writer` trait contract). The CLI knows
the mode from `cli.standalone || cli.template.is_some()`.

**Do not** add `--toc`/`--number-sections` (separate STATUS rows).

#### 4.6.1 `--metadata-file` parsing (reuse commonmark)

`--metadata-file` values are **Markdown-parsed** into `MetaInlines` regardless of input format
(confirmed even with `-f html`). Expose a public entry point from the commonmark reader, e.g.
`pub fn parse_metadata_yaml(content: &str, ext: Extensions) -> Result<BTreeMap<Text, MetaValue>>`,
wrapping the existing `yaml::parse` + `frontmatter::yaml_to_meta`. The facade calls it for each
`--metadata-file`. Per decision 6 this lives behind `read-commonmark`, and the `metadata-file` cargo
feature pulls that in, so the capability is never half-present. JSON metadata files parse via the
existing `serde_json` dependency into the same `MetaValue` map. If `--metadata-file` is passed in a
build without the `metadata-file` feature, return a clear runtime error (mirrors the existing
`FormatNotEnabled` pattern) rather than a clap parse error.

### 4.7 Feature taxonomy (decision 6)

- `carta-core`: `template = []` (the engine).
- `carta`: `standalone = ["carta-core/template"]` — engine, context builder, standalone wrapping,
  default templates (each rides its writer feature), and CLI `-s`/`--template`/`-V`/`-M`. Works with
  any reader/writer; **no commonmark required**.
- `carta`: `metadata-file = ["standalone", "read-commonmark"]` — adds `--metadata-file`. Declaring
  `read-commonmark` makes the broken combination unrepresentable.
- `carta`/`full` and `default` include both `standalone` and `metadata-file`.
- `carta-cli`: enable `carta/standalone` (+ `carta/metadata-file` in default builds). Gate the
  `--metadata-file` handling on the feature with a clear runtime error otherwise (§4.6.1).

## 5. Metadata/variable semantics (empirical — confirmed)

- **Precedence** (highest first): `-V` > `-M` > document YAML/title metadata > `--metadata-file`.
- `-V key=val`: raw template variable; **verbatim, unescaped**; repeated key → list; bare `-V key` ⇒
  `"true"`.
- `-M key=val`: metadata; `true`/`false` → `MetaBool` (bare interpolation renders `true`/`false`;
  falsy-when-false in `$if$`); else `MetaString` (**target-escaped** when interpolated, **not**
  Markdown-parsed).
- `--metadata-file`: YAML/JSON → typed `MetaValue`; scalars are **Markdown-parsed** (→ `MetaInlines`),
  independent of input format; lowest precedence.
- document metadata: reader-produced, gated on reader extensions (`commonmark` alone does **not** parse
  a YAML block; `markdown` / `commonmark+yaml_metadata_block` do). carta already gates this — no change.
- `MetaInlines`/`MetaBlocks` render through the **target** writer (§4.3).
- **Map ordering is sorted** in both pandoc and carta (`$for(m/pairs)$` over `{z,a,m}` → `a,m,z`).
  carta's `BTreeMap` matches pandoc's sorted maps — no divergence; do **not** try to preserve insertion
  order.

## 6. Implementation steps

Each step ends green before the next. Steps 1–4 are pure `carta-core`, no oracle. Land as separate
Conventional Commits / PRs (`feat(template): …`, `feat(carta): standalone wrapping`, `feat(carta-cli):
…`, `test(conformance): templates surface`, `docs: …`).

1. **Engine parser + node AST** (`carta-core::template`, feature `template`), incl. §3.5 whitespace.
   Layer-0 unit tests: `$$`, comments, vars, nested fields, `if/elseif/else`, `for/sep`, and **every
   §3.5 case** (directive-alone-on-line consumption; multi-line variable indentation; `for`-body
   indentation). *Verify*: `cargo nextest run -p carta-core template::`.
2. **Renderer + `Value`** (§3.2 truthiness; `Bool`→`true`/`false`; direct/list/map interpolation).
   Unit tests.
3. **Pipes** (§3.3 must-have; nice-to-have if cheap). Unit tests per pipe + chaining.
4. **Partials** (§3.4) via the resolver callback; map + separator forms; `$it$`. Unit tests with an
   in-memory resolver.
5. **Context builder + `Writer` provided methods** (§4.3) + `WriterOptions` fields + metadata merge
   into `Document.meta` with precedence (§4.4, §5). *Verify*: a `carta`-level test rendering a small
   inline template against a `Document` with mixed `MetaValue`s to **html and latex**, asserting
   target-specific rendering, `pagetitle` plain-text, and precedence.
6. **Writer integration**: standalone branch in the facade (§4.4); `default_template()` returns `None`
   for all writers initially. *Verify*: `cargo nextest run --workspace` still green (additive trait
   change broke nothing).
7. **Default templates** (§4.5), one writer at a time, **html and latex first**, then the rest until
   all 13 land (decision 1). Add Layer-1 `insta` snapshots of carta's **own** standalone output over a
   small `corpus/ast` fixture set. *Verify*: `cargo insta review` then `cargo nextest run -p carta`.
8. **CLI** (§4.6): flags, threading, the **verbatim-newline** fix, `-D`, and the `metadata-file`
   feature gate. *Verify against the oracle* (observation only, nothing copied) and encode in the
   `commands` surface: `--template` implies `-s`; bare `-V key` ⇒ `true`; `-M k=true` ⇒ bool; `-s`
   output has the right trailing-newline byte count.
9. **Conformance**: new **`templates`** differential surface (§7.1) wired into `run.sh`'s `SURFACES`;
   plus the **structural** default-template checks (§7.2). *Verify*: `tools/conformance-suite/run.sh
   templates` → all pass, exit 0.
10. **Feature-matrix check**: build `carta` with `--no-default-features` + `standalone` + one
    writer/reader (no commonmark) and confirm it compiles and `-s` works; build with `metadata-file`
    and confirm commonmark is pulled in. *Verify*: the two `cargo build -p carta --no-default-features
    --features …` lines in §7.3.
11. **Docs**: flip `docs/STATUS.md` Standalone + Metadata/variables rows to `✅`; note the wired-but-
    inert `$toc$`/number-sections/highlighting slots.

## 7. Oracle verification & CI gates

Two gates, matching the §4.1 boundary.

### 7.1 Engine + context — byte-exact differential (new `templates` surface)

`tools/conformance-suite/surfaces/templates.sh`: for each **carta-authored neutral template** T (a
small committed set under `corpus/templates/`, exercising every §3 construct + pipes + partials +
whitespace edge cases) × each metadata input M (committed `.md`/YAML) × each target in a set with
diverse escaping/inline rendering (`html`, `latex`, `plain`, `rst`, `markdown`, `gfm`, `asciidoc`,
`mediawiki`):

```
carta_out  = carta  -f markdown -t <fmt> --template=T  < M
pandoc_out = pandoc -f markdown -t <fmt> --template=T  < M
diff byte-for-byte
```

Include M/flag combinations exercising precedence (`-V`/`-M`/`--metadata-file` together) and
`pairs`/`for` ordering. Because T is ours, this is fully clean-room and pins the engine, pipes,
whitespace rules, metadata-through-writer rendering, and precedence. Emits `RESULT templates <group>
pass/fail/err/skip`; non-zero exit on any fail/err; **CI-gated**. Skip+count `(template, fmt)` pairs
using constructs/targets carta does not yet support, so coverage is honest.

### 7.2 Default templates — own snapshots + structural oracle checks (NOT byte-diffed)

- **Layer-1 `insta` snapshots** of `carta -s` (default template) over a small `corpus/ast` set, one per
  wrapping format; reviewed with `cargo insta review`. carta's own output — **never** an oracle diff.
- **Structural oracle assertions** (new check in the conformance suite; clean-room — compares
  *structure*, commits nothing): for representative inputs with title/author/date,
  - **HTML family**: parse **both** `carta -s` and `pandoc -s` with carta's **own HTML reader** and
    assert the **body block-AST is equal** and the title text is present in `<title>`/the title header.
    This proves carta's scaffold carries content and metadata equivalently without copying the CSS.
  - **LaTeX / others** (no reader): targeted assertions — `\title{…}`/`\author{…}` (or the format's
    title-block) present with the expected text, and the rendered body substring present in both.
  - LaTeX *compiles*: optional, manual (needs a TeX toolchain) — **not** a CI gate; note it as a
    smoke-check in the Executed note if run.

### 7.3 Other gates

- Layer-0 unit tests (Steps 1–4) carry the whitespace risk (`cargo nextest`).
- Coverage: `carta-core::template` must clear the 90% line floor
  (`cargo llvm-cov --workspace --summary-only --fail-under-lines 90`).
- Feature matrix (Step 10):
  `cargo build -p carta --no-default-features --features standalone,read-html,write-html` (compiles, no
  commonmark) and
  `cargo build -p carta --no-default-features --features metadata-file,write-html` (commonmark pulled in
  transitively).
- `commands` surface gains the CLI-implication + newline-byte checks from Step 8.

## 8. Clean-room guardrails (this feature is provenance-sensitive)

- **Do not** copy *or reword* pandoc's default templates/CSS/preamble — not via
  `--print-default-template`, not by transcribing observed `-s` output. Author carta's own from first
  principles (decision 2).
- **Do not** commit any pandoc output as golden values. The `templates` surface and the structural
  checks generate oracle output **live**; snapshots are carta's own output.
- Neutral templates (§7.1) are carta-authored inputs (own them like the corpus).
- No upstream provenance in any product source or template file (repo source-hygiene rule).
  "template"/"standalone"/"variable" are generic vocabulary and fine; do not write "matches pandoc",
  "the reference template", etc. No sanctioned external-format literal is needed here.

## 9. STOP conditions

- Any attempt to make `carta -s` byte-match `pandoc -s` for a **default** template, or to source
  carta's CSS/preamble by rewording pandoc's — stop (the gate is §7.2, not a byte diff).
- The engine differential surface (§7.1) cannot reach byte-parity on a §3 construct: document it as a
  known divergence and skip+count it — **do not** mutate default templates to mask an engine bug.
- The `Writer`-trait extension forces changes outside `carta-writers`/`carta-core`/`carta` — stop and
  report.
- A feature combination produces a build where `--metadata-file` is accepted but non-functional (the
  taxonomy in §4.7 should make this impossible) — stop and fix the feature graph.
- Drift check (top of file) shows material change to an integration point.

## 10. Open questions (RESOLVED during execution — see Executed for probe detail)

1. **`left`/`right`/`center` pipes — RESOLVED: implement.** Syntax `$x/left WIDTH ["LBORDER" "RBORDER"]$`.
   With a width and optional border strings they pad to the column: `left` pads on the right, `right`
   pads on the left, `center` splits the pad. **Known gap to watch**: without border strings, the
   block-layout pass strips the trailing pad (`[$s/left 20$]` → `[Hello World]`), so a bare
   `left`/`center` with no borders is a no-op in practice; if any divergence surfaces there, log it as a
   known gap rather than mutating templates.
2. **Slide templates — RESOLVED: faithful-but-minimal.** Author minimal `revealjs`/`beamer` scaffolds
   covering the common variable slots (`title`/`author`/`date`/`body`/`header-includes`); list any
   deferred slide-only variables in Executed.
3. **Nested-map merge — RESOLVED: WHOLE-KEY REPLACE, not deep-merge.** A higher-precedence source's
   value for a top-level key entirely replaces the lower source's value for that key; nested maps are
   **not** deep-merged. Among multiple `--metadata-file`s, **later overrides earlier** at key level
   (union of distinct keys; shared keys take the later file's value). Oracle-confirmed: docmeta
   `m:{a,keep}` over metadata-file `m:{a,add}` → `add` is gone (the whole `m` was replaced).
4. **Bare `$abstract$`/`MetaBlocks` — RESOLVED: renders block-level through the target writer.** The
   `render_meta_blocks` wrap is correct: html → `<p>…</p>` paragraphs; latex → paragraphs separated by a
   blank line. Matches the oracle on the differential surface.

Additional semantics confirmed during execution (all oracle-pinned):
- `$elseif(y)$` chains work as documented.
- `$for$` over a list-of-maps with `$it.field$`, and **nested** `$for$` rebinding `$it$` to the inner
  loop, both work.
- `pagetitle` = plain text of the title inlines (`Hi *there* and \`code\`` → `Hi there and code`); the
  visible `$title$` renders the inlines (a soft break becomes a newline).
- `$body$` carries **no** trailing newline (it is the writer's verbatim fragment output).
- Comment `$-- …$` deletes to end of line; if the resulting physical line is wholly empty (column 0) the
  newline collapses, otherwise the leading whitespace + newline are kept.

_(Resolved during planning, no longer open: `--template` implies `-s` (yes); bare `-V key` ⇒ `true`;
`Document: Default` (yes); `pairs`/map ordering is sorted; bare `MetaBool` renders `true`/`false`;
pandoc emits templates verbatim with no added newline.)_

## Executed (fill in during/after execution)

- _Commits, `RESULT templates …`, structural-check results, known divergences, deferred
  pipes/variables, and the STATUS.md edits go here._
