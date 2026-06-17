#!/usr/bin/env bash
# Writer surface: render the AST-JSON corpus to each target and diff against pandoc.
#
# For each `corpus/ast/<feature>/*.json` not excluded for the target, compare
# `carta -f json -t <target>` against `pandoc -f json -t <target>` (with oracle normalization).
# JSON targets compare structurally; every other target compares as text. Excluded
# (target, feature) pairs from exclusions.tsv are skipped and counted.
#
# Usage: surfaces/writer.sh [target]   (no arg = every writer target)
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require_tools

TARGETS="html latex rst plain commonmark mediawiki native json typst dokuwiki jira asciidoc man"
[ $# -gt 0 ] && TARGETS="$1"

for target in $TARGETS; do
  conf_reset "writer-$target"
  norm="$(oracle_norm "$target")"
  is_json_target "$target" && mode=json || mode=text
  for input in "$CORPUS"/ast/*/*.json; do
    [ -f "$input" ] || continue
    feature="$(basename "$(dirname "$input")")"
    if is_excluded "$target" "$feature" "$(basename "$input" .json)"; then
      SKIP=$((SKIP + 1))
      continue
    fi
    run_diff "$mode" "writer/$target/$feature/$(basename "$input")" "$input" \
      "-f json -t $target $norm" "-f json -t $target"
  done
  report writer "$target"
  tally_group
done

exit "$SUITE_RC"
