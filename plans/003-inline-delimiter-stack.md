# Plan 003: Make CommonMark inline emphasis/bracket resolution linear instead of quadratic

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 5e110f9..HEAD -- crates/carta-readers/src/commonmark/inline.rs`
> If the file changed since this plan was written, compare the "Current state"
> excerpts against the live code before proceeding; on a mismatch, treat it as
> a STOP condition.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: MED
- **Depends on**: plans/001-criterion-bench-suite.md (provides `emphasis_heavy` and `pathological_brackets` regression benches)
- **Category**: perf
- **Planned at**: commit `5e110f9`, 2026-06-10

## Why this matters

The inline parser's emphasis resolution restarts its scan from the bottom of the node list after **every** successful emphasis match, and searches backward through all preceding nodes for each closer. A paragraph with k emphasis pairs therefore does O(k) full rescans — quadratic work on completely ordinary emphasis-heavy documents, not just adversarial ones. Separately, every `]` triggers a backward scan over the entire node list to find a bracket opener, which is quadratic on `]`-heavy input. A document converter ingests arbitrary input, so a quadratic blowup is also a denial-of-service vector. The CommonMark specification's appendix ("A parsing strategy", in the vendored spec) describes the standard linear-time delimiter-stack algorithm; this plan adopts it.

## Current state

All in `crates/carta-readers/src/commonmark/inline.rs`:

- `parse_inlines` (line 69) builds a flat `Vec<Node>` where `Node::Delimiter` entries represent `*`/`_` runs and `[`/`![` openers, then calls `process_emphasis(&mut nodes, 0)`.
- `process_emphasis` (lines 407–488). The three performance problems, as the code stands today:

  ```rust
  // line 408-409: forward scan for a closer
  let mut closer = stack_bottom;
  while closer < nodes.len() {
      ...
      // lines 422-434: backward scan for the opener — O(n) per closer
      let mut opener = None;
      let mut index = closer;
      while index > stack_bottom {
          index -= 1;
          if let Some(Node::Delimiter(d)) = nodes.get(index)
              && d.can_open && d.ch == closer_ch
              && emphasis_match(d, nodes, closer)
          { opener = Some(index); break; }
      }
      ...
      // line 480: after every successful match, restart from the bottom
      closer = stack_bottom;
  }
  ```

- `last_bracket_opener` (lines 325–331) — full backward scan of `nodes` on every `]`:

  ```rust
  fn last_bracket_opener(&self) -> Option<usize> {
      self.nodes.iter().enumerate().rev()
          .find_map(|(i, node)| matches!(node, Node::Delimiter(d) if d.ch == b'[').then_some(i))
  }
  ```

- `process_emphasis` is called from two places: `parse_inlines` (line 79) and `build_link` (line 388, on the link's inner nodes with `stack_bottom = 0` after `split_off`).
- The match-eligibility rules live in `flanking` (line 592) and `emphasis_match` (line 490, the "rule of 3"); `use_count` selection (strong vs. emph, lines 446–454) and the node-splicing logic (lines 456–478) are behaviorally correct today — the 652 vendored spec examples pass byte-identically. **Behavior must not change; only complexity.**

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
| Bench (regression) | `cargo bench -p carta -- emphasis_heavy` | reported time scales ~linearly between SMALL and LARGE |
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

Drive them through the existing private API (`parse_inlines` with an empty/populated `RefMap`) and assert the produced `Vec<Inline>`. Run them BEFORE the refactor to capture today's outputs as the expected values.

**Verify**: `cargo nextest run -p carta-readers` → all pass, including the new tests.

### Step 2: Replace the rescan loop with the spec's delimiter-stack algorithm

Rewrite `process_emphasis` to the linear algorithm described in the spec appendix. The essential shape:

1. Maintain delimiter entries with predecessor/successor structure (indices into `nodes` plus prev/next links, or a separate `Vec` of delimiter records pointing at node indices — pick one; avoid `unsafe`).
2. Walk closers left-to-right **once**; never reset to `stack_bottom` after a match (delete the `closer = stack_bottom;` behavior).
3. Track `openers_bottom` per delimiter kind (the spec uses one slot per `(char, count mod 3, can_open)` combination) so failed opener searches are never repeated over the same range.
4. On a match, splice exactly as today (`Strong` for 2, `Emph` for 1, decrement counts, drop emptied delimiters) but update the delimiter links instead of restarting.
5. Keep `emphasis_match` (rule of 3) and `flanking` untouched.

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

Run `cargo bench -p carta -- emphasis_heavy` and `cargo bench -p carta -- pathological_brackets`. Compare SMALL (10 KiB) vs LARGE (1 MiB): time should scale roughly ×100 (linear), not ×10000 (quadratic). Record before/after numbers in the commit message body. If plan 001 has not landed, STOP — its benches are this plan's regression harness.

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

- Extension work (strikethrough `~~`, etc.) will add new delimiter kinds; the `openers_bottom` table must be extended per kind — note this in a comment at its definition.
- Reviewer focus: the rule-of-3 interplay with `openers_bottom` bucketing (the `count mod 3` dimension) is the classic place implementations diverge from the spec; check Step-1 tests `***a***`, `**a*b**` carefully.
- Deferred: `collapse`/splice still copies node vectors per link; harmless now, revisit only if benches say so.
