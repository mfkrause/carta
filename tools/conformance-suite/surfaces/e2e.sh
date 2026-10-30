#!/usr/bin/env bash
# Compare full conversions and CommonMark HTML. Usage: surfaces/e2e.sh [format]
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
      media=""
      if { [ "$fmt" = ipynb ] || [ "$fmt" = rtf ]; } && [ "$target" = latex ]; then
        media="--extract-media=$WORK/e2e-media/$label"
      fi
      is_json_target "$target" && mode=json || mode=text
      run_diff "$mode" "e2e/$fmt->$target/$label" "$input" \
        "-f $fmt -t $target --syntax-highlighting=none $media" "-f $fmt -t $target --syntax-highlighting=none $media"
    done
  done
  report e2e "$fmt"
  tally_group
done

if echo "$FORMATS" | grep -qw commonmark; then
  specdir="$WORK/spec"
  extract_spec "$specdir"
  conf_reset "e2e-commonmark-html"
  for input in "$specdir"/*.md; do
    [ -f "$input" ] || continue
    run_diff text "e2e/commonmark-spec->html/$(basename "$input")" "$input" \
      "-f commonmark -t html --syntax-highlighting=none" "-f commonmark -t html --syntax-highlighting=none"
  done
  report e2e commonmark-spec-html
  tally_group
fi

exit "$SUITE_RC"
