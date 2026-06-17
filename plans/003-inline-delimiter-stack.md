# Plan 003: Make CommonMark inline emphasis/bracket resolution linear instead of quadratic

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat cf540ef..HEAD -- crates/carta-readers/src/commonmark/inline.rs`
> If the file changed since this plan was written, compare the "Current state"
> excerpts against the live code before proceeding; on a mismatch, treat it as
> a STOP condition.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: MED
- **Depends on**: plans/001-criterion-bench-suite.md (provides `emphasis_heavy` and `pathological_brackets` regression benches)
- **Category**: perf
- **Planned at**: commit `5e110f9`, 2026-06-10 (reconciled at `cf540ef`, 2026-06-10). Two in-scope changes landed since the first reconcile: plan 006's extension work (`a0c456d`) threads an `Extensions` set through the inline phase and adds `~`/`^` delimiter kinds resolved by the same emphasis walk, and `e21042c` moved use-count selection into the opener search (`match_use_count`). The quadratic restart, the per-closer backward opener scan, and the full-list bracket scan all survive unchanged; excerpts and line numbers below are refreshed to `cf540ef`. Plan 001's benches are on `main` at this commit.

## Executed (2026-06-11)

Four commits on `worktree-agent-a4dba10cd5f705906`, head `02e1e7b`; reviewed and approved, awaiting operator merge.

- **Outcome**: `pathological_brackets` is now linear — large/small ratio 9.5× → 3.4–3.7×, absolute large time 23.8 ms → 2.7 ms. All 248 offline tests pass with zero snapshot churn, and the **full conformance suite is green** (652/652 spec examples on both reader and e2e surfaces).
- **Review found one regression** (fixed in revision `02e1e7b`): the first bracket-stack implementation removed deactivated link openers from the stack eagerly, letting a later `]` reach an active image opener below too early; `![[[foo](uri1)](uri2)](uri3)` produced the wrong nesting. The fix restores the spec's lazy semantics (inactive top is popped and literalized, one per `]`) and adds a regression test. The plan's Step-1 test matrix had no case with an active image opener *below* a deactivated link opener — add that shape to future inline-parser test matrices.
- **Documented deviation**: `emphasis_heavy` still scales ~10× (quadratic). The plan's allowance "Vec::remove/insert is acceptable to keep" contradicted its own done criterion: with O(n) *matched pairs*, the per-match O(n) node splices (and the delimiter-list index-shift loop the rewrite adds) are themselves a quadratic term that dominates on matched-pair-heavy input. The rescan elimination only wins when unmatched closers dominate (the brackets bench). Making `emphasis_heavy` linear requires a node-representation change (deferred splicing or a linked/tree node structure) — that belongs with plan 005's spike on AST/scanning representation, not a revision round here.

## Why this matters

The inline parser's emphasis resolution restarts its scan from the bottom of the node list after **every** successful emphasis match, and searches backward through all preceding nodes for each closer. A paragraph with k emphasis pairs therefore does O(k) full rescans — quadratic work on completely ordinary emphasis-heavy documents, not just adversarial ones. Separately, every `]` triggers a backward scan over the entire node list to find a bracket opener, which is quadratic on `]`-heavy input. A document converter ingests arbitrary input, so a quadratic blowup is also a denial-of-service vector. The CommonMark specification's appendix ("A parsing strategy", in the vendored spec) describes the standard linear-time delimiter-stack algorithm; this plan adopts it.

## Current state

All in `crates/carta-readers/src/commonmark/inline.rs` (line numbers as of `cf540ef`; the file is 889 lines):

- `parse_inlines` (line 164, signature `fn parse_inlines(text: &str, refs: &RefMap, ext: Extensions) -> Vec<Inline>`) builds a flat `Vec<Node>` where `Node::Delimiter` entries represent `*`/`_`/`~`/`^` runs and `[`/`![` openers, then calls `process_emphasis(&mut nodes, 0, ext)` (line 175).
- `process_emphasis` (lines 516–581, signature `fn process_emphasis(nodes: &mut Vec<Node>, stack_bottom: usize, ext: Extensions)`). The three performance problems, as the code stands today:

  ```rust
  // line 517-518: forward scan for a closer
  let mut closer = stack_bottom;
  while closer < nodes.len() {
      ...
      // lines 530-543: backward scan for the opener — O(n) per closer
      let mut opener = None;
      let mut index = closer;
      while index > stack_bottom {
          index -= 1;
          if let Some(Node::Delimiter(d)) = nodes.get(index)
              && d.can_open
              && d.ch == closer_ch
              && emphasis_match(d, nodes, closer)
              && let Some(use_count) = match_use_count(d.count, closer_count, closer_ch, ext)
          {
              opener = Some((index, use_count));
              break;
          }
      }
      ...
      // line 575: after every successful match, restart from the bottom
      closer = stack_bottom;
  }
  ```

- **Four delimiter kinds**, not two: `is_delimiter_char` (line 584) admits `*`, `_`, `~` (strikeout/subscript when those extensions are on), and `^` (superscript). How many delimiters a matched pair consumes lives in `match_use_count` (lines 594–615) and the pair→inline mapping in `wrap_emphasis` (lines 619–627). Note that `match_use_count` can return `None` for `~` (strikeout on, subscript off, and either run shorter than 2), in which case the opener search **skips that opener and keeps scanning** — opener rejection is no longer a function of rule-of-3 alone. This matters for `openers_bottom` bucketing (see Step 2).
- `last_bracket_opener` (lines 428–434) — full backward scan of `nodes` on every `]`:

  ```rust
  fn last_bracket_opener(&self) -> Option<usize> {
      self.nodes
          .iter()
          .enumerate()
          .rev()
          .find_map(|(i, node)| matches!(node, Node::Delimiter(d) if d.ch == b'[').then_some(i))
  }
  ```

- `process_emphasis` is called from two places: `parse_inlines` (line 175) and `build_link` (line 491, on the link's inner nodes with `stack_bottom = 0` after `self.nodes.split_off(opener_index + 1)`).
- The match-eligibility rules live in `flanking` (line 731 — note the `~`/`^` clause anchors only on whitespace) and `emphasis_match` (line 629, the "rule of 3"); `match_use_count` and the node-splicing logic (lines 555–573: `drain` + `collapse` + `wrap_emphasis` + `decrement_delimiter` + drop-emptied) are behaviorally correct today — the 652 vendored spec examples pass byte-identically and the extension toggles have their own snapshot coverage. **Behavior must not change; only complexity.**
- A `#[cfg(test)] mod tests` already exists at the bottom of the file (lines 813–889) with an `exts(&[Extension::…])` helper built on `Extensions::from_list`; extend it rather than creating a second module.

Repo conventions that apply:

- No panics in shipped paths: no `unwrap`/`expect`/`xs[i]`; index with `.get()` and propagate. (Lint-enforced.)
- Provenance rule (`AGENTS.md`): never mention pandoc or "the reference implementation" in code/comments. Citing the **CommonMark spec** is fine and encouraged — it is a public format specification, vendored under `vendor/`.
- Comments only where the *why* is non-obvious.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Tests | `cargo nextest run --workspace` | all pass, zero snapshot diffs |
| Doctests | `cargo test --doc` | all pass |
| Lint | `cargo clippy --all-targets` | exit 0, no new warnings |
| Bench (regression) | `cargo bench -p carta -- emphasis_heavy` | large/small time ratio near the 3.2× size ratio (see Step 5) |
| Bench (regression) | `cargo bench -p carta -- pathological_brackets` | same |
| Conformance (only if `.oracle/` exists) | `tools/conformance-suite/run.sh reader commonmark` | `RESULT … fail=0 err=0` |

## Suggested executor toolkit

- Read the vendored CommonMark spec's appendix on inline parsing before starting: look under `vendor/` for the spec document and find the section describing `delimiter stack`, `openers_bottom`, and the bracket-stack handling of `[`/`![`. The algorithm there is the target shape.

## Scope

**In scope**:
- `crates/carta-readers/src/commonmark/inline.rs`
- `plans/README.md` (status row)

**Out of scope**:
- `crates/carta-readers/src/commonmark/scan.rs`, `block.rs`, `cursor.rs` — separate plans cover those.
- The `Node`/`Delimiter` data model may gain fields (e.g. links into a delimiter list) but the produced `Vec<Inline>` must be identical.
- Any `.snap` file — if snapshots change, that is a STOP condition, not something to `insta review` away.

## Git workflow

- Branch: `advisor/003-inline-delimiter-stack` off `main`.
- Conventional Commits, e.g. `perf(commonmark): resolve emphasis with a linear delimiter stack`.
- Stage explicit paths only. Do NOT push.

## Steps

### Step 1: Characterize current behavior with targeted unit tests

Before changing anything, add `#[cfg(test)]` unit tests at the bottom of `inline.rs` (a test module may already exist — extend it; unwrap is allowed in tests per `clippy.toml`) covering the tricky interaction cases so refactor regressions surface in-file, not only in snapshots:

- nested emphasis `*a **b** c*`, mixed `*a _b_ c*`
- rule-of-3 cases: `***a***`, `**a*b**`, `*a**b*`
- unmatched runs: `*a`, `a*`, `**a*`
- underscore flanking: `a_b_c`, `_a_b`
- brackets: `[a](u)`, `[a][r]`, `![i](u)`, nested `[[a]](u)`, unmatched `]]]`, link-suppresses-earlier-openers `[a [b](u) c](v)`
- emphasis inside link text `[*a*](u)`
- extension delimiters (pass the matching `Extensions`, reusing the existing `exts` helper):
  `~~a~~` with strikeout on; `~a~` with subscript on; `^a^` with superscript on;
  `~~a~~` with subscript on but strikeout off (becomes nested subscripts);
  `~a~~b~~` and `~~a~` with strikeout on but subscript off (length-1 runs have no mapping — the
  opener search must skip them and the leftovers stay literal);
  `*a ~~b~~ c*` with strikeout on (mixed kinds interleaved)

Drive them through the existing private API (`parse_inlines` with an empty/populated `RefMap` and an explicit `Extensions` argument — `Extensions::empty()` for the core cases) and assert the produced `Vec<Inline>`. Run them BEFORE the refactor to capture today's outputs as the expected values.

**Verify**: `cargo nextest run -p carta-readers` → all pass, including the new tests.

### Step 2: Replace the rescan loop with the spec's delimiter-stack algorithm

Rewrite `process_emphasis` to the linear algorithm described in the spec appendix. The essential shape:

1. Maintain delimiter entries with predecessor/successor structure (indices into `nodes` plus prev/next links, or a separate `Vec` of delimiter records pointing at node indices — pick one; avoid `unsafe`).
2. Walk closers left-to-right **once**; never reset to `stack_bottom` after a match (delete the `closer = stack_bottom;` behavior).
3. Track `openers_bottom` per closer class (the spec uses one slot per `(char, count mod 3, can_open)` combination) so failed opener searches are never repeated over the same range. **Bucketing invariant**: two closers may share a slot only if every opener would accept or reject them identically. With `match_use_count` in the eligibility test, that is no longer determined by `(char, count mod 3, can_open)` alone — for `~` with strikeout on and subscript off, an opener's acceptance also depends on whether *both* runs have length ≥ 2 (`match_use_count` returns `None` otherwise and the scan continues past that opener). Extend the slot key for `~` with the closer's `count >= 2` bit (or handle the `None` rejection without advancing `openers_bottom` for that slot). Get this wrong and a valid opener is skipped — the Step-1 tilde tests are the canary.
4. On a match, splice exactly as today (`drain` the inner span, `collapse`, `wrap_emphasis`, decrement counts, drop emptied delimiters) but update the delimiter links instead of restarting. Per the spec, delimiters strictly between opener and closer leave the delimiter structure when the span is wrapped (they were drained into the content).
5. Keep `emphasis_match` (rule of 3), `match_use_count`, `wrap_emphasis`, and `flanking` untouched.

Node removal via `Vec::remove`/`insert` (O(n) each) is acceptable to keep — the quadratic factor being eliminated is the rescans, and splices are bounded by the number of matches.

**Verify**: `cargo nextest run -p carta-readers` → all Step-1 tests still pass unchanged.

### Step 3: Make bracket-opener lookup O(1)

Replace `last_bracket_opener`'s full backward scan with a bracket stack on `InlineParser`: push the node index in `push_open_bracket`, pop in `close_bracket` (both on success and on the literalize path). `deactivate_earlier_brackets` can then walk only the remaining stack entries instead of all earlier nodes. Account for `build_link`'s `split_off(opener_index + 1)`: indices of stack entries above the split point are consumed with the link content — truncate the stack accordingly.

**Verify**: `cargo nextest run -p carta-readers` → all pass.

### Step 4: Full-suite and snapshot gate

**Verify**:
- `cargo nextest run --workspace` → all pass; `git status` shows **no modified `.snap` files** and no pending `.snap.new` files.
- `cargo clippy --all-targets` → exit 0, no new warnings.
- `cargo test --doc` → pass.

### Step 5: Prove the complexity win

Run `cargo bench -p carta -- emphasis_heavy` and `cargo bench -p carta -- pathological_brackets` **before the change (on the base commit) and after**. The adversarial generators run at `small` = 10 KiB and `large` = 32 KiB (`ADVERSARIAL_LARGE` in `crates/carta/benches/convert.rs` — deliberately small because the current resolver is quadratic; do not edit the bench file, it is out of scope). Two signals to record in the commit message body:

- **Scaling**: large/small time ratio per bench. Quadratic shows ~10× (3.2² ≈ 10); linear shows ~3.2×. Criterion's throughput lines make this easy — under linear scaling, MiB/s at `large` roughly matches `small` instead of dropping ~3×.
- **Absolute**: before→after time at `large` for both benches.

If the benches are missing from `crates/carta/benches/convert.rs`, STOP — plan 001 is this plan's regression harness.

If `.oracle/` and `jq` are present, also run `tools/conformance-suite/run.sh reader commonmark` → `fail=0 err=0`. If `.oracle/` is absent, note in the report that the conformance layer must run in CI before merge.

## Test plan

- New unit tests: the Step-1 characterization set in `inline.rs`'s test module (≥ 12 cases listed above), written against pre-refactor behavior.
- Existing layer-1 golden snapshots (`crates/carta/tests/snapshots/`) must be byte-identical — they are the contract.
- Verification: `cargo nextest run --workspace` → all pass, no snapshot churn.

## Done criteria

- [ ] `grep -n 'closer = stack_bottom;' crates/carta-readers/src/commonmark/inline.rs` returns no match inside the match-success path (a single initialization is fine)
- [ ] `cargo nextest run --workspace` exits 0
- [ ] `git status` shows no `.snap` or `.snap.new` changes
- [ ] `cargo clippy --all-targets` exits 0, no new warnings
- [ ] Bench comparison recorded: LARGE/SMALL time ratio for `emphasis_heavy` and `pathological_brackets` is ~linear
- [ ] No `unwrap`/`expect`/slice-indexing added outside `#[cfg(test)]`
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- Any golden snapshot changes — output parity is the project's correctness bar; a snapshot diff means the rewrite changed behavior. Do NOT run `cargo insta review` to accept it.
- A Step-1 characterization test fails after the rewrite and the fix isn't apparent within two attempts.
- The vendored spec's appendix can't be located under `vendor/` — the algorithm must come from the spec, not from memory of other implementations.
- Implementing the linked delimiter list seems to require `unsafe` or index arithmetic you can't express with `.get()` — report the design instead of bending the panic rules.

## Maintenance notes

- The `~`/`^` extension kinds landed before this plan executed and are covered by Step 2's bucketing invariant. Any *future* delimiter kind must re-derive its `openers_bottom` slot key from the same invariant (identical accept/reject behavior across the bucket) — note this in a comment at the slot-key definition.
- Reviewer focus: the rule-of-3 interplay with `openers_bottom` bucketing (the `count mod 3` dimension) is the classic place implementations diverge from the spec; check Step-1 tests `***a***`, `**a*b**`, and the tilde length-1 skip cases carefully.
- Deferred: `collapse`/splice still copies node vectors per link; harmless now, revisit only if benches say so.
