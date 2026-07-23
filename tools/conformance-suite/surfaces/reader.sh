#!/usr/bin/env bash
# Reader surface: diff carta and pandoc JSON ASTs over the corpus, plus every CommonMark spec example.
# Usage: surfaces/reader.sh [format] [case]   (no arg = every reader format; case = one corpus stem)
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require_tools

FORMATS="${1:-$(shared_input_formats)}"
CASE="${2:-}"

stem() { local b; b="$(basename "$1")"; echo "${b%.*}"; }

for fmt in $FORMATS; do
  dir="$CORPUS/text/$fmt"
  [ -d "$dir" ] || continue
  conf_reset "reader-$fmt"
  for input in "$dir"/*; do
    [ -f "$input" ] || continue
    [ -n "$CASE" ] && [ "$(stem "$input")" != "$CASE" ] && continue
    run_diff json "reader/$fmt/$(basename "$input")" "$input" "-f $fmt -t json" "-f $fmt -t json"
  done
  report reader "$fmt"
  tally_group
done

# Byte-container formats keep fixtures under corpus/binary/<fmt>/ (non-UTF-8 archives); diffed
# like text readers but discovered from the binary tree, so they need no FORMATS entry.
if [ -d "$CORPUS/binary" ]; then
  for fmt_dir in "$CORPUS"/binary/*; do
    [ -d "$fmt_dir" ] || continue
    fmt="$(basename "$fmt_dir")"
    [ $# -gt 0 ] && [ "$1" != "$fmt" ] && continue
    conf_reset "reader-$fmt"
    for input in "$fmt_dir"/*; do
      [ -f "$input" ] || continue
      [ -n "$CASE" ] && [ "$(stem "$input")" != "$CASE" ] && continue
      run_diff json "reader/$fmt/$(basename "$input")" "$input" "-f $fmt -t json" "-f $fmt -t json"
    done
    report reader "$fmt"
    tally_group
  done
fi

# Each corpus/text-ext/<spec>/ dir is named for the format spec both tools parse it with; the
# specs build on the markdown-family readers, so they run with the commonmark group.
if echo "$FORMATS" | grep -qw commonmark && [ -d "$CORPUS/text-ext" ]; then
  conf_reset "reader-ext"
  for spec_dir in "$CORPUS"/text-ext/*; do
    [ -d "$spec_dir" ] || continue
    spec="$(basename "$spec_dir")"
    for input in "$spec_dir"/*; do
      [ -f "$input" ] || continue
      [ -n "$CASE" ] && [ "$(stem "$input")" != "$CASE" ] && continue
      run_diff json "reader-ext/$spec/$(basename "$input")" "$input" "-f $spec -t json" "-f $spec -t json"
    done
  done
  report reader ext
  tally_group
fi

# CommonMark spec examples: exhaustive reader parity, reported on its own line.
if echo "$FORMATS" | grep -qw commonmark; then
  specdir="$WORK/spec"
  extract_spec "$specdir"
  conf_reset "reader-commonmark-spec"
  for input in "$specdir"/*.md; do
    [ -f "$input" ] || continue
    [ -n "$CASE" ] && [ "$(stem "$input")" != "$CASE" ] && continue
    run_diff json "reader/commonmark-spec/$(basename "$input")" "$input" "-f commonmark -t json" "-f commonmark -t json"
  done
  report reader commonmark-spec
  tally_group
fi

exit "$SUITE_RC"
