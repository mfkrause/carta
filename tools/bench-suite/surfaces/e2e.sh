#!/usr/bin/env bash
# Time end-to-end conversions. Usage: surfaces/e2e.sh [from:to]
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

PAIRS="commonmark:html commonmark:latex commonmark:rst commonmark:json"
[ $# -gt 0 ] && PAIRS="$1"

for pair in $PAIRS; do
  from="${pair%%:*}"
  to="${pair##*:}"
  table_header e2e "$from → $to"
  for size in $(sizes_list); do
    input="$(fixture_for "$from" "$size")"
    [ -s "$input" ] || continue
    bench_pair "e2e/$from-$to/$size" "$input" "$(file_bytes "$input")" \
      "-f $from -t $to" "-f $from -t $to"
  done
done

exit "$SUITE_RC"
