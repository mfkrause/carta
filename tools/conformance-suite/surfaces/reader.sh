#!/usr/bin/env bash
# Reader surface: parse text into the JSON AST and diff against pandoc.
#
# For each `corpus/text/<fmt>/*` case, compare `carta -f <fmt> -t json` against
# `pandoc -f <fmt> -t json` (jq -S). For commonmark, additionally run every worked example from the
# CommonMark spec as a dedicated group so the spec-parity count stays visible.
#
# Usage: surfaces/reader.sh [format]   (no arg = every reader format)
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require_tools

FORMATS="commonmark html native json"
[ $# -gt 0 ] && FORMATS="$1"

for fmt in $FORMATS; do
  dir="$CORPUS/text/$fmt"
  [ -d "$dir" ] || continue
  conf_reset "reader-$fmt"
  for input in "$dir"/*; do
    [ -f "$input" ] || continue
    run_diff json "reader/$fmt/$(basename "$input")" "$input" "-f $fmt -t json" "-f $fmt -t json"
  done
  report reader "$fmt"
  tally_group
done

# CommonMark spec examples — exhaustive reader parity, reported on its own line.
if echo "$FORMATS" | grep -qw commonmark; then
  specdir="$WORK/spec"
  extract_spec "$specdir"
  conf_reset "reader-commonmark-spec"
  for input in "$specdir"/*.md; do
    [ -f "$input" ] || continue
    run_diff json "reader/commonmark-spec/$(basename "$input")" "$input" "-f commonmark -t json" "-f commonmark -t json"
  done
  report reader commonmark-spec
  tally_group
fi

exit "$SUITE_RC"
