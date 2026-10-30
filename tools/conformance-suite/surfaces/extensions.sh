#!/usr/bin/env bash
# Check that reader extensions have text-ext cases; no binary or oracle is needed.
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

EXTENSIONS_RS="$ROOT/crates/carta-core/src/extensions.rs"
READER_SRC="$ROOT/crates/carta-readers/src"
TEXT_EXT="$CORPUS/text-ext"

# Use plain variant/token rows for compatibility with macOS bash 3.2.
TOKEN_ROWS="$(sed -nE 's/^[[:space:]]+([A-Z][A-Za-z]+)[[:space:]]*=>[[:space:]]*"([a-z_]+)".*/\1 \2/p' "$EXTENSIONS_RS")"

token_for_variant() {
  awk -v variant="$1" '$1 == variant { print $2; exit }' <<<"$TOKEN_ROWS"
}

covered="$(
  for spec_dir in "$TEXT_EXT"/*/; do
    [ -d "$spec_dir" ] || continue
    basename "$spec_dir" | sed 's/[+-]/\n/g' | tail -n +2
  done | sort -u
)"

conf_reset "extensions-coverage"
# Count only real variants, filtering the `ALL`/`COUNT` associated constants sharing the prefix.
for variant in $(grep -rhoE 'Extension::[A-Z][A-Za-z]+' "$READER_SRC" | sed 's/Extension:://' | sort -u); do
  token="$(token_for_variant "$variant")"
  [ -n "$token" ] || continue
  if grep -Fxq "$token" <<<"$covered"; then
    PASS=$((PASS + 1))
  else
    note_fail "extensions/$token" "honored by the reader but no corpus/text-ext/<base>+$token/ case diffs it against the oracle"
  fi
done

if [ "$FAIL" -gt 0 ]; then
  echo "  $FAIL honored extension(s) lack an oracle-parity case under corpus/text-ext/:" >&2
  grep '^FAIL ' "$SURFACE_LOG" | sed 's#^FAIL extensions/#    - #' >&2
fi

report extensions coverage
tally_group
exit "$SUITE_RC"
