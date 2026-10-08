#!/usr/bin/env bash
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

stat_bytes() { stat -f%z "$1" 2>/dev/null || stat -c%s "$1" 2>/dev/null; }

xb=$(stat_bytes "$OX")
pb=$(stat_bytes "$ORACLE")

echo
echo "## binary size"
echo
echo "| binary | size       | ratio |"
echo "|--------|------------|-------|"
printf '| %-6s | %10s | %5s |\n' "carta"  "$(human_bytes "$xb")" "1.0x"
printf '| %-6s | %10s | %5s |\n' "pandoc" "$(human_bytes "$pb")" \
  "$(awk -v p="$pb" -v x="$xb" 'BEGIN { printf "%.0fx", p/x }')"

record_binary "$xb" "$pb"
