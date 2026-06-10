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

- **Priority**: P1
- **Effort**: M (one reader, contained; the delimiter resolver is the only subtle part)
- **Risk**: MED (a custom same-count delimiter resolver for `~`/`^`; output parity is the bar)
- **Depends on**: nothing. Note: plan 003 (linear delimiter stack) is still TODO and rewrites
  `process_emphasis`; this plan deliberately builds on the **current** quadratic resolver and leaves
  a maintenance note so 003 can absorb the new delimiter kinds.
- **Category**: feature (manually authored; not from the perf-focused improve run that produced 001–005)
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

### 3.1 `strikeout`, `subscript`, `superscript` — a same-count delimiter resolver

`~` and `^` are **delimiter runs**, scanned like `*`/`_` runs, using the `*` flanking rules
(intraword allowed — `a~~b~~c` → `Strikeout[b]`). They are recorded as delimiters **only** when a
relevant extension is enabled (`~`: subscript or strikeout; `^`: superscript); otherwise the
character is literal text.

Resolution differs from `*`/`_` emphasis in two ways:

1. **Matching is by equal char and equal run length** (no rule of 3). A closer run of count N
   matches the nearest preceding opener run of the *same char* with `can_open` and *count == N*.
   A run that finds no equal-count opener is left in place (it may serve as an opener for a later
   closer) and becomes literal only if still unmatched at the end.
   - `^a^^b^` → `Superscript[Str "a^^b"]` — the inner `^^` (count 2) finds no count-2 partner and
     stays literal; the outer `^`(1) pair wraps everything between, `^^` included.
   - `^^a^b^` → `Str "^^a", Superscript[b]` — `^^`(2) is literal (no count-2 closer); `^`(1) pair wraps `b`.
   - `^a^b^c^` → `Superscript[a], Str "b", Superscript[c]`.

2. **A matched run of count N expands to nested wrappers, built from the content outward, and the
   match is valid only if every tilde/caret is consumed:**
   - `^` (count N): N nested `Superscript`. (`^^a^^` → `Superscript[Superscript[a]]`;
     `^^^a^^^` → triple.)
   - `~` (count N): innermost a single `Strikeout` **iff** strikeout is enabled and N ≥ 2 (consumes
     2); then one `Subscript` layer per remaining tilde **iff** subscript is enabled (consumes 1
     each). The pair forms only if the count is fully consumed this way; otherwise the runs stay
     literal.
     - both on:  `~a~`→`Subscript[a]`; `~~a~~`→`Strikeout[a]`; `a~~~b~~~c`→`Str"a",Subscript[Strikeout[b]],Str"c"`.
     - sub only: `~a~`→`Subscript[a]`; `~~a~~`→`Subscript[Subscript[a]]`.
     - strike only: `~~a~~`→`Strikeout[a]`; `~a~`→ literal `~a~` (count 1 cannot be consumed).

Nesting with emphasis works because resolution is a single left-to-right closer walk: inner closers
precede outer closers, so they resolve first and the outer drain/`collapse` sees them already
wrapped. Confirmed: `^a*b*c^`→`Superscript[a,Emph[b],c]`; `*em ~sub~*`→`Emph[em, Subscript[sub]]`;
`~~a *b~~ c*`→`Strikeout[Str"a",Space,Str"*b"], Space, Str"c*"` (the `*` does not cross the `~~`
boundary — its opener collapses to literal inside the drained strikeout content).

Escapes disable a run: `\~~a~~`→`Str "~~a~~"`, `\^a^`→`Str "^a^"` (the existing `backslash` handler
already covers this since `~`/`^` are ASCII punctuation).

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
| Conformance (reader) | `cargo build -p carta-cli && tools/conformance-suite/run.sh reader` | `RESULT reader ext-… fail=0 err=0` |
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

- [ ] `Extension::HardLineBreaks`, `Extension::RawHtml`, `Extension::from_name`, `Extensions::union`,
      `Error::UnknownExtension` exist with tests.
- [ ] `parse_format_spec` handles base/`+`/`-`/defaults/unknown, tested.
- [ ] CommonMark reader honors `options.extensions` for the five extensions; `commonmark` (no spec)
      output is byte-unchanged from before.
- [ ] `corpus/text-ext/` exists; Layer-1 snapshots committed; `reader-ext` conformance group present.
- [ ] `tools/conformance-suite/run.sh all` → no fail/err locally (with `.oracle/`).
- [ ] `cargo nextest run --workspace`, `--doc`, `clippy --all-targets --all-features`,
      `fmt --check`, the minimal-feature build, and coverage ≥ 90% all pass.
- [ ] No `unwrap`/`expect`/`panic`/slice-indexing added outside `#[cfg(test)]`.
- [ ] No upstream provenance introduced in product source (extension *syntax* and the CommonMark spec
      are fine to cite; the upstream tool is not).
- [ ] `plans/README.md` status row updated.

## 9. STOP conditions

Stop and report (do not improvise) if:

- An **existing** golden snapshot (non-`text-ext`) changes — that means wiring extensions altered
  strict-commonmark output, a regression. Do not `insta accept` it.
- A `reader-ext` conformance case diverges from the oracle and the cause isn't resolved within two
  attempts — capture the input, carta's output, and the oracle's output in the report. In
  particular, the same-count / nesting rule in §3.1 was derived from a finite probe set; a divergent
  case (e.g. some `count ≥ 4` tilde combination, or a `~`/`^` flanking corner) is new information,
  not something to paper over with a special case.
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
