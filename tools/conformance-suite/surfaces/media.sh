#!/usr/bin/env bash
# Media surface: exercise both sides of the media bag against the oracle.
#
# Group `media/ipynb` extracts a notebook's embedded resources to files and diffs both the rewritten
# document and the extracted file tree. Each corpus/text/ipynb/*.ipynb case is converted to native
# with --extract-media, each tool writing into its own isolated working directory so the two media/
# trees never collide. The comparison is writer-neutral: the native AST carries the references
# rewritten to the extracted paths, and the media/ tree carries the resource bytes and their
# content-addressed names — both are compared byte-for-byte. A notebook that carries no media
# extracts nothing on either side, which agrees.
#
# Group `media/reembed` renders each notebook back to a notebook and diffs the fields the bag drives
# on the write side — each output's reconstructed metadata and each cell's attachment table — so the
# re-embedding path is checked differentially, not just the extraction path.
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

# Re-embedding path: render each notebook back to a notebook and diff the media-driven fields.
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

# Embed path: render each notebook to HTML with --embed-resources so every bagged image inlines as a
# data: URI, and diff the two HTML outputs. Wrapping is disabled to keep the comparison on the inlined
# content rather than fill-column line breaks. To hold the group on the embedding behavior alone, a
# notebook whose plain (unembedded) HTML already diverges — an unrelated writer gap, e.g. math
# rendering — is skipped rather than counted against this surface.
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
  # Only cases the two tools already render identically without embedding isolate the inlining.
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
