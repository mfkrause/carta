#!/usr/bin/env bash
# Round-trip surface: exercise the JSON codec over realistic ASTs and confirm identity.
#
# For each `.native` document in the fetched corpus, mint the JSON AST with pandoc
# (`pandoc -f native -t json`), feed that JSON through `carta -f json -t json`, and require the
# result to match structurally. This proves carta decodes and re-encodes the interchange AST
# without loss. The native corpus is gitignored and only present once tools/fetch-pandoc-tests.sh
# has run; absent it, the surface reports zero cases.
#
# Usage: surfaces/roundtrip.sh
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require_tools

golden="$WORK/.rt.golden.json"
actual="$WORK/.rt.ox.json"
efile="$WORK/.rt.err"

conf_reset "roundtrip"
if [ -d "$FETCHED" ]; then
  for native in "$FETCHED"/*.native; do
    [ -f "$native" ] || continue
    label="roundtrip/$(basename "$native")"
    if ! "$ORACLE" -f native -t json <"$native" >"$golden" 2>/dev/null; then
      SKIP=$((SKIP + 1))
      continue
    fi
    if ! "$OX" -f json -t json <"$golden" >"$actual" 2>"$efile"; then
      note_err "$label" "$(head -n 3 "$efile")"
      continue
    fi
    if detail=$(compare_json "$golden" "$actual"); then
      PASS=$((PASS + 1))
    else
      note_fail "$label" "$detail"
    fi
  done
fi
report roundtrip native
tally_group

exit "$SUITE_RC"
