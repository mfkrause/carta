#!/usr/bin/env bash
# Templates surface: byte-exact differential of the standalone template engine.
#
# Each `corpus/templates/<case>/` holds a self-contained, carta-authored template (`doc.<ext>`, with
# any partials beside it sharing that extension), an `input.md` body/metadata document, and an
# optional `flags` file of extra CLI arguments (`-V`/`-M`/`--metadata-file`, with `@CASE@` standing
# in for the case directory). Because the template is an input we own, feeding the *same* template to
# carta and the oracle is a legitimate clean-room byte-for-byte differential — it pins the engine,
# pipes, whitespace rules, metadata-through-writer rendering, and precedence without copying anything.
#
# For each case we render through both binaries across targets with diverse escaping and inline
# rendering, comparing every byte (trailing newlines included — the template output is verbatim).
# A case may list targets it cannot yet reach in a `skip-targets` file (one target per line); those
# pairs are skipped and counted so coverage stays honest.
#
# Usage: surfaces/templates.sh [target]   (no arg = every target)
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
