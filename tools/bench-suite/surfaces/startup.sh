#!/usr/bin/env bash
# Startup surface: time a near-empty conversion to isolate process spin-up (pandoc's runtime vs
# carta's). This is the explicit startup figure that keeps the small-input comparison honest — the
# bulk of any small-input gap is this, not parsing.
# Usage: surfaces/startup.sh [from:to]   (no arg = curated default pairs)
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

PAIRS="commonmark:html commonmark:json"
[ $# -gt 0 ] && PAIRS="$1"

for pair in $PAIRS; do
  from="${pair%%:*}"
  to="${pair##*:}"
  table_header "startup — $from → $to (near-empty input)"
  norm="$(oracle_norm "$to")"
  input="$FIXTURES/startup.md"
  [ "$from" = json ] && input="$FIXTURES/startup.ast.json"
  [ -s "$input" ] || continue
  bench_pair "startup/$from-$to" "$input" "$(file_bytes "$input")" \
    "-f $from -t $to $norm" "-f $from -t $to"
done

exit "$SUITE_RC"
