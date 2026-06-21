#!/usr/bin/env bash
# Standalone surface: structural parity of the default templates (NOT a byte diff).
#
# Each format ships its own separately-authored scaffold (title block, preamble, CSS), so `carta -s`
# and the oracle's `-s` are never byte-equal and must not be diffed. Instead this surface proves the
# scaffolds carry the SAME content and metadata:
#
#   - HTML family (html, html4): parse BOTH standalone outputs back through carta's own HTML reader
#     and assert the body block-AST is identical. This shows the title block, body slot, and metadata
#     land equivalently without comparing a single byte of CSS or chrome.
#   - LaTeX family (latex, beamer): assert the `\title{…}`/`\author{…}` slots are present in both with
#     the expected text.
#   - Every wrapping format: assert the title token and the body tokens appear in both outputs, so a
#     dropped title or a lost body is caught even where no reader exists to round-trip the scaffold.
#
# The oracle output is generated live and nothing is committed. Emits one `RESULT standalone defaults`
# line; non-zero exit on any fail/err.
#
# Usage: surfaces/standalone.sh
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require_tools

# A metadata-rich document with distinctive, transform-surviving tokens: a two-word title, an author,
# a date, and a body whose heading and prose carry unique words.
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
TARGETS="html html4 latex beamer revealjs typst markdown gfm rst asciidoc plain man opml"

# The body block-AST carried by a standalone HTML document, as read back by carta's own HTML reader.
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
  # Universal content presence: the title token and both body tokens must survive into both scaffolds.
  local tok
  for tok in Sphinx Quux wibble; do
    grep -q "$tok" "$ofile" || problems="$problems oracle-missing:$tok"
    grep -q "$tok" "$xfile" || problems="$problems carta-missing:$tok"
  done

  case "$target" in
  html | html4)
    # Strong check: the body block-AST must match once both scaffolds are read back through the same
    # HTML reader. Captures the title-block header structure as well as the document body.
    if ! diff <(html_blocks "$ofile") <(html_blocks "$xfile") >/dev/null 2>&1; then
      problems="$problems body-ast-differs"
    fi
    ;;
  latex | beamer)
    # The title-block macros must be wired and the author text must flow into both scaffolds. The
    # exact slot bytes are a scaffold choice (one side may wrap the title for PDF bookmarks), so only
    # the macro presence and the author text are asserted.
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
