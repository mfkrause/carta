#!/usr/bin/env bash
# Time near-empty conversions to measure startup. Usage: surfaces/startup.sh [from:to]
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

PAIRS="commonmark:html commonmark:json"
[ $# -gt 0 ] && PAIRS="$1"

for pair in $PAIRS; do
  from="${pair%%:*}"
  to="${pair##*:}"
  table_header startup "$from → $to (near-empty input)"
  norm="$(oracle_norm "$to")"
  input="$FIXTURES/startup.md"
  [ "$from" = json ] && input="$FIXTURES/startup.ast.json"
  [ -s "$input" ] || continue
  bench_pair "startup/$from-$to" "$input" "$(file_bytes "$input")" \
    "-f $from -t $to $norm" "-f $from -t $to"
done

exit "$SUITE_RC"
