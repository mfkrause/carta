# Plan 006: CommonMark reader — the low-complexity extension set (strikeout, sub/superscript, hard_line_breaks, task_lists, raw_html)

> **Executor instructions**: Follow this plan step by step. Run every verification command and
> confirm the expected result before moving on. If anything under "STOP conditions" occurs, stop and
> report — do not improvise. When done, update this plan's status row in `plans/README.md`.
>
> **Drift check (run first)**:
> `git diff --stat e730995..HEAD -- crates/carta-readers/src/commonmark/inline.rs crates/carta-readers/src/commonmark/mod.rs crates/carta-core/src/extensions.rs crates/carta/src/lib.rs`
> If any of these changed since this plan was written, re-verify the "Current state" excerpts against
> the live code before proceeding; on a material mismatch, treat it as a STOP condition.

## Status

- **Status**: DONE (2026-06-10). Reader honors all five extensions; `reader ext` conformance group
  green (9/9) and CI-gated; two task_lists block-phase edge cases documented as known divergences (§3.3).
- **Priority**: P1
- **Effort**: M (one reader, contained; the `~`/`^` resolver is the only subtle part)
- **Risk**: MED (resolved — `~`/`^` reuse the emphasis resolver with simplified flanking; output parity is the bar)
- **Depends on**: nothing. Note: plan 003 (linear delimiter stack) is still TODO and rewrites
  `process_emphasis`; this plan deliberately builds on the **current** quadratic resolver and leaves
  a maintenance note so 003 can absorb the new delimiter kinds.
- **Category**: feature
- **Planned at**: commit `e730995`, 2026-06-10. Semantics in §3 were derived empirically from the
  pinned oracle (pandoc 3.10) via `pandoc -f commonmark+<ext> -t native`, which is the sanctioned
  clean-room source (observable CLI behavior). They are reproduced here so the executor does not need
  the oracle to understand the target; the conformance layer re-checks them live.

## 1. Why this matters

carta's `Extensions` set exists in `carta-core` and `ReaderOptions.extensions` carries it, but **no
reader consults it** — the CommonMark reader hardcodes the strict preset and takes `_options`. This
plan wires extensions through the reader and implements the five lowest-complexity ones, bringing the
reader to parity with `pandoc -f commonmark+strikeout+subscript+superscript+task_lists+hard_line_breaks`
and `+raw_html`. It also adds the missing plumbing every future extension needs: pandoc-style
`format+ext-ext` parsing in the facade, `Extension::from_name`, and an oracle-backed parity group in
the conformance suite that runs in CI.

Clears two `corpus/exclusions.tsv`-adjacent IOUs indirectly (task-list rendering is unblocked
downstream) and establishes the test pattern for the rest of the extension roadmap.

## 2. Scope

**In scope (reader-side parsing + wiring + parity/CI):**

- `carta-core`: add `Extension::HardLineBreaks` and `Extension::RawHtml`; add `Extension::from_name`;
  add `Extensions::union`; add `Error::UnknownExtension`.
- `carta` facade: parse `format[+ext][-ext]…` specs in `convert`; per-format default extension sets;
  thread the resulting `Extensions` into reader/writer options.
- `carta-readers` CommonMark: honor `options.extensions`; implement `strikeout`, `subscript`,
  `superscript`, `hard_line_breaks`, `task_lists`; recognize `raw_html` (already on-par — see §3.6).
- Tests: Layer 0 unit tests for the resolver and task-lists; Layer 1 golden snapshots over a new
  `corpus/text-ext/` corpus; Layer 2 a new oracle-backed `reader-ext` conformance group (CI-gated).

**Out of scope (deliberate, with rationale):**

- **Writer-side emission of these extensions.** The commonmark writer renders `Strikeout`/
  `Superscript`/`Subscript` as HTML fallback (`<s>`/`<sup>`/`<sub>`), which already matches the
  *strict* commonmark writer's output, so no regression. Emitting native `~~`/`^`/`~`/`[ ]` under
  `-t commonmark+ext` is a symmetric but separate unit (its own writer conformance group). Keeping
  this plan reader-only keeps the diff reviewable. Tracked as a follow-up (§9).
- **The other ~18 extensions** pandoc's commonmark reader supports (attributes, footnotes,
  pipe_tables, yaml_metadata_block, sourcepos, …). Separate plans.
- **The `markdown` alias's full pandoc-markdown default set.** `markdown` stays an alias of the
  commonmark reader with commonmark defaults; no markdown-specific conformance is added.
- **Extensions present in the enum but not implemented** (e.g. `footnotes`): they parse into the set
  and are silently ignored by the reader (the established `ReaderOptions.extensions` contract). Only
  names absent from the enum error as `UnknownExtension`.

## 3. Exact semantics (oracle-derived; these are the test oracle)

All confirmed against pandoc 3.10's commonmark reader. `☐` = U+2610, `☒` = U+2612.

### 3.1 `strikeout`, `subscript`, `superscript` — reuse the emphasis resolver, two changes

> **As-built correction.** An earlier draft modelled `~`/`^` as a separate "equal-char,
> equal-run-length" resolver. Oracle probing during implementation disproved that: `~`/`^` go through
> the **same `process_emphasis` walk as `*`/`_`**, rule-of-three and all. They differ in exactly two
> places. The examples below are the verified oracle output (pandoc 3.10) and double as the test
> oracle; the superseded examples (`^a^^b^`, `^^a^b^`, the equal-count rule) were predictions of the
> wrong model and have been removed.

`~` and `^` are **delimiter runs**, scanned and resolved exactly like `*`/`_` runs, recorded as
delimiters **only** when a relevant extension is enabled (`~`: subscript or strikeout; `^`:
superscript); otherwise the character is literal text. The two differences from `*`/`_`:

1. **Flanking is simplified to its first clause** (`flanking`, the `b'~' | b'^'` arm). A run *opens*
   unless whitespace follows it, and *closes* unless whitespace precedes it. The punctuation
   sub-clauses that `*`/`_` apply do **not** apply, so intraword opens freely and a punctuation
   neighbour does not block a run the way it would for `*`.
   - `.~a~`→`Str ".", Subscript[a]`, `a~!b~`→`Subscript[Str "!b"]`, `~b!~c`→`Subscript[Str "b!"], Str "c"` (all open).
   - `~a ~`→ literal (space before the closer ⇒ cannot close).

   The standard rule-of-three (`emphasis_match`) runs on top of this. Combined with the simplified
   flanking it accounts for the multiple-of-three cases: when opener + closer lengths sum to a
   multiple of three (and aren't both multiples of three), the pair resolves only if neither run can
   both open and close — i.e. the opener must follow whitespace.
   - `~a~~`→`Subscript[a], Str "~"` (opener after whitespace; sum 3 allowed).
   - `x~a~~`, `.~a~~`→ literal (opener is intraword/punctuation-adjacent ⇒ can both open and close ⇒
     sum 3 blocked).
   - `~~a~`→`Str "~", Subscript[a]`; `x~~a~`→ literal (same rule, lengths swapped).

2. **Use-count semantics** (`match_use_count`) decide how many delimiters a match consumes and what
   it wraps:
   - `^`: always consumes one → `Superscript`, so a longer run nests. `^^a^^`→`Superscript[Superscript[a]]`,
     `^^^a^^^`→ triple.
   - `~`: consumes two → a single `Strikeout` **iff** both runs ≥ 2 and strikeout is enabled; else
     consumes one → `Subscript` **iff** subscript is enabled; else it is not a delimiter.
     - both on:  `~a~`→`Subscript[a]`; `~~a~~`→`Strikeout[a]`; `z~~a~~`→`Str "z", Strikeout[a]`.
     - sub only: `~a~`→`Subscript[a]`; `x~~a~~`→`Str "x", Subscript[Subscript[a]]`.
     - strike only: `~~a~~`→`Strikeout[a]`; `~a~`→ literal (a length-one run is never a strikeout).

Nesting with emphasis works because resolution is a single left-to-right closer walk: inner closers
precede outer closers, so they resolve first and the outer drain/`collapse` sees them already
wrapped. Confirmed: `^a*b*c^`→`Superscript[a,Emph[b],c]`; `*em ~sub~*`→`Emph[em, Subscript[sub]]`.

Escapes disable a run: `\^a^`→`Str "^a^"`, and `\~~a~~`→`Str "~~a~~"` — the escaped `~` becomes a
literal punctuation neighbour, which (per change 1) lets the following run both open and close, so
the rule-of-three blocks the remaining `~a~~`. The existing `backslash` handler covers the escape
itself since `~`/`^` are ASCII punctuation.

### 3.2 `hard_line_breaks`

Every soft line break becomes a hard break: `a\nb` → `Para[Str"a", LineBreak, Str"b"]` (vs
`SoftBreak` without the extension). Inline-phase only.

### 3.3 `task_lists`

**Bullet lists only** (ordered lists do not get task markers — `1. [ ] x` stays literal). For each
bullet-list item, if the item's **first** leaf block (Para/Plain) begins with `[ ]`, `[x]`, or `[X]`
followed by a space or end-of-text, replace those three characters with `☐` (for `[ ]`) or `☒`
(for `[x]`/`[X]`), keeping any following space:

- `- [ ] todo` → `BulletList[[Plain[Str"☐", Space, Str"todo"]]]`
- `- [x] a` / `- [X] a` → `☒`
- `- [ ]` (no text) → `Plain[Str"☐"]`
- `- [ ]nospace` → not a task (`Str"[", Space, Str"]nospace"`)
- a `[ ]` that is not at the start of the item's first leaf is not a marker.

The replacement is on the **raw leaf text before inline parsing**, so `☐ todo` then parses normally.

**List splitting (as-built).** The oracle partitions a bullet list into maximal runs of consecutive
task / non-task items and emits **one `BulletList` per run**. A homogeneous list (all task, or all
non-task) is a single run, i.e. one list unchanged; a mixed list splits.
`- [ ] a\n- plain\n- [x] c` → three `BulletList` blocks: `[task a]`, `[plain]`, `[task c]`.
`resolve_bullet_list` implements this; with the extension off, no item classifies as a task so the
result is always one list (the prior behavior). The transform recurses into nested bullet lists.

**Known divergences (documented, not in the committed corpus).** Two oracle behaviors originate in
its *block* phase and are out of reach for a post-inline transform; both involve pathological input
and are deliberately excluded from `corpus/text-ext/`:

1. **Empty task items** (`- [ ]` / `- [x]` with no label) make the oracle *nest* the following item
   under the first (`- [ ]\n- y` → `[☐ , BulletList[y]]`). carta keeps them as siblings. Trigger: a
   task item whose only content is the checkbox.
2. **Tightness after a split.** When a *loose* list splits, the oracle recomputes tightness per
   resulting sub-list (a single-item run becomes tight → `Plain`). carta inherits the Para/Plain
   choice the block phase already baked in, so a split-off simple item stays `Para`. The blank-line
   information needed to recompute is gone by the inline phase. Tight lists (the common case) are
   unaffected.

Closing either would require integrating task-list recognition into the block parser; tracked as a
follow-up (§10), not blocking this slice.

### 3.4 `raw_html`

The commonmark reader already emits `RawInline (Format "html")` / `RawBlock (Format "html")` for
inline tags and HTML blocks, **regardless** of the toggle — the oracle's commonmark reader output is
byte-identical for `+raw_html` and `-raw_html`. So no reader behavior change is needed; we only make
`commonmark+raw_html` / `commonmark-raw_html` *parse* (recognize the name) and include `raw_html` in
the commonmark default set for fidelity. The reader ignores the flag.

### 3.5 Default extension set per format

pandoc's commonmark reader has exactly `+raw_html` on by default (everything else off). So
`default_extensions("commonmark") = {RawHtml}` and `default_extensions("markdown") = {RawHtml}`;
every other format → empty. `+ext` inserts, `-ext` removes, applied onto this base. Because `RawHtml`
is a reader no-op, `commonmark`, `commonmark+raw_html`, and `commonmark-raw_html` all produce
identical carta output — matching the oracle.

## 4. Current state (excerpts to build on)

- `crates/carta-core/src/extensions.rs`: `define_extensions!` macro generates the enum + `ALL`/
  `COUNT`/`name`. Current variants (11): `Smart, Strikeout, Superscript, Subscript, PipeTables,
  Footnotes, TaskLists, Autolink, TexMathDollars, FencedDivs, BracketedSpans`. `Extensions` is a
  `[u64; WORDS]` bitset; `contains`/`insert`/`remove`/`from_list`/`iter` exist. A compile-time assert
  requires variant discriminants stay contiguous (adding variants at the end is safe; `WORDS` stays 1
  until 64 variants).
- `crates/carta-core/src/lib.rs`: `Error` enum; `ReaderOptions { extensions }`, `WriterOptions
  { extensions }` (both `#[non_exhaustive]`); `Reader::read(&self, input, options)`.
- `crates/carta/src/lib.rs`: `convert(from, to, input, reader_options, writer_options)` →
  `reader_for(from)?` / `writer_for(to)?` then `read`/`write`. `crates/carta/src/registry.rs`:
  `reader_for`/`writer_for` resolve canonical names + aliases (`commonmark`|`markdown`,
  `html`|`html5`).
- `crates/carta-cli/src/main.rs`: passes `cli.from`/`cli.to` verbatim to `convert` with
  `ReaderOptions::default()`/`WriterOptions::default()`. **No CLI change needed** once `convert`
  parses specs.
- `crates/carta-readers/src/commonmark/mod.rs`: `read` calls `parse(input)` (ignores options);
  `parse` → `block::parse` → `inline::resolve_blocks(&ir, &refs)`. `IrBlock::BulletList(Vec<Vec<IrBlock>>)`.
- `crates/carta-readers/src/commonmark/inline.rs`: `parse_inlines(text, refs)` builds `InlineParser`,
  runs it, `process_emphasis(&mut nodes, 0)`, `collapse(nodes)`. `run()` dispatches by char;
  `emphasis_run(ch)` records `*`/`_` runs; `line_ending()` chooses `SoftBreak`/`LineBreak`;
  `process_emphasis` resolves `*`/`_` (the closer test gates on `d.ch == b'*' || d.ch == b'_'`);
  `flanking` special-cases `_`, else returns `(left_flanking, right_flanking)`; `collapse` renders
  leftover delimiters via `d.ch` (already generic over char).
- `crates/carta/tests/golden_reader.rs`: snapshots `convert(&case.group, "json", …)` for each
  `corpus/text/<fmt>/*`, skipping groups whose base format isn't a supported reader.
- `tools/conformance-suite/surfaces/reader.sh`: hardcodes `FORMATS="commonmark html native json"`,
  loops `corpus/text/$fmt/*`, `run_diff json … "-f $fmt -t json" "-f $fmt -t json"`. **e2e/roundtrip/
  writer/commands surfaces all hardcode their format lists**, so a new `corpus/text-ext/` dir is
  invisible to them — only the reader surface will be taught about it (this is what keeps the
  unimplemented writer side out of CI).
- `.github/workflows/ci.yml`: the `conformance` job provisions `.oracle/` and runs
  `tools/conformance-suite/run.sh all` — so any new reader-surface group runs in CI automatically.

Repo conventions that apply: no `unwrap`/`expect`/`panic`/slice-indexing outside `#[cfg(test)]`
(use `.get()` + `?`); comments only where the *why* is non-obvious; provenance rule — cite the
CommonMark spec/extension *syntax*, never the upstream tool, in product source; deterministic output;
Conventional Commits, explicit `git add` paths, no push.

## 5. Implementation steps

Branch: `feat/006-commonmark-easy-extensions` off `main`.

### Step 1 — `carta-core`: enum variants, `from_name`, `union`, error

In `crates/carta-core/src/extensions.rs`:

1. Append to `define_extensions!` (at the end, to preserve discriminant contiguity):
   `HardLineBreaks => "hard_line_breaks", RawHtml => "raw_html",`.
2. Extend the macro to also generate `pub fn from_name(name: &str) -> Option<Extension>` (match the
   `$name` literals → `Some(Extension::$variant)`, `_ => None`).
3. Add `Extensions::union(self, other: Extensions) -> Extensions` (bitwise OR over the word array).

In `crates/carta-core/src/lib.rs`: add `Error::UnknownExtension(String)` with message
`"unknown extension: {0}"`.

**Verify**: `cargo nextest run -p carta-core` (extend the existing `extensions.rs` tests with
`from_name` round-trips and a `union` case). `cargo clippy -p carta-core --all-targets`.

### Step 2 — `carta` facade: format-spec parsing

In `crates/carta/src/lib.rs` (or a small new `format_spec` module re-exported from it):

- `fn default_extensions(base: &str) -> Extensions` — `{RawHtml}` for `"commonmark"`/`"markdown"`,
  else empty.
- `pub fn parse_format_spec(spec: &str) -> Result<(String, Extensions)>`:
  - base = substring up to the first `+` or `-`; remainder parsed as a sequence of
    `(+|-)<name>` tokens.
  - start from `default_extensions(base)`; for each token, `Extension::from_name(name)` →
    insert (`+`) / remove (`-`); `None` → `Err(Error::UnknownExtension(name))`.
  - return `(base, set)`.
- Modify `convert` to parse both `from` and `to`, resolve trait objects on the **base** names, and
  build `ReaderOptions { extensions: from_ext.union(reader_options.extensions), ..clone }` /
  `WriterOptions` likewise. (Union so a programmatic `options.extensions` still composes; current
  callers pass `default()`, so this equals the spec's set.)

Re-export `parse_format_spec` from the crate root (the golden test uses it for the base-format skip
check).

**Note**: a plain base name (`"commonmark"`, `"json"`) parses to `(name, default_extensions(name))`,
so existing call sites are unaffected behaviorally (the commonmark reader ignores `RawHtml`).

**Verify**: add unit tests in `lib.rs` (or the new module) for `parse_format_spec`:
`"commonmark"` → `{RawHtml}`; `"commonmark+strikeout"` → `{RawHtml, Strikeout}`;
`"commonmark-raw_html"` → `{}`; `"commonmark+bogus"` → `Err(UnknownExtension)`; `"json"` → `{}`.
`cargo nextest run -p carta`.

### Step 3 — Reader wiring

In `crates/carta-readers/src/commonmark/mod.rs`:

- `read`: `Ok(parse(input, options.extensions))`.
- `parse(input, ext)`: thread `ext` into the inline phase and the task-list transform. Update the
  doc comment (drop the stale "deferred to the configurable markdown engine" line and the dangling
  `refactor-1-…` reference; state that the reader honors the extension set, listing the supported
  ones, and point at this plan / `docs/PORTING.md`).

In `inline.rs`: add an `ext: Extensions` field to `InlineParser` and thread `ext` through
`resolve_blocks → resolve_block → parse_inlines`. `process_emphasis` and the new format-run code
take/borrow `ext`.

### Step 4 — `hard_line_breaks`

In `line_ending`: choose `LineBreak` when `hard || backslash_hard || ext.contains(HardLineBreaks)`,
else `SoftBreak`.

### Step 5 — `~`/`^` scanning and resolution

- In `run()`, add arms for `'~'` and `'^'`:
  - `'~'`: if `ext.contains(Subscript) || ext.contains(Strikeout)` → `format_run(b'~')`; else
    `{ self.pos += 1; self.push_text('~') }`.
  - `'^'`: if `ext.contains(Superscript)` → `format_run(b'^')`; else literal.
  - `format_run` mirrors `emphasis_run` (consume the run, compute `flanking(ch, before, after)` —
    `flanking`'s else-branch already gives `*`-style rules for `~`/`^`, no change there — push a
    `Delimiter`).
- In `process_emphasis`, generalize the closer test to also accept `~`/`^`, and branch matching by
  char class:
  - `*`/`_`: unchanged (existing rule-of-3 / min-consume / decrement path).
  - `~`/`^`: find the nearest preceding opener with `d.ch == closer_ch`, `d.can_open`, and
    `d.count == closer_count`. If none, advance (do **not** literalize — unlike the `*`/`_`
    no-opener branch). On an equal-count opener, compute the nested wrappers per §3.1:
    - caret: N nested `Superscript`.
    - tilde: optional innermost `Strikeout` (if `ext` has Strikeout and N ≥ 2, consume 2), then a
      `Subscript` per remaining tilde (if `ext` has Subscript). If the count cannot be fully
      consumed, treat as no match (advance, leave literal).
    - On a valid match: drain `opener+1..closer`, `collapse`, build the nested inline, **remove both
      delimiter nodes entirely** (full consumption — not the decrement path), insert the wrapped
      inline, and `closer = stack_bottom` (consistent with the existing reset).
  - Extend the trailing "leftover delimiters become literal text" loop and/or rely on `collapse`
    (already char-generic) so unmatched `~`/`^` render literally. Confirm `convert_delimiter_to_text`
    is not needed for `~`/`^` (collapse handles them); extend its char guard only if a test shows a gap.
- Factor the wrapper-building into a small helper (e.g. `wrap_format(ch, count, content, ext) ->
  Option<Inline>`) to keep `process_emphasis` readable; `None` means "not consumable → no match".

Keep the `build_link` recursion path working: it calls `process_emphasis(&mut inner, 0)` then
`collapse` — pass `ext` through so `~`/`^` inside link text resolve.

**Maintenance note (add at the resolver)**: plan 003 will replace this with a linear delimiter
stack; its `openers_bottom` table must bucket `~`/`^` by `(char, exact count)` since they match by
equal length, not by the `*`/`_` `count mod 3` rule.

### Step 6 — `task_lists`

In `mod.rs` (or `inline.rs` `resolve_block`), when `ext.contains(TaskLists)`, transform
`IrBlock::BulletList` items before inline parsing: for each item, if its first `IrBlock` is
`Para`/`Plain` whose raw text starts with `[ ]`/`[x]`/`[X]` followed by a space or end-of-text,
replace the leading three characters with `☐`/`☒` (keep the rest verbatim). Apply recursively into
nested bullet lists / block quotes as the IR is resolved. Ordered lists are untouched.

Add a focused helper `fn task_marker_replacement(text: &str) -> Option<String>` returning the
rewritten leading text (or `None`), unit-tested in isolation.

### Step 7 — Layer 0 unit tests

Add `#[cfg(test)]` cases (in `inline.rs` for the resolver, `mod.rs`/`block.rs` for task-lists)
driving the reader through `CommonmarkReader::read` with explicit `ReaderOptions { extensions }` and
asserting the produced blocks. Cover, with the §3 expected values:

- strikeout: `~~a~~`, `a~~b~~c`, `~a~` (strike only → literal)
- subscript: `~a~`, `~~a~~` (sub only → nested), `H~2~O`
- superscript: `^a^`, `^a b^`, `^a^b^`, `^a^^b^`, `^^a^^`, `^^^a^^^`, `^^a^b^`
- both tildes: `a~~~b~~~c` → `Subscript[Strikeout[b]]`, `~~a ~b~ c~~`
- emphasis interplay: `^a*b*c^`, `*em ~sub~*`, `~~a *b~~ c*`
- escapes: `\~~a~~`, `\^a^`
- hard_line_breaks: `a\nb` (on → LineBreak, off → SoftBreak)
- task_lists: `- [ ] x`, `- [x] x`, `- [X] x`, `- [ ]`, `- [ ]nospace` (not a task),
  `1. [ ] x` (ordered → not a task), `[ ]` in a non-first block (not a task)
- gating: with the empty extension set, `~`/`^` are literal and `[ ]` is literal.

**Verify**: `cargo nextest run -p carta-readers`.

### Step 8 — Layer 1 golden corpus + snapshots

- Create `corpus/text-ext/<spec>/*.md`, one directory per extension config, the directory name being
  the **full format spec** (the single source of truth consumed identically by carta and the
  oracle). Suggested set:
  - `commonmark+strikeout/`, `commonmark+subscript/`, `commonmark+superscript/`,
    `commonmark+strikeout+subscript/`, `commonmark+superscript+subscript/`,
    `commonmark+hard_line_breaks/`, `commonmark+task_lists/`, `commonmark+raw_html/`.
  - Each with a few `.md` files exercising the §3 cases (these double as Layer-1 and Layer-2 inputs).
- Extend `crates/carta/tests/golden_reader.rs` with a second pass over `corpus_cases("text-ext")`:
  use `parse_format_spec(&case.group)?` to get the base, skip if the base isn't a supported reader,
  else `convert(&case.group, "json", &case.input, &ReaderOptions::default(), &WriterOptions::default())`
  and snapshot under a `text-ext__<group>__<label>` name. (`convert` re-parses the spec.) Add a tiny
  base-name helper to `common/mod.rs` if cleaner than calling `parse_format_spec`.

**Verify**: `cargo nextest run -p carta` produces `.snap.new` files; **do not accept them yet** —
they freeze carta's output, which must be proven correct against the oracle in Step 9 first.

### Step 9 — Layer 2 conformance group (the parity check, CI-gated)

In `tools/conformance-suite/surfaces/reader.sh`, after the existing format loop and before the spec
group, add a loop over `corpus/text-ext/*` directories that uses the directory name as the `-f` spec
for **both** the oracle and carta:

```sh
for dir in "$CORPUS"/text-ext/*; do
  [ -d "$dir" ] || continue
  spec="$(basename "$dir")"
  conf_reset "reader-ext-$spec"
  for input in "$dir"/*; do
    [ -f "$input" ] || continue
    run_diff json "reader-ext/$spec/$(basename "$input")" "$input" "-f $spec -t json" "-f $spec -t json"
  done
  report reader "ext-$spec"
  tally_group
done
```

Update the surface's header comment to document the new group. (No CI YAML change: the `conformance`
job already runs `run.sh all`.)

**Verify (requires `.oracle/` + `jq`, both present locally)**:
`cargo build -p carta-cli && tools/conformance-suite/run.sh reader` → every `RESULT reader ext-…`
line shows `fail=0 err=0`. Iterate on §5/§6 until clean. **Only once green**, accept the Layer-1
snapshots: `cargo insta accept` (they now equal the oracle's structure). Re-run
`cargo nextest run --workspace` → all pass, no pending snapshots.

### Step 10 — Full gate

**Verify** (all must pass):

- `cargo nextest run --workspace` → pass; `git status` shows no unexpected `.snap.new`.
- `cargo test --doc --workspace` → pass.
- `cargo clippy --all-targets --all-features` → exit 0, no warnings (CI uses `-D warnings`).
- `cargo fmt --all --check` → clean.
- `cargo build -p carta --no-default-features --features read-commonmark,write-html` and its
  `nextest run` (the `minimal` CI job) → pass (the facade's spec parsing must compile without all
  formats).
- `tools/conformance-suite/run.sh all` → no `fail`/`err` (confirms e2e/roundtrip/writer are
  unaffected, i.e. `corpus/text-ext/` leaked into no other surface).
- `cargo llvm-cov --workspace --summary-only --fail-under-lines 90` (run `cargo llvm-cov clean
  --workspace` first) → ≥ 90%.

## 6. Commands reference

| Purpose | Command | Expected |
|---|---|---|
| Unit/golden tests | `cargo nextest run --workspace` | all pass, no pending snapshots |
| Doctests | `cargo test --doc --workspace` | pass |
| Lint | `cargo clippy --all-targets --all-features` | exit 0, no warnings |
| Format | `cargo fmt --all --check` | clean |
| Snapshots | `cargo insta accept` (only after Step 9 parity is green) | snapshots written |
| Conformance (reader) | `cargo build -p carta-cli && tools/conformance-suite/run.sh reader` | `RESULT reader ext pass=9 fail=0 err=0` |
| Conformance (all) | `tools/conformance-suite/run.sh all` | no fail/err on any surface |
| Coverage | `cargo llvm-cov clean --workspace && cargo llvm-cov --workspace --summary-only --fail-under-lines 90` | ≥ 90% |
| Oracle probe (ad hoc) | `printf '%s' '<in>' \| .oracle/bin/pandoc -f commonmark+<ext> -t native` | the §3 expected |

## 7. Test plan summary

- **Layer 0** — resolver + task-list unit tests in `carta-readers`; `from_name`/`union`/
  `parse_format_spec` unit tests in `carta-core`/`carta`. Offline, edge-focused, oracle-derived
  expecteds baked in.
- **Layer 1** — golden AST-JSON snapshots over `corpus/text-ext/`. Offline regression net; frozen
  only after Layer 2 proves them correct.
- **Layer 2** — `reader-ext` conformance group diffs carta vs the pinned oracle live, per spec dir.
  Runs in CI via the existing `conformance` job. This is the parity guarantee.

## 8. Done criteria

- [x] `Extension::HardLineBreaks`, `Extension::RawHtml`, `Extension::from_name`, `Extensions::union`,
      `Error::UnknownExtension` exist with tests.
- [x] `parse_format_spec` handles base/`+`/`-`/defaults/unknown, tested.
- [x] CommonMark reader honors `options.extensions` for the five extensions; `commonmark` (no spec)
      output is byte-unchanged from before (652 spec examples + existing golden snapshots unchanged).
- [x] `corpus/text-ext/` exists; Layer-1 snapshots committed; `reader-ext` conformance group present.
- [x] `tools/conformance-suite/run.sh all` → no fail/err locally (with `.oracle/`); `reader ext pass=9`.
- [x] `cargo nextest run --workspace`, `--doc`, `clippy --all-targets --all-features`,
      `fmt --check`, the minimal-feature build, and coverage ≥ 90% (93.4%) all pass.
- [x] No `unwrap`/`expect`/`panic`/slice-indexing added outside `#[cfg(test)]`.
- [x] No upstream provenance introduced in product source (extension *syntax* and the CommonMark spec
      are fine to cite; the upstream tool is not).
- [x] `plans/README.md` status row updated.
- [x] Two task_lists block-phase divergences documented (§3.3) and excluded from the committed corpus.

## 9. STOP conditions

Stop and report (do not improvise) if:

- An **existing** golden snapshot (non-`text-ext`) changes — that means wiring extensions altered
  strict-commonmark output, a regression. Do not `insta accept` it.
- A `reader-ext` conformance case diverges from the oracle and the cause isn't resolved within two
  attempts — capture the input, carta's output, and the oracle's output in the report. The `~`/`^`
  model in §3.1 (simplified flanking + standard rule-of-three) was verified over a broad probe set; a
  new divergence is information, not something to paper over with a special case. (The two task_lists
  divergences in §3.3 are the known, accepted exceptions.)
- Implementing the resolver appears to need `unsafe` or slice indexing that can't be expressed with
  `.get()` — report the design instead of bending the panic rules.
- The minimal-feature build fails to compile — the facade's spec parsing must not depend on any
  optional format feature.

## 10. Follow-ups (out of scope here; note in `plans/README.md` if useful)

- **Writer-side emission** of these extensions in the commonmark writer under `-t commonmark+ext`,
  with a `writer-ext` conformance group (the symmetric counterpart to this plan).
- Expand `define_extensions!` to the full pandoc commonmark set (the remaining ~16 names) so presets
  and future readers can name them before each is implemented.
- The medium-tier extensions (pipe_tables, footnotes, fenced_divs/bracketed_spans + a shared
  attribute parser, tex_math_dollars + the math writer IOUs, …) — separate plans.
- **task_lists block-phase integration**: handle empty-task-item nesting and per-split tightness
  recomputation (§3.3 known divergences) by recognizing task markers in the block parser, where the
  blank-line/looseness information still exists.
