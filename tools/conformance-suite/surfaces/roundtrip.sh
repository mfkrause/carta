#!/usr/bin/env bash
# Round-trip surface: for each fetched-corpus .native doc, mint the JSON AST with pandoc, feed it
# through `carta -f json -t json`, and require structural identity. Corpus absent: zero cases.
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
