#!/usr/bin/env bash
# Compare PDFs compiled by external Typst.
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require_tools
export CARTA_BIN="$OX" CONF_WORK="$WORK"
exec python3 "$ROOT/tools/conformance-suite/pdf.py"
