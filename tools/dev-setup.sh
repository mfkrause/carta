#!/usr/bin/env bash
# One-time local dev setup: point git at the committed hooks and report any missing tooling.
#
# Usage: tools/dev-setup.sh
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

git config core.hooksPath .githooks
echo "✓ git hooks enabled (core.hooksPath = .githooks)"

missing=0
check() { # check <command> <install hint>
  if command -v "$1" >/dev/null 2>&1; then
    echo "✓ $1"
  else
    echo "✗ $1 — $2"
    missing=1
  fi
}

check cargo "install Rust via https://rustup.rs"
check cargo-nextest "cargo install cargo-nextest --locked"
check cargo-insta "cargo install cargo-insta --locked"
check cargo-llvm-cov "cargo install cargo-llvm-cov --locked"
check typos "cargo install typos-cli --locked"
check cargo-deny "cargo install cargo-deny --locked"
check jq "install jq via your package manager (e.g. brew install jq) — used by tools/conformance-suite"
check hyperfine "brew install hyperfine (or cargo install hyperfine) — used by tools/bench-suite"

if [ ! -x "$repo_root/.oracle/bin/pandoc" ]; then
  echo "• oracle binary not installed — run tools/install-pandoc.sh"
fi
if [ ! -d "$repo_root/.oracle/tests" ]; then
  echo "• oracle test corpus not fetched — run tools/fetch-pandoc-tests.sh"
fi

exit "$missing"
