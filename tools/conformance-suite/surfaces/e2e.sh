#!/usr/bin/env bash
# End-to-end surface: each `corpus/text/<fmt>/*` case converts to every curated target and diffs
# vs pandoc; the 652 CommonMark spec examples additionally drive HTML as a dedicated group.
#
# Usage: surfaces/e2e.sh [format]   (no arg = every reader format)
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require_tools

FORMATS="${1:-$(shared_input_formats)}"
# Curated subset: the full cross-product is expensive; exhaustive coverage belongs to the writer surface.
TARGETS="html latex rst plain commonmark mediawiki native json"

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

# CommonMark spec examples to HTML: full-pipeline parity, reported on its own line.
if echo "$FORMATS" | grep -qw commonmark; then
  specdir="$WORK/spec"
  extract_spec "$specdir"
  conf_reset "e2e-commonmark-html"
  for input in "$specdir"/*.md; do
    [ -f "$input" ] || continue
    run_diff text "e2e/commonmark-spec->html/$(basename "$input")" "$input" \
      "-f commonmark -t html --syntax-highlighting=none --mathjax" "-f commonmark -t html --syntax-highlighting=none"
  done
  report e2e commonmark-spec-html
  tally_group
fi

exit "$SUITE_RC"
