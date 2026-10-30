#!/usr/bin/env bash
# Compare AST rendering, applying exclusions. Usage: surfaces/writer.sh [target] [case]
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require_tools

TARGETS="${1:-$(shared_output_formats)}"
EXT_RUN=1
[ $# -gt 0 ] && EXT_RUN=0
CASE="${2:-}"

for target in $TARGETS; do
  conf_reset "writer-$target"
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
      "-f json -t $target --syntax-highlighting=none" "-f json -t $target --syntax-highlighting=none"
  done
  report writer "$target"
  tally_group
done

# Full sweeps use ast-ext specs; the base selects normalization and comparison.
if [ "$EXT_RUN" = 1 ] && [ -d "$CORPUS/ast-ext" ]; then
  conf_reset "writer-ext"
  for spec_dir in "$CORPUS"/ast-ext/*; do
    [ -d "$spec_dir" ] || continue
    spec="$(basename "$spec_dir")"
    base="${spec%%[+-]*}"
    case "$base" in docx | epub | epub2 | epub3 | odt) continue ;; esac
    mode="$(compare_mode "$base")"
    for input in "$spec_dir"/*.json; do
      [ -f "$input" ] || continue
      run_diff "$mode" "writer-ext/$spec/$(basename "$input")" "$input" \
        "-f json -t $spec --syntax-highlighting=none" "-f json -t $spec --syntax-highlighting=none"
    done
  done
  report writer ext
  tally_group
fi

exit "$SUITE_RC"
