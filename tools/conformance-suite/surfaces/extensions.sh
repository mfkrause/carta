#!/usr/bin/env bash
# Extensions surface: assert every extension the reader honors is exercised by an oracle-parity case.
#
# Golden snapshots lock carta's own output; only the reader surface's `corpus/text-ext/<spec>/` cases
# diff an extension against the oracle. So an extension can be fully wired into the reader yet never
# compared to the oracle — invisible to CI. This surface closes that hole: it fails when a honored
# extension has no `text-ext/` directory, turning "implemented but never oracle-verified" into a hard
# error instead of a silent gap.
#
# Pure structural check — no oracle, jq, or built binary required. Three inputs, all in-repo:
#   honored  = every `Extension::<Variant>` the reader's source branches on (auto-updates: wiring a
#              new extension necessarily references its variant here)
#   token    = each variant's identifier, parsed from the `define_extensions!` table (single source)
#   covered  = every extension token named in a `corpus/text-ext/<spec>/` directory (a `+tok` that
#              enables it or a `-tok` that exercises its absence both count)
#
# Usage: surfaces/extensions.sh   (no arguments)
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

EXTENSIONS_RS="$ROOT/crates/carta-core/src/extensions.rs"
READER_SRC="$ROOT/crates/carta-readers/src"
TEXT_EXT="$CORPUS/text-ext"

# Variant -> token, straight from the `Variant => "token",` rows of the define_extensions! table.
declare -A TOKEN
while read -r variant token; do
  [ -n "$variant" ] && TOKEN["$variant"]="$token"
done < <(sed -nE 's/^[[:space:]]+([A-Z][A-Za-z]+)[[:space:]]*=>[[:space:]]*"([a-z_]+)".*/\1 \2/p' "$EXTENSIONS_RS")

# Every extension token an existing text-ext spec dir exercises (split each spec on +/-, drop the
# leading base-format segment).
covered="$(
  for spec_dir in "$TEXT_EXT"/*/; do
    [ -d "$spec_dir" ] || continue
    basename "$spec_dir" | sed 's/[+-]/\n/g' | tail -n +2
  done | sort -u
)"

conf_reset "extensions-coverage"
# A grepped name counts as honored only when it is a real variant (in TOKEN), which filters out the
# `ALL`/`COUNT` associated constants that share the `Extension::` prefix.
for variant in $(grep -rhoE 'Extension::[A-Z][A-Za-z]+' "$READER_SRC" | sed 's/Extension:://' | sort -u); do
  token="${TOKEN[$variant]:-}"
  [ -n "$token" ] || continue
  if printf '%s\n' "$covered" | grep -Fxq "$token"; then
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
