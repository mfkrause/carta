#!/usr/bin/env bash
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

FILTER="${1:-}"
OUTDIR="${CARTA_ONLY_OUT:-$BENCH_OUT/carta-only}"
mkdir -p "$OUTDIR"
WARMUP="${BENCH_WARMUP:-5}"
RUNS="${CARTA_ONLY_RUNS:-25}"

time_one() { # <label> <input> <args...>
  local label="$1" input="$2"
  shift 2
  case "$label" in *"$FILTER"*) ;; *) return 0 ;; esac
  [ -s "$input" ] || return 0
  local json="$OUTDIR/$(printf '%s' "$label" | tr '/ ' '__').json"
  if ! hyperfine --shell=none --warmup "$WARMUP" --min-runs "$RUNS" --max-runs "$RUNS" \
    --input "$input" --export-json "$json" "$OX $*" >/dev/null 2>"$OUTDIR/.err"; then
    echo "ERR  $label: $(head -n 2 "$OUTDIR/.err")" >&2
    return 1
  fi
  awk -v l="$label" '{ }' </dev/null
  jq -r --arg l "$label" '.results[0] | "\($l)\t\(.mean*1000)\t\(.stddev*1000)"' "$json"
}

for fmt in commonmark html; do
  for size in $(sizes_list); do
    time_one "reader/$fmt/$size" "$(fixture_for "$fmt" "$size")" -f "$fmt" -t json
  done
done

for target in html latex rst plain commonmark mediawiki native json; do
  for size in $(sizes_list); do
    time_one "writer/$target/$size" "$FIXTURES/ast.$size.json" -f json -t "$target"
  done
done

for pair in commonmark:html commonmark:latex commonmark:rst commonmark:json; do
  from="${pair%%:*}"
  to="${pair##*:}"
  for size in $(sizes_list); do
    time_one "e2e/$from-$to/$size" "$(fixture_for "$from" "$size")" -f "$from" -t "$to"
  done
done

for to in html json; do
  time_one "startup/commonmark-$to" "$FIXTURES/startup.md" -f commonmark -t "$to"
done
