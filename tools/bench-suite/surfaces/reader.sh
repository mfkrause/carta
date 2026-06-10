#!/usr/bin/env bash
# Reader surface: time `<fmt> -> json` parsing for carta vs pandoc, per size.
# Usage: surfaces/reader.sh [format]   (no arg = curated default formats)
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

FORMATS="commonmark html"
[ $# -gt 0 ] && FORMATS="$1"

for fmt in $FORMATS; do
  group_reset
  table_header "reader — $fmt → json"
  for size in $(sizes_list); do
    input="$(fixture_for "$fmt" "$size")"
    [ -s "$input" ] || continue
    bench_pair "reader/$fmt/$size" "$input" "$(file_bytes "$input")" \
      "-f $fmt -t json" "-f $fmt -t json"
  done
  tally_group
done

exit "$SUITE_RC"
