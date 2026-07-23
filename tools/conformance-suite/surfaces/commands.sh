#!/usr/bin/env bash
# Reuse pandoc's command tests as differential cases, diffing carta against a live normalized oracle
# run, NOT the baked expected. Only bare format-flag commands on implemented pairs run; rest skipped.
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require_tools

READABLE="commonmark html native json docx epub odt"
WRITABLE="html native json mediawiki rtf"

in_set() {
  local needle="$1"
  shift
  local item
  for item in $1; do [ "$item" = "$needle" ] && return 0; done
  return 1
}

# Populate FROM/TO from a command string; OK=0 when any non-format flag appears.
parse_cmd() {
  FROM="" TO="" OK=1
  local -a toks
  read -ra toks <<<"$1"
  if [ "${toks[0]:-}" != "pandoc" ]; then OK=0; return; fi
  local i=1 n=${#toks[@]} t
  while [ "$i" -lt "$n" ]; do
    t="${toks[$i]}"
    case "$t" in
      -f | -r | --from | --read) i=$((i + 1)); FROM="${toks[$i]:-}" ;;
      -t | -w | --to | --write) i=$((i + 1)); TO="${toks[$i]:-}" ;;
      --from=* | --read=*) FROM="${t#*=}" ;;
      --to=* | --write=*) TO="${t#*=}" ;;
      -f* | -r*) FROM="${t#-?}" ;;
      -t* | -w*) TO="${t#-?}" ;;
      *) OK=0; return ;;
    esac
    i=$((i + 1))
  done
}

cmddir="$WORK/cmd"
rm -rf "$cmddir"
mkdir -p "$cmddir"
manifest="$WORK/cmd.manifest"
: >"$manifest"

extract='
  /^`+$/ {
    if (st == 0) { st = 1; flen = length($0); cmd = ""; input = ""; sawD = 0; phase = "cmd"; next }
    if (length($0) == flen) {
      if (cmd != "" && sawD == 1) {
        fn = sprintf("%s/%s.%03d.in", outdir, base, ++seq)
        printf "%s", input > fn; close(fn)
        printf "%s\t%s\n", fn, cmd
      }
      st = 0; next
    }
    if (st == 1 && phase == "input") { input = input $0 "\n" }
    next
  }
  st == 1 && phase == "cmd" {
    if ($0 ~ /^%[ \t]/) { cmd = substr($0, 3); phase = "input" } else { phase = "skip" }
    next
  }
  st == 1 && phase == "input" {
    if ($0 == "^D") { sawD = 1; phase = "expected"; next }
    input = input $0 "\n"; next
  }
  { next }
'

conf_reset "commands"
cmdroot="$FETCHED/command"
if [ -d "$cmdroot" ]; then
  for file in "$cmdroot"/*.md; do
    [ -f "$file" ] || continue
    awk -v base="$(basename "$file" .md)" -v outdir="$cmddir" "$extract" "$file" >>"$manifest"
  done

  while IFS=$'\t' read -r infile cmd; do
    parse_cmd "$cmd"
    if [ "$OK" -ne 1 ] || ! in_set "$FROM" "$READABLE" || ! in_set "$TO" "$WRITABLE"; then
      SKIP=$((SKIP + 1))
      continue
    fi
    is_json_target "$TO" && mode=json || mode=text
    run_diff "$mode" "commands/$(basename "$infile" .in) [$cmd]" "$infile" \
      "-f $FROM -t $TO $(oracle_norm "$TO")" "-f $FROM -t $TO"
  done <"$manifest"
fi
report commands all
tally_group

exit "$SUITE_RC"
