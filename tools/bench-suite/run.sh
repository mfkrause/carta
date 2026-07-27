#!/usr/bin/env bash
# Benchmark-suite dispatcher: time carta against the pinned pandoc binary on equivalent work.
# Usage: run.sh <surface [filter]|pair <from> <to>|all> [--json <path>]; needs hyperfine, jq,
# .oracle/; see README.md.
set -uo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$DIR/lib.sh"
SURFACES="reader writer e2e startup size"

JSON_OUT=""
ARGS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --json)
      [ $# -ge 2 ] || { echo "error: --json needs a path" >&2; exit 2; }
      JSON_OUT="$2"
      shift 2
      ;;
    --json=*)
      JSON_OUT="${1#--json=}"
      shift
      ;;
    *)
      ARGS+=("$1")
      shift
      ;;
  esac
done
# bash 3.2 treats an unset empty array as an unbound variable under `set -u`.
set -- ${ARGS[@]+"${ARGS[@]}"}

[ $# -ge 1 ] || { echo "usage: run.sh <surface|all|pair> [args] [--json <path>]" >&2; exit 2; }

require_tools
ensure_release_binary
bash "$DIR/gen-fixtures.sh" || exit 1

if [ -n "$JSON_OUT" ]; then
  BENCH_ROWS="$BENCH_OUT/rows.jsonl"
  : >"$BENCH_ROWS"
  export BENCH_ROWS
fi

echo "# carta vs pandoc $(oracle_version) — $(date '+%Y-%m-%d')"

run_surface() {
  local surface="$1"
  shift
  local script="$DIR/surfaces/$surface.sh"
  if [ ! -f "$script" ]; then
    echo "error: unknown surface '$surface' (expected one of: $SURFACES all pair)" >&2
    return 2
  fi
  bash "$script" "$@"
}

# Merges the recorded rows with a provenance header describing what produced them.
write_json() {
  local runs="$BENCH_RUNS"
  [ -n "$runs" ] || runs=$(jq -s '[.[] | select(.kind == "timing") | .runs] | min // 0' "$BENCH_ROWS")
  mkdir -p "$(dirname "$JSON_OUT")"
  jq -s \
    --arg date "$(date '+%Y-%m-%d')" \
    --arg carta "$(carta_version)" \
    --arg pandoc "$(oracle_version)" \
    --arg hyperfine "$(hyperfine_version)" \
    --arg runner "$(runner_description)" \
    --argjson warmup "$BENCH_WARMUP" \
    --argjson runs "$runs" \
    '{
       provenance: {
         date: $date, carta: $carta, pandoc: $pandoc, hyperfine: $hyperfine,
         runner: $runner, warmup: $warmup, runs: $runs
       },
       timings: [.[] | select(.kind == "timing") | del(.kind)],
       binary:  ([.[] | select(.kind == "binary") | del(.kind)] | last)
     }' "$BENCH_ROWS" >"$JSON_OUT"
  echo "wrote $JSON_OUT" >&2
}

rc=0
case "$1" in
  all)
    for surface in $SURFACES; do run_surface "$surface" || rc=1; done
    ;;
  pair)
    [ $# -eq 3 ] || { echo "usage: run.sh pair <from> <to>" >&2; exit 2; }
    run_surface e2e "$2:$3" || rc=$?
    ;;
  *)
    run_surface "$@" || rc=$?
    ;;
esac

[ -n "$JSON_OUT" ] && write_json
exit "$rc"
