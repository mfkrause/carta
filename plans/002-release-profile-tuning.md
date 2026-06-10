# Plan 002: Tune the release profile for speed and binary size (LTO, codegen-units, strip)

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat f5d2e3b..HEAD -- Cargo.toml`
> If `Cargo.toml` changed since this plan was written, compare the
> "Current state" excerpt against the live file before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none (plan 001 makes the speed effect measurable; binary-size effect is measurable regardless)
- **Category**: perf
- **Planned at**: commit `5e110f9`, 2026-06-10 (reconciled at `f5d2e3b`, 2026-06-10 — profile unchanged; measurement notes refreshed after `tools/bench-suite/` landed)

## Why this matters

The README states two goals in its first bullet: performance and a smaller binary than pandoc. The release profile currently configures neither: no link-time optimization, default codegen-units (16), and no symbol stripping. For a CLI split across six workspace crates, fat LTO + `codegen-units = 1` enables cross-crate inlining of the hot reader/writer paths, and `strip` typically cuts a Rust binary's size substantially. This is a pure-config change.

## Current state

Root `Cargo.toml`, the entire current release profile (with its intentional comment):

```toml
# Overflow wraps silently in release by default. Readers do arithmetic on byte offsets, positions,
# and counts; a silent wrap is a latent correctness bug. The check is cheap insurance for a
# correctness-critical parser.
[profile.release]
overflow-checks = true
```

`overflow-checks = true` is a deliberate correctness decision (see the comment) — keep it.

The release binary builds with `cargo build --release -p carta-cli` and lands at `target/release/carta`. (Note: the binary crate is `carta-cli`; `-p carta` builds only the library.)

`docs/BENCHMARKS.md` records reference numbers (binary 2.6 MB, the speed/RSS tables) measured at commit `5e110f9` with this untuned profile — they are the committed before-picture for this change. If `.oracle/`, `hyperfine`, and `jq` are available, `tools/bench-suite/run.sh size` (binary size, no timing) and `tools/bench-suite/run.sh e2e` give ready-made before/after measurements; `ls -l` suffices otherwise.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Release build | `cargo build --release -p carta-cli` | exit 0 |
| Binary size | `ls -l target/release/carta` | prints size |
| Smoke test | `echo '# Hi' \| ./target/release/carta -f commonmark -t html` | prints `<h1>Hi</h1>` (strict CommonMark generates no heading identifiers; the reader snapshot `golden_reader__commonmark__headings.snap` pins the empty attr) |
| Tests | `cargo nextest run --workspace` | all pass (unaffected; runs in dev profile) |
| Benches (if plan 001 landed) | `cargo bench -p carta` | exit 0 |

## Scope

**In scope**:
- `Cargo.toml` (workspace root) — `[profile.release]` section only.
- `plans/README.md` (status row)

**Out of scope**:
- `overflow-checks = true` — keep it; it is a documented correctness choice.
- `panic = "abort"` — would shrink the binary further but changes unwinding behavior for library consumers and test tooling; deferred (see Maintenance notes).
- `[profile.dev.package."*"]` — the existing dev-dependency optimization stays as is.
- Any source file.

## Git workflow

- Branch: `advisor/002-release-profile` off `main`.
- One commit, Conventional Commits: `perf(build): enable lto, codegen-units=1, and strip in release`.
- Stage `Cargo.toml` explicitly. Do NOT push.

## Steps

### Step 1: Record the baseline

```sh
cargo build --release -p carta-cli && ls -l target/release/carta
```

Note the byte size. If plan 001 has landed, also run `cargo bench -p carta` and keep the summary for comparison.

**Verify**: build exits 0; size recorded.

### Step 2: Extend the release profile

Edit `[profile.release]` in root `Cargo.toml` to:

```toml
[profile.release]
overflow-checks = true
lto = true
codegen-units = 1
strip = "symbols"
```

Keep the existing comment above `overflow-checks` untouched.

**Verify**: `cargo build --release -p carta-cli` → exit 0 (expect a noticeably longer link step; that is normal for fat LTO).

### Step 3: Compare and smoke-test

```sh
ls -l target/release/carta
echo '# Hi' | ./target/release/carta -f commonmark -t html
```

Record old vs. new binary size in the commit message body. If plan 001 landed, re-run `cargo bench -p carta` — note that criterion benches run in the `bench` profile which inherits `release`, so improvements should show there too.

**Verify**: smoke test prints `<h1>Hi</h1>` (strict CommonMark generates no heading identifiers; the reader snapshot `golden_reader__commonmark__headings.snap` pins the empty attr); new size ≤ old size.

## Test plan

No new tests — config only. Gate on the existing suite:

- `cargo nextest run --workspace` → all pass.
- Release smoke test above.

## Done criteria

- [ ] `cargo build --release -p carta-cli` exits 0
- [ ] `echo '# Hi' | ./target/release/carta -f commonmark -t html` prints `<h1>Hi</h1>` (strict CommonMark generates no heading identifiers; the reader snapshot `golden_reader__commonmark__headings.snap` pins the empty attr)
- [ ] `cargo nextest run --workspace` exits 0
- [ ] Binary size before/after recorded in the commit message
- [ ] `overflow-checks = true` still present in `[profile.release]`
- [ ] Only `Cargo.toml` and `plans/README.md` modified (`git status`)

## STOP conditions

Stop and report back (do not improvise) if:

- Fat LTO fails to link on the pinned toolchain — report the error; do not silently fall back to `lto = "thin"` without recording the failure (thin LTO IS the sanctioned fallback, but the operator should know why).
- The new binary is larger than the baseline.
- Any existing test fails.

## Maintenance notes

- `panic = "abort"` in release is a follow-up candidate worth a measured decision: it shrinks the binary and speeds up code paths, but removes unwinding (relevant if the library is ever embedded by consumers that catch panics). Decide after the binary-size goal is quantified against a pandoc baseline.
- After this lands, `docs/BENCHMARKS.md` describes the old profile. Per `tools/bench-suite/README.md`, regenerating it is a deliberate manual act for the operator — flag it in the report; do not regenerate it yourself.
- If incremental release-build times become painful during development, that is the expected cost of `codegen-units = 1`; dev builds are unaffected.
