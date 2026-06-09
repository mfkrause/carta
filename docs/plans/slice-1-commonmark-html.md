# Slice 1 — CommonMark → HTML

Status: **landed**. Owner: slice-1.
Read `../PORTING.md` §3–9 and `../../AGENTS.md` first, then the slice-0 plan
(`slice-0-ast-json-contract.md`) for the conventions this slice builds on. This document is
self-contained: it should be possible to implement the slice from it alone.

**Outcome.** `carta -f commonmark -t html` is byte-identical to the pinned binary
(`--syntax-highlighting=none --mathjax`) on all 652 vendored CommonMark spec examples, and the
reader matches the oracle JSON AST on all 652. The HTML writer additionally matches the oracle
across the full document model (tables, definition lists, figures, footnotes, citations, math, raw
passthrough, spans) — see the writer-parity suite. Product-crate line coverage is ~96%.

**Known scoped-out divergence.** GFM task lists (`- [ ] item`) are not special-cased: the writer
emits the literal `☐`/`☒` rather than pandoc's `<input type="checkbox">`. Task lists are a GFM
extension with no CommonMark syntax, unreachable from the CommonMark reader; the heuristic belongs
with GFM reader support in a later slice.

## 0. Goal & done criteria

Stand up the first end-to-end conversion path — `carta -f commonmark -t html` — by landing a
hand-rolled **strict-CommonMark reader**, a **full-coverage HTML writer**, a uniform `Reader`/
`Writer` trait layer, and the reader/writer/end-to-end differential surfaces. This proves the
`readers → AST → writers` pipeline and the differential harness on a real format pair.

**Definition of done:**

1. `carta -f commonmark -t html`, `carta -f commonmark -t json`, `carta -f json -t html`, and the
   existing `carta -f json -t json` all run end-to-end (stdin/file in, stdout/`-o` out).
2. **Reader gate (hard):** for every example in the vendored CommonMark spec suite,
   `Value(carta -f commonmark -t json) == Value(pandoc -f commonmark -t json)`. 100% parity is the
   bar — no silently skipped examples. Any example not yet matching is a tracked `todo!`/IOU with the
   case recorded, and the slice is **not done** while any remain (see §11 risk).
3. **Writer gate (hard, byte-exact):** for every `.native` file in the fetched corpus (the slice-0
   set), the document minted to JSON, parsed, and re-rendered by our HTML writer equals
   `pandoc -f json -t html --syntax-highlighting=none --mathjax <doc>` byte-for-byte.
4. **End-to-end gate:** for the spec-suite inputs and hand-authored fixtures,
   `carta -f commonmark -t html` equals
   `pandoc -f commonmark -t html --syntax-highlighting=none --mathjax` byte-for-byte.
5. The pandoc command-test grammar parser (`command_test::parse`) is implemented; the runnable
   subset (input ∈ {commonmark, json}, output ∈ {json, html, html5}, no unsupported flags) passes,
   and the skipped/unrunnable count is reported (not hidden).
6. No `todo!` reachable on a shipped conversion path except the two sanctioned classes: (a) formats
   outside {commonmark, json}→{json, html}; (b) recorded spec-suite parity gaps under the hard gate
   in (2), which block "done".
7. Clippy clean under the workspace lints (no `unwrap`/`expect`/`panic`/indexing in shipped code);
   `cargo fmt --check` clean.

**Explicitly out of scope (deferred, with their PORTING home):**

- Syntax highlighting (skylighting) — Tier C; neutralized via `--syntax-highlighting=none`.
- Math rendering via texmath — Tier C; neutralized via `--mathjax` (TeX passthrough).
- Templates / `--standalone` / `-s` — Tier C.
- CommonMark extensions: `gfm`, `commonmark_x` (tables, footnotes-from-source, attributes, math
  syntax, definition lists, task lists, strikethrough, autolink ext) — later slices.
- The `native` reader/writer; any other reader/writer.
- `--wrap`, `--columns`, and other writer-shape options (default wrapping only).

## 1. Decisions locked (from the slice-1 grilling)

| Decision | Choice |
| --- | --- |
| CommonMark reader strategy | **Hand-roll** from the CommonMark spec (no third-party parser dependency) |
| Slice sequencing | **Writer first** (de-risk against the whole slice-0 corpus), then the reader; both ship in slice 1 |
| CommonMark variant | **Strict `commonmark`** (no extensions) |
| HTML target | **html5 fragment** (no standalone, no templates) |
| Writer node coverage | **Full AST coverage** — every `Block`/`Inline` renders |
| Code-highlighting + Math subsystems | **Neutralize via flags** (`--syntax-highlighting=none --mathjax`); defer skylighting + texmath to Tier C |
| Reader/Writer API | **Traits now** (`Reader`/`Writer` in `carta-core`), `&str` input, empty `#[non_exhaustive]` options structs |
| JSON codec home | **Stays in `carta-ast`**; thin `JsonReader`/`JsonWriter` adapters in the new crates |
| Conformance corpus | **Vendor CommonMark spec 0.31.2** under the testkit, **inputs only** (diff vs pandoc, never vs the spec's reference HTML) |
| Reader done-bar | **100% spec-suite parity, hard gate** (gaps tracked as IOUs that block done) |
| Writer differential gate | **Byte-exact** vs pandoc (with the two neutralizing flags) |
| Command-test parser | **Implement now**; run the format-supported subset, report the rest |

## 2. Architecture & crate layout

Two new crates complete the `readers → AST → writers` shape from PORTING §3.

```
carta-core      + Reader/Writer traits, ReaderOptions/WriterOptions, extended Error
                   (gains a dependency on carta-ast — no cycle: ast depends on neither)
carta-readers   NEW. modules: json (adapter), commonmark (hand-rolled)
carta-writers   NEW. modules: json (adapter), html (full-coverage writer)
carta-cli       dispatches input/output format enums over the trait impls
carta-testkit   + differential.rs (reader/writer/e2e surfaces, in-process),
                 + commonmark_spec.rs (vendored spec parser),
                 + command_test::parse implemented; gains dev-deps on readers/writers
```

Dependency direction stays acyclic: `ast` depends on nothing internal; `core` → `ast`;
`readers`/`writers` → `ast` + `core`; `cli`/`testkit` → all.

### 2.1 The trait contract (`carta-core`)

```rust
use carta_ast::Document;

pub trait Reader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document>;
}

pub trait Writer {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<String>;
}

#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct ReaderOptions {}

#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct WriterOptions {}
```

- `&str` in / `String` out: the CLI decodes input bytes to UTF-8 once and passes `&str`; writers
  return the document body **without** a trailing newline (see §4.1). The trait surface is stable as
  real options arrive (they extend the `#[non_exhaustive]` structs, not the signatures).
- Implementors are unit structs: `CommonmarkReader`, `JsonReader`, `HtmlWriter`, `JsonWriter`.
- `Result` is `carta_core::Result`.

### 2.2 Error type extension (`carta-core`)

Add to the existing enum (slice-0 §4):

```rust
#[error("input is not valid UTF-8: {0}")]
InvalidUtf8(#[from] std::string::FromUtf8Error),
```

Rationale: the CLI now decodes arbitrary input bytes to text before handing them to a reader. The
strict-CommonMark reader is **total** (every UTF-8 string is valid CommonMark — it never errors);
the full HTML writer is **total** (every node renders). So no reader/writer-specific error variants
are needed in slice 1; `Json` still covers `JsonReader` failures. Revisit when a fallible
reader/writer lands.

## 3. The wire targets (derived from the pinned binary — pandoc 3.10, never from memory)

All shapes below were observed from `.oracle/bin/pandoc` with
`--syntax-highlighting=none --mathjax`. Re-derive with the binary whenever unclear; never read
pandoc source. The byte-exact gate (§0.3–0.4) is the final authority — these are the documented
starting point, not a substitute for differential verification node-by-node.

### 3.1 HTML writer output shape

- **Fragment only.** No wrapper. Output is the concatenation of top-level blocks **joined by `\n`**,
  with no trailing newline from the writer (the CLI/oracle comparison appends exactly one `\n`, §4.1).
- **No indentation.** Nested blocks are newline-separated, never indented:
  `<blockquote>\n<p>q1</p>\n<p>q2</p>\n</blockquote>`.
- **Text escaping** (in text nodes): `&`→`&amp;`, `<`→`&lt;`, `>`→`&gt;`. `"` and `'` are **not**
  escaped in text. (`A & B < C > "q"` → `A &amp; B &lt; C &gt; "q"`.)
- **Attribute-value escaping**: as text plus `"`→`&quot;`.
- **Entity decoding is the reader's job**, not the writer's: by the time text reaches the writer it
  is decoded Unicode (`©`), and the writer re-escapes only the three text-significant characters.

Block renderings (starting spec; verify each byte-exact):

```
Para            <p>INLINES</p>
Plain           INLINES                       (no wrapper)
Header n        <hN ATTRS>INLINES</hN>         (attr order: class, keyvals, id — see §3.2)
CodeBlock       <pre class="LANG"><code>ESCAPED</code></pre>   (LANG = first class; omit class attr
                  when there are none; highlighting is OFF by gate flag)
BlockQuote      <blockquote>\nBLOCKS\n</blockquote>
BulletList      <ul>\n<li>ITEM</li>\n…\n</ul>  (tight vs loose changes <li> inner shape — derive)
OrderedList     <ol start="S" type="T">…</ol>  (omit start when 1; type per ListNumberStyle:
                  Decimal→none, LowerAlpha→a, UpperAlpha→A, LowerRoman→i, UpperRoman→I; verify)
DefinitionList  <dl>\n<dt>TERM</dt>\n<dd>\nDEFS\n</dd>\n</dl>
HorizontalRule  <hr />
Div             <div ATTRS>\nBLOCKS\n</div>
Figure          <figure ATTRS>\nBLOCKS\n<figcaption>CAPTION</figcaption>\n</figure>
                  (figcaption omitted when caption empty — see command/figures-html.md)
RawBlock        verbatim payload when format=="html"; dropped otherwise (verify drop behavior)
Table           <table>…</table>  (verbose; derive the full <colgroup>/<thead>/<tbody>/<tfoot>
                  shape from the oracle — this is the largest single node)
```

Inline renderings (starting spec; verify each byte-exact):

```
Str             escaped text                  (tokenization is the reader's concern)
Space           a single space
SoftBreak       a single "\n"                 (default no-wrap)
LineBreak       <br />\n
Emph            <em>…</em>     Strong <strong>…</strong>    Strikeout <del>…</del>
Superscript     <sup>…</sup>   Subscript <sub>…</sub>       Underline <u>…</u>
SmallCaps       <span class="smallcaps">…</span>
Quoted          DoubleQuote → “…”   SingleQuote → ‘…’       (curly quotes, literal)
Code            <code ATTRS>ESCAPED</code>    (no highlighting on inline code)
Math InlineMath   <span class="math inline">\(TEX\)</span>     (--mathjax passthrough)
Math DisplayMath  <span class="math display">\[TEX\]</span>    (verify delimiters)
RawInline       verbatim payload when format=="html"; dropped otherwise
Link            <a href="URL"[ title="T"]>INLINES</a>   (title attr omitted when empty)
Image           <img src="URL"[ title="T"] alt="ALTTEXT" />  (alt = rendered-to-plain-text of the
                  inlines; title before alt; self-closing " />")
Span            <span ATTRS>INLINES</span>
Cite            renders its display inlines (verify wrapper span/attrs from oracle)
Note            footnote reference + accumulation — see §3.3
```

### 3.2 Attribute rendering (`Attr` → HTML), element-specific ordering

`Attr = (id, classes, [(key, val)])`. Emit only non-empty parts:

- `id` → `id="ID"` ; `classes` → `class="C1 C2"` (space-joined) ; each `(key,val)` → `key="val"`,
  **except** keys that are not valid HTML attribute names are prefixed `data-` (e.g. `k` → `data-k`;
  `title` stays `title`). Derive the exact "valid HTML attribute name" predicate from the oracle.
- **Ordering is element-specific** (a confirmed pandoc quirk):
  - Most elements (`span`, `div`, `code`, …): **id, class, keyvals**.
    `<span id="myid" class="c1 c2" data-x="1" title="t">`
  - **Headers**: **class, keyvals, id** — id goes last.
    `<h1 class="hc" data-k="v" id="hid">`
  - Verify other elements (`a`, `figure`, `table`, `pre`) empirically; the byte-exact gate forces it.
  Model this as a small `enum AttrContext` (or per-call ordering) rather than scattering `if`s.

### 3.3 Footnotes (`Inline::Note`) — stateful, document-end section

A `Note` does not render inline. Rendering carries mutable state: a monotonic counter and a list of
collected note bodies. Each `Note` emits an inline reference and pushes its body:

```
ref:   <a href="#fnN" class="footnote-ref" id="fnrefN" role="doc-noteref"><sup>N</sup></a>
```

After all blocks are rendered, if any notes were collected, append the section:

```
<section id="footnotes" class="footnotes footnotes-end-of-document" role="doc-endnotes">
<hr />
<ol>
<li id="fnN"><BODY><a href="#fnrefN" class="footnote-back" role="doc-backlink">↩︎</a></li>
…
</ol>
</section>
```

Derive the exact backlink placement (inside the body's last block) and the `↩︎` glyph from the
oracle. Keep this in a `Writer` state struct; do not thread booleans through every render fn.

### 3.4 HTML writer internal design

Avoid spaghetti: a single `struct HtmlState { out: String, footnotes: Vec<String>, note_count: u32 }`
with methods `block`, `inline`, `inlines`, `attr(ctx, &Attr)`, `escape_text`, `escape_attr`. Block
joining via a helper that interleaves `\n`. `HtmlWriter::write` builds the state, renders blocks,
appends the footnote section, returns `out`. No panics, no indexing — iterate, use `.get()`.

## 4. CLI dispatch (`carta-cli`)

### 4.1 Pipeline & newline ownership

`bytes → String::from_utf8 (→ InvalidUtf8) → reader.read(&str) → writer.write → stdout + "\n"`.

Writers return the document body with **no** trailing newline; the CLI appends exactly one `\n`
(this already matches slice-0 JSON behavior and pandoc's single-trailing-`\n` for html and json).
The differential harness mirrors this: it appends `\n` to writer output before comparing to pandoc's
raw stdout. Verify the empty-document case against the oracle.

### 4.2 Format dispatch

```rust
enum InputFormat { Json, Commonmark }
enum OutputFormat { Json, Html }
```

Parse `--from`/`--to` strings into these. `html5` is accepted as an alias for `html`. Any other
value → `Error::UnsupportedFormat` (the sanctioned non-panic for unsupported formats). Map each enum
to its trait impl (a `match` returning `&dyn Reader`/`&dyn Writer`, or direct calls). Keep `--from`
and `--to` required, as in slice 0.

## 5. HTML writer (`carta-writers::html`) — build order

Writer-first. Build node-by-node and differential-verify against the corpus (§6.2) as you go:

1. Skeleton + `Para`/`Plain`/`Str`/`Space`/`SoftBreak` + text escaping → run the corpus subset.
2. Inline formatting (`Emph`…`Underline`, `Quoted`, `Code`, `LineBreak`).
3. `Attr` rendering + `Span`/`Div`/`Header` (nail the element-specific ordering, §3.2).
4. `Link`/`Image` (+ image alt-text flattening), `RawInline`/`RawBlock`.
5. Block containers: `BlockQuote`, `BulletList`, `OrderedList` (tight/loose), `DefinitionList`,
   `HorizontalRule`, `CodeBlock` (highlighting off), `Figure`.
6. `Math` (passthrough, §3.1), `Cite`, `Note` (§3.3).
7. `Table` — the big one; derive the full shape from the oracle.

After each step the writer differential (§6.2) should monotonically reduce failures. The order means
the easy, high-frequency nodes are correct before the rare, verbose ones.

## 6. Differential harness (`carta-testkit`)

Three programmatic surfaces run **in-process** (call the trait impls directly — simpler and faster
than shelling out, and sidesteps cross-crate `CARGO_BIN_EXE`). The command-test runner (§6.4) is the
exception: it needs the real binary, so that integration test lives in `carta-cli/tests/` where
`env!("CARGO_BIN_EXE_carta")` is available, using testkit's parser + comparison helpers.

Reuse slice-0 infrastructure: `mint_golden` (cached `.native`→JSON), `oracle_dir`/`pandoc_bin`,
`collect_files_with_extension`, and the `Value` first-difference reporter (`roundtrip::first_difference`).

### 6.1 Reader surface — `carta -f commonmark -t json` vs pandoc

For each spec example input (§7) and hand-authored input:
`Value(to_json(CommonmarkReader.read(input))) == Value(pandoc -f commonmark -t json input)`.
Use Value-equality (order/float-agnostic, like slice 0). Cache pandoc's JSON keyed on
(version + input bytes + args), reusing the slice-0 cache pattern. Report the first differing JSON
pointer and the offending example number/section.

### 6.2 Writer surface — AST → HTML vs `pandoc -f json -t html`

For each `.native` corpus file: `mint_golden` → `from_json` → `HtmlWriter.write` → append `\n` →
compare **byte-exact** to `pandoc -f json -t html --syntax-highlighting=none --mathjax <doc>` (cache
the pandoc html output keyed on version + minted-json bytes + args). On mismatch, report the file and
the first differing byte offset with a short context window.

### 6.3 End-to-end surface — `carta -f commonmark -t html` vs pandoc

For spec inputs + fixtures: `HtmlWriter.write(CommonmarkReader.read(input))` + `\n` compared
byte-exact to `pandoc -f commonmark -t html --syntax-highlighting=none --mathjax`. This is implied by
6.1 ∧ 6.2 but catches integration gaps directly.

### 6.4 Command-test runner — implement `command_test::parse`

Grammar (confirmed from `test/command/*.md`; the leading `pandoc` word and the args after it are the
spec): each fenced ```` ``` ```` block is one test:

```
% pandoc <ARGS>
<INPUT…>
^D                     (literal caret-D on its own line; separates input from expected)
<EXPECTED OUTPUT…>
```

`parse(source) -> Vec<CommandTest>` extracts every such block (prose between blocks is ignored),
splitting `args` (drop the leading `pandoc`), `input`, `expected`. The runner:

- **Filters to runnable tests**: input format ∈ {commonmark, json}, output format ∈ {json, html,
  html5}, and no unsupported flags (anything beyond `-f`/`--from`, `-t`/`--to`,
  `--syntax-highlighting=none`, `--mathjax`). Inject `--syntax-highlighting=none --mathjax` for html
  output so expectations match our neutralized target — **skip** any html test whose expected output
  contains highlighting/texmath markup (those were generated without our flags); count them.
- Runs the carta binary with the test's args, feeds `input` on stdin, compares stdout byte-exact to
  `expected`.
- **Reports** counts: passed / failed / skipped-unrunnable, with reasons. Most `html-writer`/`figures`
  command tests feed `-f native` (no native reader in slice 1) and are therefore skipped — this is
  expected and recorded, not a failure (watch-item §11).

### 6.5 Test gating

Like slice 0, the corpus-backed tests **hard-require** `.oracle/`; absence fails with provisioning
instructions, never silently skips. The spec-suite tests require the vendored `spec.txt` (committed,
§7) and the oracle (for pandoc's side). Offline fixtures (§8) run with no oracle.

## 7. Vendored CommonMark spec suite (inputs only)

- Vendor `spec.txt` at **CommonMark 0.31.2** under `crates/carta-testkit/vendor/commonmark/spec.txt`
  with a sibling `LICENSE` (CC-BY-SA-4.0) and a short `ATTRIBUTION.md` (source URL + version +
  license). This is the CommonMark project's own file — unrelated to pandoc, **no clean-room concern**
  — and PORTING §5 explicitly permits vendoring the conformance suite with attribution.
- Add `tools/fetch-commonmark-spec.sh`: fetch `spec.txt` at the pinned tag into the vendor path,
  idempotently. (Committed, unlike the gitignored pandoc corpus.)
- **Parser** (`commonmark_spec.rs`): the spec embeds examples as fenced blocks delimited by a run of
  32 backticks with the word `example`, input and expected HTML separated by a line containing only
  `.`, closed by 32 backticks. Markdown `#` headings between examples name the current section.
  Parse into `Vec<SpecExample { number, section, markdown, html }>`. **Verify the exact fence/marker
  format against the actual file** before finalizing. We use `markdown` (the input) only; `html` is
  the spec's reference (cmark) output and is **never** compared against — pandoc is the oracle.

## 8. Committed offline fixtures

Reuse the slice-0 pattern: small hand-authored cases that run without the oracle.

- **Writer fixtures**: `crates/carta-testkit/fixtures/html/` — pairs of `(input.json, expected.html)`
  authored to match our neutralized target, covering every block/inline node (mirrors the §3 table),
  including the footnote section, a table, a figure with/without caption, and the header attr-order
  quirk. Offline test: `HtmlWriter.write(from_json(input)) + "\n" == expected`.
- **Reader fixtures**: `crates/carta-testkit/fixtures/commonmark/` — `(input.md, expected.json)`
  pairs for representative constructs. Offline test: `Value(to_json(read(input))) == Value(expected)`.

These give readable regression cases and keep a meaningful slice-1 signal green for agents/CI without
the oracle.

## 9. CommonMark reader (`carta-readers::commonmark`) — architecture

Hand-rolled, two-phase, following the CommonMark spec's recommended structure. The spec
(`vendor/commonmark/spec.txt`, public CC-BY-SA) is the **specification** source; the pinned pandoc
binary is the **differential oracle**. Build to the 100% spec-suite hard gate (§0.2).

### 9.1 Phase 1 — block structure

Line-oriented scan maintaining a stack of open blocks; produces the block tree with leaf inline text
deferred as raw strings, and collects link reference definitions into a map. Handle:

- Container blocks: block quotes (`>`), list items (bullet `-+*`, ordered `N.`/`N)`), with tight/loose
  determination and lazy continuation.
- Leaf blocks: ATX headings (`#`), setext headings (`=`/`-`), thematic breaks, fenced code (`` ``` ``/
  `~~~` with info string), indented code, HTML blocks (all 7 start conditions), paragraphs, blank
  lines, link reference definitions.
- Tabs expand to spaces per spec (tab stops of 4) for indentation purposes.

### 9.2 Phase 2 — inline parsing

Walk each leaf block's deferred text producing `Vec<Inline>`, reproducing **pandoc's tokenization**
(the highest-risk parity area — verify relentlessly via §6.1):

- Text runs split into `Str` tokens with `Space`/`SoftBreak` between (match pandoc's exact splitting).
- Code spans (`` ` `` runs), emphasis/strong via the spec's delimiter-stack algorithm, links/images
  with reference resolution (inline + reference + collapsed + shortcut), autolinks (`<uri>` /
  `<email>`), raw inline HTML (emitted as `RawInline "html"`, split at tag boundaries as pandoc does),
  hard breaks (two trailing spaces / backslash-newline → `LineBreak`), soft breaks → `SoftBreak`.
- Backslash escapes; entity & numeric character references **decoded to their Unicode text** in `Str`
  (e.g. `&copy;` → `©`, `&amp;` → `&`).

### 9.3 Totality

The reader returns `Ok(Document)` for every input — strict CommonMark has no parse errors. The
document's `api_version` is `CURRENT_API_VERSION` (freshly constructed). No panics: the panic
lints apply; this reader is also a future `cargo-fuzz` target (PORTING §8).

## 10. Work breakdown (suggested commit sequence, Conventional Commits)

Each commit builds, is clippy/fmt clean, and keeps `todo!`s only at sanctioned sites (§0.6).

1. `feat(core): reader/writer traits, options, and utf-8 error` — §2.1–2.2.
2. `feat(writers): scaffold crate with json writer adapter` — crate + `JsonWriter` delegating to
   `carta_ast::to_json`; wire nothing else yet.
3. `feat(readers): scaffold crate with json reader adapter` — crate + `JsonReader`.
4. `refactor(cli): dispatch via reader/writer traits` — replace the slice-0 inline json path with
   trait dispatch + format enums (still json→json only); behavior unchanged, gate stays green.
5. `feat(writers): html writer — text and inline formatting` — §5 steps 1–4.
6. `feat(writers): html writer — block containers, code, figure` — §5 step 5.
7. `feat(writers): html writer — math passthrough, cite, footnotes, table` — §5 steps 6–7.
8. `test(testkit): writer differential surface against the corpus` — §6.2 + §8 writer fixtures.
   (After this, `carta -f json -t html` is done and corpus-green.)
9. `feat(readers): commonmark block structure` — §9.1.
10. `feat(readers): commonmark inline parsing` — §9.2.
11. `build(testkit): vendor commonmark spec suite + fetch tool` — §7.
12. `test(testkit): reader + end-to-end differential surfaces` — §6.1, §6.3 + §8 reader fixtures.
13. `feat(testkit): command-test grammar parser + runner` — §6.4 (runner test in `carta-cli/tests/`).
14. `feat(cli): enable commonmark→html / commonmark→json paths` — flip the format enums on; the
    end-to-end gate is the acceptance check.
15. `docs: mark slice 1 landed; update AGENTS build/test section` — record new crates + commands.

Sequence rationale: traits + writer (mechanical, corpus-verifiable) land first and prove the harness;
the reader (the hard, hard-gated piece) lands against an already-trusted writer and oracle.

## 11. Risks / watch-items

- **The reader's 100% spec-suite hard gate is the dominant schedule risk.** A long tail of pandoc
  tokenization quirks (raw-HTML token boundaries, entity edge cases, tight/loose list nuances,
  emphasis-run corner cases) can each block "done". Mitigation: build phase-2 tokenization against
  §6.1 from the first construct; record each unmatched example as a numbered IOU; do not relax the
  gate — fix the root cause (PORTING §7 conformance-loop ethos).
- **Element-specific HTML attribute ordering** (§3.2) is easy to get subtly wrong; the byte-exact gate
  catches it but only once that node appears in the corpus — author a fixture per element to force it.
- **Command-test yield is low in slice 1**: most html-writer/figures command tests feed `-f native`,
  which we can't run yet, so they're skipped-and-counted. The parser is still built (unlocks future
  slices); don't mistake the large skip count for a problem.
- **Neutralizing flags change the meaning of "byte-exact vs pandoc"**: the gate is vs pandoc *with*
  `--syntax-highlighting=none --mathjax`. Any committed expected-html fixture must be generated with
  those same flags, or it will diverge spuriously.
- **Trailing-newline / empty-document edge cases**: verify the empty doc and single-block doc against
  the oracle; the `\n` ownership rule (§4.1) must hold for both formats.
- **Image alt text** is the inlines flattened to plain text — confirm pandoc's flattening (drops
  formatting, keeps text) against the oracle rather than assuming.
