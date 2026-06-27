#!/usr/bin/env bash
# End-to-end surface: convert text straight to a target through the full pipeline and diff vs pandoc.
#
# Each `corpus/text/<fmt>/*` case is converted to every writer target and compared. The 652
# CommonMark spec examples are additionally driven to HTML as a dedicated group (the spec-parity
# count for the full pipeline).
#
# Usage: surfaces/e2e.sh [format]   (no arg = every reader format)
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require_tools

FORMATS="commonmark html native json rst ipynb mediawiki dokuwiki jira man"
TARGETS="html latex rst plain commonmark mediawiki native json"
[ $# -gt 0 ] && FORMATS="$1"

for fmt in $FORMATS; do
  dir="$CORPUS/text/$fmt"
  [ -d "$dir" ] || continue
  conf_reset "e2e-$fmt"
  for input in "$dir"/*; do
    [ -f "$input" ] || continue
    label="$(basename "$input")"
    for target in $TARGETS; do
      norm="$(oracle_norm "$target")"
      is_json_target "$target" && mode=json || mode=text
      run_diff "$mode" "e2e/$fmt->$target/$label" "$input" \
        "-f $fmt -t $target $norm" "-f $fmt -t $target"
    done
  done
  report e2e "$fmt"
  tally_group
done

# CommonMark spec examples → HTML — full-pipeline parity, reported on its own line.
if echo "$FORMATS" | grep -qw commonmark; then
  specdir="$WORK/spec"
  extract_spec "$specdir"
  conf_reset "e2e-commonmark-html"
  for input in "$specdir"/*.md; do
    [ -f "$input" ] || continue
    run_diff text "e2e/commonmark-spec->html/$(basename "$input")" "$input" \
      "-f commonmark -t html --syntax-highlighting=none --mathjax" "-f commonmark -t html"
  done
  report e2e commonmark-spec-html
  tally_group
fi

exit "$SUITE_RC"
