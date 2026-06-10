# Plan 005: Spike — measure the cost of `Vec<char>` scanning and oversized `Inline` nodes

> **Executor instructions**: This is an INVESTIGATION plan. The deliverable is
> a written report (`plans/005-report.md`) with measurements and a
> recommendation — NOT merged optimizations. Prototype code stays on a branch
> and is referenced from the report. Follow the steps in order; honor the STOP
> conditions. When done, update the status row in `plans/README.md`.
>
> **Drift check (run first)**: `git diff --stat f5d2e3b..HEAD -- crates/carta-ast/src/ast.rs crates/carta-readers/src/commonmark/inline.rs crates/carta-readers/src/html/tokenize.rs`
> On any in-scope drift, re-verify the "Current state" excerpts before
> proceeding; on a mismatch, treat it as a STOP condition.

## Status

- **Priority**: P2
- **Effort**: M (timeboxed spike; the follow-up refactors would each be L)
- **Risk**: LOW (no production code merges)
- **Depends on**: plans/001-criterion-bench-suite.md (hard dependency — without benches there is nothing to measure against)
- **Category**: perf
- **Planned at**: commit `5e110f9`, 2026-06-10 (reconciled at `f5d2e3b`, 2026-06-10 — no in-scope drift, excerpts re-verified)

## Why this matters

Two architectural costs are baked in deeply enough that "just fix it" would be a multi-day, high-churn refactor — so we measure first:

1. **`Vec<char>` scanning.** The CommonMark inline phase collects every leaf block's text into a `Vec<char>` (4 bytes/char, one allocation per paragraph), and every scanner helper then re-collects char sub-slices into `String`s. The HTML reader tokenizes the *entire input* the same way. Byte-offset scanning over `&str` would eliminate the conversions, but touches every scanner signature.
2. **`Inline` node size.** `Inline::Link(Attr, Vec<Inline>, Target)` makes the largest variant ~144 bytes of payload, so **every** `Inline` — including each `Space` and `Str` between words — occupies ~152 bytes in every `Vec<Inline>`. Documents are dominated by `Str`/`Space`; boxing the fat variants could shrink the node ~3× with corresponding allocator and cache-pressure savings on every reader, writer, and clone.

Both are plausible big wins for a project whose first goal is speed — and both are expensive to do. This spike produces numbers so the maintainer can decide which (if either) to commission as a full plan.

## Current state

- `crates/carta-readers/src/commonmark/inline.rs:69-81`:

  ```rust
  fn parse_inlines(text: &str, refs: &RefMap) -> Vec<Inline> {
      let chars: Vec<char> = text.chars().collect();
      ...
  }
  ```

  Everything in `crates/carta-readers/src/commonmark/scan.rs` takes `chars: &[char]`, and re-materializes `String`s via `.iter().collect()` (e.g. `code_span` at inline.rs:175-179, `raw_label` at inline.rs:379-383, `unescape_string` at scan.rs:~505).

- `crates/carta-readers/src/html/tokenize.rs:28`: `pub(super) fn tokenize(chars: &[char]) -> Vec<Token>` over the whole input; `slice()` (tokenize.rs:276) rebuilds a `String` per tag name/attribute/text chunk.

- `crates/carta-ast/src/ast.rs:108-134`: the `Inline` enum; fat variants `Link(Attr, Vec<Inline>, Target)`, `Image(…)`, `Code(Attr, Text)`, `Span(Attr, Vec<Inline>)`. `Attr` = `{ Text, Vec<Text>, Vec<(Text, Text)> }` (72 bytes), `Target` = two `Text`s (48 bytes). `Block::Table` is already `Box<Table>` (ast.rs:101) — precedent for boxing fat payloads.

- Serde: the enums derive `Serialize`/`Deserialize` with `#[serde(tag = "t", content = "c")]` via the `node_enum!` macro. Boxing a variant's payload (`Link(Box<LinkData>)`) changes the serialized shape unless the boxed struct (de)serializes as the same tuple — the JSON interchange format is a hard external contract (`carta -f json -t json` round-trips, layer-1 snapshots pin it).

- Clone sites that multiply node size: readers build `Vec<Inline>` everywhere; writers traverse by reference but several paths clone subtrees (e.g. `inline.rs` `Node::Inline(Inline)` buffering).

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Benches | `cargo bench -p carta` | baseline + prototype numbers |
| Tests | `cargo nextest run --workspace` | all pass on each prototype branch |
| Node sizes | (Step 1 test below) | prints sizes |
| Peak memory | `/usr/bin/time -l ./target/release/carta -f commonmark -t html big.md` (macOS; `-v` on Linux) | "maximum resident set size" line |

## Scope

**In scope** (prototype branches + report only):
- `plans/005-report.md` (create — the deliverable)
- Prototype branches `advisor/005-spike-bytes` and `advisor/005-spike-boxing` touching reader/AST code freely — **never merged**, only referenced.
- A size-assertion test added to `crates/carta-ast/src/ast.rs`'s test module (the one production-adjacent change worth landing; it's a test).

**Out of scope**:
- Merging either refactor — that is a follow-up plan after the maintainer reads the report.
- The JSON wire format — any prototype must keep `cargo nextest` green, snapshots identical.

## Git workflow

- Report lands on `advisor/005-spike` (just the two `plans/` files + the size test).
- Prototypes on their own branches as above; commit messages `wip(spike): …` are acceptable there per repo conventions (`wip` is a sanctioned type).
- Do NOT push any branch.

## Steps

### Step 1: Pin the facts

Add to `crates/carta-ast/src/ast.rs`'s `#[cfg(test)]` module a test that prints (and asserts, to catch silent growth later) the sizes:

```rust
#[test]
fn node_sizes() {
    let inline = std::mem::size_of::<Inline>();
    let block = std::mem::size_of::<Block>();
    println!("Inline: {inline} B, Block: {block} B");
    assert!(inline <= 160, "Inline grew: {inline} B");
    assert!(block <= 160, "Block grew: {block} B");
}
```

Record the actual numbers in the report (run with `cargo nextest run -p carta-ast --no-capture`). Then capture the baseline: full `cargo bench -p carta` output and peak RSS converting a ~10 MB generated CommonMark file (generate with the plan-001 bench generators, written to a temp file via a small `cargo run` or by copying a generator into a scratch test; alternatively, repeat `corpus/bench/seed.md` to size — `tools/bench-suite/gen-fixtures.sh` builds sized CommonMark fixtures under `target/bench/` this way and needs only `jq` plus a `cargo build --release -p carta-cli` binary, with `BENCH_SIZES=10m` for the 10 MB case).

**Verify**: report file exists with a "Baseline" section containing node sizes, bench summary, peak RSS.

### Step 2: Prototype A — byte-offset scanning (timebox: focus on ONE reader)

On `advisor/005-spike-bytes`, convert **only the HTML tokenizer** (`crates/carta-readers/src/html/tokenize.rs`) from `&[char]`+positions to `&str`+byte offsets (`as_bytes()`, `char_indices` where needed; `slice()` becomes a borrow `&input[start..end]` → `.to_owned()` only at token construction — keep `.get()` discipline). The HTML tokenizer is chosen because it is self-contained (one file, ~350 lines) and representative of the same pattern in the CommonMark scanners.

Gate: `cargo nextest run --workspace` green, snapshots unchanged. Then `cargo bench -p carta` and record the delta on HTML-reading paths (add a quick HTML reader bench to the suite on this branch if plan 001 didn't include one — generators can wrap prose in `<p>` tags).

**Verify**: report has a "Prototype A" section with the bench delta and a paragraph estimating, by analogy, the win for doing the same to the CommonMark inline scanners (which run on far more input in practice).

### Step 3: Prototype B — box the fat `Inline` variants

On `advisor/005-spike-boxing`, change `Inline::Link`/`Image` to carry boxed payloads while keeping the wire format identical. Two candidate shapes — try the first; fall back to the second:

1. `Link(Box<(Attr, Vec<Inline>, Target)>)` with `#[serde(with = …)]` or manual impls in `crates/carta-ast/src/serde_impls.rs` so JSON stays `{"t":"Link","c":[attr, inlines, target]}`.
2. Keep the variant arity but box only `Attr` (smaller win, less serde friction).

Update construction sites compiler-led (the compiler lists them; expect ~dozens across readers/writers). Gate: full `cargo nextest run --workspace` green, **zero snapshot diffs** — the JSON layer-1 snapshots are exactly the contract check. Re-run the size test, benches, and peak-RSS measurement.

**Verify**: report has a "Prototype B" section with new `size_of::<Inline>()`, bench deltas (readers AND writers — boxing adds a pointer chase on traversal; it can lose), and peak RSS delta.

### Step 4: Write the recommendation

Conclude `plans/005-report.md` with: for each prototype, the measured win/loss, the estimated full-refactor effort, and a clear DO / DON'T / DO-LATER recommendation. List which follow-up plans should be written if DO.

## Test plan

- The `node_sizes` test (Step 1) is the only test that lands.
- Prototype branches must each pass `cargo nextest run --workspace` with zero snapshot diffs before their numbers count — a prototype that changes behavior measures nothing.

## Done criteria

- [ ] `plans/005-report.md` exists with Baseline, Prototype A, Prototype B, Recommendation sections, all with concrete numbers
- [ ] `node_sizes` test landed and passes (`cargo nextest run -p carta-ast`)
- [ ] Both prototype branches exist locally, each green (`cargo nextest run --workspace`)
- [ ] No production code changed on the report branch (`git diff --stat main` shows only `plans/` + the ast.rs test module)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- Plan 001's benches don't exist — this plan cannot proceed without them.
- Prototype B cannot keep the JSON wire format identical without hand-writing large serde impls (> ~100 lines) — record that as the finding ("boxing requires serde_impls investment of size X") and skip to the report.
- Prototype A's byte-offset conversion produces any snapshot diff that isn't a clear bug in the prototype — UTF-8 boundary handling is exactly where this refactor can silently change behavior; report rather than patch around it.
- Either prototype exceeds roughly a day of effort — timebox hit; write up what was measured.

## Maintenance notes

- If Prototype A recommends DO: the follow-up plan should convert `commonmark/scan.rs` + `inline.rs` together (they share the `&[char]` convention), keeping `match_until`-style helpers' semantics from plan 004.
- If Prototype B recommends DO: writers' pattern-matches all need `..` or deref adjustments; the JSON snapshots and `carta -f json -t json` identity tests are the safety net.
- The `node_sizes` assertion thresholds should be tightened to the post-refactor sizes if either refactor lands.
