#!/usr/bin/env bash
# Writer surface: time `json -> <target>` rendering for carta vs pandoc, per size. Input is the
# curated AST (corpus/ast subset cycled to size); pandoc is normalized so both do equivalent work.
# Usage: surfaces/writer.sh [target]   (no arg = every writer target)
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

TARGETS="html latex rst plain commonmark mediawiki native json"
[ $# -gt 0 ] && TARGETS="$1"

for target in $TARGETS; do
  table_header "writer — json → $target"
  norm="$(oracle_norm "$target")"
  for size in $(sizes_list); do
    input="$FIXTURES/ast.$size.json"
    [ -s "$input" ] || continue
    bench_pair "writer/$target/$size" "$input" "$(file_bytes "$input")" \
      "-f json -t $target $norm" "-f json -t $target"
  done
done

exit "$SUITE_RC"
