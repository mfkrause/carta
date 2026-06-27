#!/usr/bin/env bash
# Writer surface: render the AST-JSON corpus to each target and diff against pandoc.
#
# For each `corpus/ast/<feature>/*.json` not excluded for the target, compare
# `carta -f json -t <target>` against `pandoc -f json -t <target>` (with oracle normalization).
# JSON targets compare structurally; every other target compares as text. Excluded
# (target, feature) pairs from exclusions.tsv are skipped and counted. On a full run (no target
# argument) the extension-toggle group below also runs.
#
# Usage: surfaces/writer.sh [target]   (no arg = every writer target)
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require_tools

TARGETS="html html4 latex rst plain commonmark commonmark_x markdown markdown_github markdown_phpextra markdown_mmd markdown_strict gfm mediawiki native json typst dokuwiki jira asciidoc man opml beamer revealjs"
EXT_RUN=1
[ $# -gt 0 ] && { TARGETS="$1"; EXT_RUN=0; }

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

# Extension-toggle cases: each `corpus/ast-ext/<spec>/*.json` directory is named for the full target
# format spec (e.g. `markdown-fenced_divs`, `latex-smart`) it is rendered with, for both carta and
# the oracle. The spec's base (the name before the first `+`/`-`) selects the oracle normalization
# and the comparison mode. This is the writer-side counterpart to the reader-ext loop; it runs on a
# full sweep only.
if [ "$EXT_RUN" = 1 ] && [ -d "$CORPUS/ast-ext" ]; then
  conf_reset "writer-ext"
  for spec_dir in "$CORPUS"/ast-ext/*; do
    [ -d "$spec_dir" ] || continue
    spec="$(basename "$spec_dir")"
    base="${spec%%[+-]*}"
    norm="$(oracle_norm "$base")"
    is_json_target "$base" && mode=json || mode=text
    for input in "$spec_dir"/*.json; do
      [ -f "$input" ] || continue
      run_diff "$mode" "writer-ext/$spec/$(basename "$input")" "$input" \
        "-f json -t $spec $norm" "-f json -t $spec"
    done
  done
  report writer ext
  tally_group
fi

exit "$SUITE_RC"
