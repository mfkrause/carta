# Plan 004: Remove avoidable allocations on reader hot paths (allocation hygiene bundle)

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 5e110f9..HEAD -- crates/carta-readers/src/commonmark/scan.rs crates/carta-readers/src/commonmark/html_block.rs crates/carta-readers/src/html/convert.rs crates/carta-ast/src/ast.rs`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P2
- **Effort**: M (six small independent fixes, each S)
- **Risk**: LOW
- **Depends on**: plans/001-criterion-bench-suite.md (for before/after numbers; the fixes are safe without it)
- **Category**: perf
- **Planned at**: commit `5e110f9`, 2026-06-10

## Why this matters

Several reader-side helpers allocate temporary heap structures on every call — intermediate `Vec`s that are immediately joined and discarded, a pattern `Vec<char>` rebuilt per search, whole-line lowercase copies per line of an HTML block, and a front-removal loop that shifts an entire vector per element. Each is small, but they sit on per-line / per-label / per-tag paths, so a large document pays them thousands of times. Each fix is local, behavior-preserving, and verified by the existing snapshot suite.

## Current state

Six sites, each with today's code:

**(a)** `crates/carta-readers/src/commonmark/scan.rs:497-500` — called for every link label and link reference definition:

```rust
pub(crate) fn normalize_label(label: &str) -> String {
    let collapsed = label.split_whitespace().collect::<Vec<_>>().join(" ");
    caseless::default_case_fold_str(&collapsed)
}
```

**(b)** `crates/carta-ast/src/ast.rs:319` — inside `pub fn slug(text: &str)`, called per heading:

```rust
let joined = filtered.split_whitespace().collect::<Vec<_>>().join("-");
```

**(c)** `crates/carta-readers/src/commonmark/scan.rs:275-285` — `match_until`, called while scanning entities/HTML constructs; allocates the needle as `Vec<char>` on every call even for fixed 2–3 char needles:

```rust
fn match_until(chars: &[char], from: usize, needle: &str) -> Option<usize> {
    let pattern: Vec<char> = needle.chars().collect();
    let mut index = from;
    while index + pattern.len() <= chars.len() {
        if chars.get(index..index + pattern.len()) == Some(pattern.as_slice()) {
            return Some(index + pattern.len());
        }
        index += 1;
    }
    None
}
```

Note the file already has an allocation-free `matches_at(chars, index, needle)` helper directly above it (scan.rs:267-272).

**(d)** `crates/carta-readers/src/html/convert.rs:754-762` — `Vec::remove(0)` in a loop shifts the whole vector per removed element:

```rust
fn trim_inlines(mut inlines: Vec<Inline>) -> Vec<Inline> {
    while matches!(inlines.first(), Some(Inline::Space | Inline::SoftBreak)) {
        inlines.remove(0);
    }
    while matches!(inlines.last(), Some(Inline::Space | Inline::SoftBreak)) {
        inlines.pop();
    }
    inlines
}
```

**(e)** `crates/carta-readers/src/commonmark/html_block.rs` — `classify` lowercases the whole remaining line to test four fixed tag names (line ~27: `let lower = rest.to_ascii_lowercase();`), and `closes` lowercases every line of an open type-1 HTML block (line ~52: `let lower = line.to_ascii_lowercase();` then `["</script>", …].iter().any(|needle| lower.contains(needle))`).

**(f)** `crates/carta-readers/src/html/convert.rs:67-70` — every element conversion collects a `Vec<&Node>` just to call `blocks`:

```rust
fn child_blocks(&mut self, children: &[Node], in_list: bool) -> Vec<Block> {
    let refs: Vec<&Node> = children.iter().collect();
    self.blocks(&refs, in_list)
}
```

`blocks` (convert.rs:59) takes `&[&Node]` and only does `nodes.iter().copied()` with it; `process` takes `impl Iterator<Item = &'a Node>`.

Repo conventions: no `unwrap`/`expect`/slice-indexing outside tests (lint-enforced); output must stay byte-identical (golden snapshots are the gate); comments only for non-obvious *why*.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Tests | `cargo nextest run --workspace` | all pass, zero snapshot diffs |
| Lint | `cargo clippy --all-targets` | exit 0, no new warnings |
| Benches (if 001 landed) | `cargo bench -p carta -- read_commonmark` | no regression; likely small win |

## Scope

**In scope**:
- `crates/carta-readers/src/commonmark/scan.rs` — sites (a), (c) only
- `crates/carta-readers/src/commonmark/html_block.rs` — site (e)
- `crates/carta-readers/src/html/convert.rs` — sites (d), (f)
- `crates/carta-ast/src/ast.rs` — site (b) only
- `plans/README.md` (status row)

**Out of scope**:
- `crates/carta-readers/src/commonmark/inline.rs` — covered by plan 003; do not touch even though it calls `normalize_label`.
- The `Vec<char>` scanning architecture itself (parameters of `match_until` etc. stay `&[char]`) — that is plan 005's investigation.
- `unique_id` in convert.rs:384 (format-per-candidate loop): considered and rejected — collisions are rare and bounded by heading count; not worth the churn.
- Any writer crate file.

## Git workflow

- Branch: `advisor/004-allocation-hygiene` off `main`.
- One commit per site or one commit total — either is fine; Conventional Commits, e.g. `perf(readers): drop intermediate allocations in label/slug/scan helpers`.
- Stage explicit paths only. Do NOT push.

## Steps

Each step is independent; run the verify command after each.

### Step 1: Single-pass whitespace collapse in `normalize_label` (a) and `slug` (b)

Replace the `split_whitespace().collect::<Vec<_>>().join(sep)` pattern with a single fold over `split_whitespace()`:

```rust
let mut collapsed = String::with_capacity(label.len());
for word in label.split_whitespace() {
    if !collapsed.is_empty() {
        collapsed.push(' '); // '-' for slug
    }
    collapsed.push_str(word);
}
```

Output must be byte-identical to the join (it is: `join` produces exactly this).

**Verify**: `cargo nextest run --workspace` → all pass, no snapshot changes.

### Step 2: Allocation-free `match_until` (c)

Rewrite using the existing `matches_at` helper:

```rust
fn match_until(chars: &[char], from: usize, needle: &str) -> Option<usize> {
    let len = needle.chars().count();
    (from..=chars.len().saturating_sub(len))
        .find(|&index| matches_at(chars, index, needle))
        .map(|index| index + len)
}
```

Careful with the empty-`chars` / `len > chars.len()` edge: the original returns `None` when the needle can't fit; `saturating_sub` plus the inclusive range must preserve that (when `len > chars.len()`, the range is `from..=0` — guard with an explicit `if len > chars.len() { return None; }` to keep it obviously correct). `needle` is always ASCII at current call sites, but don't rely on that.

**Verify**: `cargo nextest run -p carta-readers` → all pass.

### Step 3: Trim without shifting in `trim_inlines` (d)

Compute the first/last retained indices, then drain once:

```rust
fn trim_inlines(mut inlines: Vec<Inline>) -> Vec<Inline> {
    let is_pad = |inline: &Inline| matches!(inline, Inline::Space | Inline::SoftBreak);
    let keep_from = inlines.iter().take_while(|inline| is_pad(inline)).count();
    inlines.truncate(inlines.len() - inlines.iter().rev().take_while(|inline| is_pad(inline)).count().min(inlines.len() - keep_from));
    inlines.drain(..keep_from);
    inlines
}
```

Or any equivalent that keeps the signature and avoids per-element `remove(0)`. Watch the all-padding case (result must be empty, no underflow).

**Verify**: `cargo nextest run -p carta-readers` → all pass.

### Step 4: Case-insensitive checks without lowercasing whole lines (e)

In `html_block.rs`:

- `classify`: instead of `rest.to_ascii_lowercase()`, strip `<` and compare the next bytes against each of the four tag names with `eq_ignore_ascii_case` on a prefix slice obtained via `.get(..tag.len())`, then check the follower char as today.
- `closes` kind 1: replace `line.to_ascii_lowercase().contains(needle)` with a small case-insensitive substring search (slide a window with `line.as_bytes()` and `eq_ignore_ascii_case`, or scan for `<` and compare the tail prefix case-insensitively). Keep semantics: match anywhere in the line.

**Verify**: `cargo nextest run -p carta-readers` → all pass.

### Step 5: Drop the `Vec<&Node>` in `child_blocks` (f)

Preferred: change `Converter::blocks` to take `&[Node]` and iterate `nodes.iter()`; update its callers. First enumerate call sites with `grep -n '\.blocks(' crates/carta-readers/src/html/*.rs` — if some caller genuinely owns only `Vec<&Node>` (mixed-source nodes), instead make `blocks` generic over `impl Iterator<Item = &'a Node>` (it already forwards to `process`, which takes exactly that). Either way `child_blocks` must stop collecting.

**Verify**: `cargo nextest run --workspace` → all pass; `cargo clippy --all-targets` → exit 0.

### Step 6: Before/after numbers (only if plan 001 landed)

`cargo bench -p carta -- read_commonmark` and `-- read_corpus` before and after (use `git stash`-free approach: run on `main`, then on the branch). Record in the commit message. No regression allowed; wins may be small — that's fine, this plan is hygiene.

## Test plan

- No new test files: every site is exercised by the existing layer-1 snapshots (`corpus/text/commonmark/*` and `corpus/text/html/*` flow through these helpers) plus in-crate unit tests.
- Add unit tests only where the rewrite has an edge the suite may not pin: `match_until` with needle longer than input, `trim_inlines` with all-padding input. Put them in the files' existing `#[cfg(test)]` modules.
- Verification: `cargo nextest run --workspace` → all pass, zero snapshot diffs.

## Done criteria

- [ ] `grep -n 'collect::<Vec<_>>().join' crates/carta-readers/src/commonmark/scan.rs crates/carta-ast/src/ast.rs` returns no matches
- [ ] `grep -n 'inlines.remove(0)' crates/carta-readers/src/html/convert.rs` returns no matches
- [ ] `grep -n 'to_ascii_lowercase' crates/carta-readers/src/commonmark/html_block.rs` returns no matches
- [ ] `cargo nextest run --workspace` exits 0; no `.snap`/`.snap.new` changes in `git status`
- [ ] `cargo clippy --all-targets` exits 0, no new warnings
- [ ] No files outside the in-scope list modified (`git status`)
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- Any golden snapshot changes — these rewrites must be byte-for-byte behavior-preserving. Do not `cargo insta review`.
- Step 5 reveals `blocks` callers that can't be unified under either signature within a small diff — report the call-site inventory instead of forcing it.
- A clippy restriction lint (e.g. `indexing_slicing`) fires on your rewrite and the clean fix isn't obvious — report rather than adding `#[allow]`.

## Maintenance notes

- Plan 005 may replace the `&[char]` scanning representation wholesale; steps 2's rewrite survives that (it goes through `matches_at`, which would be rewritten with the representation).
- Reviewer focus: `closes` kind-1 substring semantics (case-insensitive `contains`) — a subtle mismatch would only show on HTML blocks closed mid-line.
