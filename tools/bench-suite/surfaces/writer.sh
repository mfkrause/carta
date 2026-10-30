#!/usr/bin/env bash
# Time curated AST rendering. Usage: surfaces/writer.sh [target]
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

TARGETS="html latex rst plain commonmark mediawiki native json"
[ $# -gt 0 ] && TARGETS="$1"

for target in $TARGETS; do
  table_header writer "json → $target"
  for size in $(sizes_list); do
    input="$FIXTURES/ast.$size.json"
    [ -s "$input" ] || continue
    bench_pair "writer/$target/$size" "$input" "$(file_bytes "$input")" \
      "-f json -t $target" "-f json -t $target"
  done
done

exit "$SUITE_RC"
