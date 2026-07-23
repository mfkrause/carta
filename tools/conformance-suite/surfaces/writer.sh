#!/usr/bin/env bash
# Writer surface: render corpus/ast through carta and pandoc for each target and diff (json
# structurally, notebooks after canonicalizing cell ids); exclusions.tsv pairs are skipped and counted.
# Usage: surfaces/writer.sh [target] [case]   (no arg = every target + extension-toggle group)
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require_tools

TARGETS="${1:-$(shared_output_formats)}"
EXT_RUN=1
[ $# -gt 0 ] && EXT_RUN=0
CASE="${2:-}"

for target in $TARGETS; do
  conf_reset "writer-$target"
  norm="$(oracle_norm "$target")"
  mode="$(compare_mode "$target")"
  for input in "$CORPUS"/ast/*/*.json; do
    [ -f "$input" ] || continue
    [ -n "$CASE" ] && [ "$(basename "$input" .json)" != "$CASE" ] && continue
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

# Each corpus/ast-ext/<spec>/ dir is named for the full target spec it renders with; the base
# (before the first +/-) selects normalization and compare mode. Full sweep only.
if [ "$EXT_RUN" = 1 ] && [ -d "$CORPUS/ast-ext" ]; then
  conf_reset "writer-ext"
  for spec_dir in "$CORPUS"/ast-ext/*; do
    [ -d "$spec_dir" ] || continue
    spec="$(basename "$spec_dir")"
    base="${spec%%[+-]*}"
    # Binary package targets have no text form to diff and carry their own surfaces.
    case "$base" in docx | epub | epub2 | epub3 | odt) continue ;; esac
    norm="$(oracle_norm "$base")"
    mode="$(compare_mode "$base")"
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
