#!/usr/bin/env bash
# Media surface: extract a notebook's embedded resources to files and diff both the rewritten
# document and the extracted file tree against the oracle.
#
# Each corpus/text/ipynb/*.ipynb case is converted to native with --extract-media, each tool writing
# into its own isolated working directory so the two media/ trees never collide. The comparison is
# writer-neutral: the native AST carries the references rewritten to the extracted paths, and the
# media/ tree carries the resource bytes and their content-addressed names — both are compared
# byte-for-byte. A notebook that carries no media extracts nothing on either side, which agrees.
#
# Usage: surfaces/media.sh
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require_tools

dir="$CORPUS/text/ipynb"
conf_reset "media-ipynb"
for input in "$dir"/*; do
  [ -f "$input" ] || continue
  label="media/ipynb/$(basename "$input")"
  work="$WORK/media/$(basename "$input")"
  rm -rf "$work"
  mkdir -p "$work/oracle" "$work/ox"
  repro="repro: (cd DIR && \$TOOL -f ipynb -t native --extract-media=media <$input)"
  # Each tool runs in its own directory and extracts to a bare `media`, so both resolve the same
  # relative reference while writing to disjoint trees.
  if ! ( cd "$work/oracle" && "$ORACLE" -f ipynb -t native --extract-media=media <"$input" >out.native 2>err ); then
    SKIP=$((SKIP + 1))
    continue
  fi
  if ! ( cd "$work/ox" && "$OX" -f ipynb -t native --extract-media=media <"$input" >out.native 2>err ); then
    note_err "$label" "$repro
$(head -n 3 "$work/ox/err")"
    continue
  fi
  detail=""
  if ! d=$(compare_text "$work/oracle/out.native" "$work/ox/out.native"); then
    detail="native AST differs:
$d"
  fi
  if ! d=$(compare_dir "$work/oracle/media" "$work/ox/media"); then
    detail="${detail:+$detail
}extracted media differs:
$d"
  fi
  if [ -z "$detail" ]; then
    PASS=$((PASS + 1))
  else
    note_fail "$label" "$repro
$detail"
  fi
done
report media ipynb
tally_group

exit "$SUITE_RC"
