#!/usr/bin/env bash
# Local CI gate: run the fast, offline checks that CI gates a pull request on, in one command.
#
# Mirrors the `lint`, `test`, and `typos`/`deny` CI jobs so a warning-only change surfaces here
# instead of after a push. Uses RUSTFLAGS="-D warnings" (as CI does) so warnings fail the build.
#
# NOT included here (slow or oracle/network-dependent): CI additionally runs the conformance suite
# (tools/conformance-suite, needs .oracle/), coverage (cargo llvm-cov), and the minimal-versions
# build (cargo hack). Run those separately when relevant.
#
# Usage: tools/check.sh
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

export RUSTFLAGS="${RUSTFLAGS:--D warnings}"

echo "==> fmt" >&2
cargo fmt --all --check

echo "==> clippy" >&2
cargo clippy --all-targets --all-features

echo "==> tests" >&2
if command -v cargo-insta >/dev/null 2>&1; then
  cargo insta test --workspace --all-features --test-runner nextest --unreferenced=reject
else
  echo "note: cargo-insta not installed — running nextest without the orphan-snapshot check" >&2
  echo "      (cargo install cargo-insta --locked)" >&2
  cargo nextest run --workspace --all-features
fi

echo "==> doctests" >&2
cargo test --doc --workspace --all-features

echo "==> docs" >&2
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace --lib --all-features

echo "==> typos" >&2
if command -v typos >/dev/null 2>&1; then
  typos
else
  echo "skip: typos not installed (cargo install typos-cli)" >&2
fi

echo "==> deny" >&2
if command -v cargo-deny >/dev/null 2>&1; then
  cargo deny check
else
  echo "skip: cargo-deny not installed (cargo install cargo-deny --locked)" >&2
fi

echo "✓ all checks passed" >&2
