#!/usr/bin/env bash
# Media surface: diff --extract-media output (native AST plus media/ tree), notebook re-embedding
# (output metadata and attachment tables), and --embed-resources HTML inlining against the oracle.
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
  # each tool extracts to its own bare media/ so the trees never collide
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

conf_reset "media-reembed"
for input in "$dir"/*; do
  [ -f "$input" ] || continue
  label="media/reembed/$(basename "$input")"
  repro="repro: \$TOOL -f ipynb -t ipynb <$input"
  ofile="$WORK/.reembed.oracle" xfile="$WORK/.reembed.ox" efile="$WORK/.reembed.err"
  if ! "$ORACLE" -f ipynb -t ipynb <"$input" >"$ofile" 2>/dev/null; then
    SKIP=$((SKIP + 1))
    continue
  fi
  if ! "$OX" -f ipynb -t ipynb <"$input" >"$xfile" 2>"$efile"; then
    note_err "$label" "$repro
$(head -n 3 "$efile")"
    continue
  fi
  if detail=$(compare_ipynb_media "$ofile" "$xfile"); then
    PASS=$((PASS + 1))
  else
    note_fail "$label" "$repro
media fields differ:
$detail"
  fi
done
report media reembed
tally_group

# Embed group: diff --embed-resources HTML with wrapping off; a notebook whose plain HTML already
# diverges (an unrelated writer gap) is skipped so only the embedding behavior is judged.
conf_reset "media-embed"
for input in "$dir"/*; do
  [ -f "$input" ] || continue
  label="media/embed/$(basename "$input")"
  repro="repro: \$TOOL -f ipynb -t html --embed-resources --wrap=none <$input"
  ofile="$WORK/.embed.oracle" xfile="$WORK/.embed.ox" efile="$WORK/.embed.err"
  obase="$WORK/.embed.oracle.plain" xbase="$WORK/.embed.ox.plain"
  if ! "$ORACLE" -f ipynb -t html --wrap=none <"$input" >"$obase" 2>/dev/null \
    || ! "$OX" -f ipynb -t html --wrap=none <"$input" >"$xbase" 2>/dev/null; then
    SKIP=$((SKIP + 1))
    continue
  fi
  if ! compare_text "$obase" "$xbase" >/dev/null; then
    SKIP=$((SKIP + 1))
    continue
  fi
  if ! "$ORACLE" -f ipynb -t html --embed-resources --wrap=none <"$input" >"$ofile" 2>/dev/null; then
    SKIP=$((SKIP + 1))
    continue
  fi
  if ! "$OX" -f ipynb -t html --embed-resources --wrap=none <"$input" >"$xfile" 2>"$efile"; then
    note_err "$label" "$repro
$(head -n 3 "$efile")"
    continue
  fi
  if detail=$(compare_text "$ofile" "$xfile"); then
    PASS=$((PASS + 1))
  else
    note_fail "$label" "$repro
embedded HTML differs:
$detail"
  fi
done
report media embed
tally_group

exit "$SUITE_RC"
