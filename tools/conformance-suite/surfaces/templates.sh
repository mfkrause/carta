#!/usr/bin/env bash
# Templates surface: render each carta-authored corpus/templates/<case>/ (doc.<ext>, input.md,
# optional flags and skip-targets) through both tools, comparing every byte. Usage: surfaces/templates.sh [target]
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require_tools

TARGETS="html latex plain rst markdown gfm asciidoc mediawiki rtf"
[ $# -gt 0 ] && TARGETS="$1"

tdir="$CORPUS/templates"
if [ ! -d "$tdir" ]; then
  echo "RESULT templates all pass=0 fail=0 err=0 skip=0"
  exit 0
fi

for target in $TARGETS; do
  conf_reset "templates-$target"
  for case in "$tdir"/*/; do
    [ -d "$case" ] || continue
    name="$(basename "$case")"
    case_abs="$(cd "$case" && pwd)"
    # The entry template is the lone `doc.<ext>` file; its extension drives partial resolution.
    tmpl=""
    for candidate in "$case"doc.*; do
      [ -f "$candidate" ] && tmpl="$candidate" && break
    done
    [ -n "$tmpl" ] || continue
    if [ -f "$case/skip-targets" ] && grep -qx "$target" "$case/skip-targets"; then
      SKIP=$((SKIP + 1))
      continue
    fi
    input="$case/input.md"
    [ -f "$input" ] || input=/dev/null
    flags=""
    if [ -f "$case/flags" ]; then
      flags="$(cat "$case/flags")"
      flags="${flags//@CASE@/$case_abs}"
    fi
    args="-f markdown -t $target --template=$tmpl $flags"
    run_diff bytes "templates/$target/$name" "$input" "$args" "$args"
  done
  report templates "$target"
  tally_group
done

exit "$SUITE_RC"
