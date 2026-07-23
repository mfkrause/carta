#!/usr/bin/env bash
# Standalone surface: separately-authored scaffolds are never byte-equal, so instead of diffing,
# prove both standalone outputs carry the same content and metadata (per-family checks below).
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require_tools

# A metadata-rich document whose title, author, date, and body tokens survive transforms.
STDOC="$WORK/.standalone.md"
cat >"$STDOC" <<'EOF'
---
title: Sphinx Report
author:
  - Ada Lovelace
date: 2026-06-20
---

# Quux Heading

Text wibble wobble here.
EOF

# Formats that ship a default standalone template.
TARGETS="html html4 latex beamer revealjs typst markdown gfm rst asciidoc plain man opml rtf"

html_blocks() { "$OX" -f html -t json <"$1" | jq -S '.blocks'; }

check_standalone() {
  local target="$1"
  local ofile="$WORK/.sa.oracle" xfile="$WORK/.sa.carta" efile="$WORK/.sa.err"
  if ! "$ORACLE" -f markdown -s -t "$target" <"$STDOC" >"$ofile" 2>/dev/null; then
    SKIP=$((SKIP + 1))
    return
  fi
  if ! "$OX" -f markdown -s -t "$target" <"$STDOC" >"$xfile" 2>"$efile"; then
    note_err "standalone/$target" "$(head -n 3 "$efile")"
    return
  fi

  local problems=""
  local tok
  for tok in Sphinx Quux wibble; do
    grep -q "$tok" "$ofile" || problems="$problems oracle-missing:$tok"
    grep -q "$tok" "$xfile" || problems="$problems carta-missing:$tok"
  done

  case "$target" in
  html | html4)
    if ! diff <(html_blocks "$ofile") <(html_blocks "$xfile") >/dev/null 2>&1; then
      problems="$problems body-ast-differs"
    fi
    ;;
  latex | beamer)
    # Slot bytes are a scaffold choice, so only macro presence and the author text are asserted.
    local macro
    for macro in '\title{' '\author{'; do
      grep -qF "$macro" "$ofile" || problems="$problems oracle-no-macro:$macro"
      grep -qF "$macro" "$xfile" || problems="$problems carta-no-macro:$macro"
    done
    grep -q Lovelace "$ofile" || problems="$problems oracle-no-author"
    grep -q Lovelace "$xfile" || problems="$problems carta-no-author"
    ;;
  esac

  if [ -z "$problems" ]; then
    PASS=$((PASS + 1))
  else
    note_fail "standalone/$target" "$problems"
  fi
}

conf_reset "standalone-defaults"
for target in $TARGETS; do
  check_standalone "$target"
done
report standalone defaults
tally_group

exit "$SUITE_RC"
