#!/usr/bin/env bash
# Reader surface: parse text into the JSON AST and diff against pandoc.
#
# For each `corpus/text/<fmt>/*` case, compare `carta -f <fmt> -t json` against
# `pandoc -f <fmt> -t json` (jq -S). For commonmark, additionally run every worked example from the
# CommonMark spec as a dedicated group so the spec-parity count stays visible.
#
# Usage: surfaces/reader.sh [format] [case]   (no arg = every reader format; case = one corpus stem)
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require_tools

FORMATS="${1:-$(shared_input_formats)}"
CASE="${2:-}"

# The case-stem of a corpus file: its basename with the extension stripped.
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

# Byte-container reader formats keep their fixtures under corpus/binary/<fmt>/ (binary archives that
# are not UTF-8 text, e.g. zipped office/e-book containers, or any file carrying raw high bytes). Each
# is diffed exactly like a text reader — carta and pandoc both read the file by path — just discovered
# from the binary tree, so a byte-container reader needs no entry in the FORMATS list above.
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

# Extension-toggle cases: each `corpus/text-ext/<spec>/*` directory is named for the full format
# spec (e.g. `commonmark+strikeout`, `markdown+citations`) it should be parsed with, for both carta
# and pandoc. The specs build on the markdown-family readers, which the commonmark group compiles, so
# they run alongside it.
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

# CommonMark spec examples — exhaustive reader parity, reported on its own line.
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
