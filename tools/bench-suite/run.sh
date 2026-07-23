#!/usr/bin/env bash
# Benchmark-suite dispatcher: time carta against the pinned pandoc binary on equivalent work.
# Usage: run.sh <surface [filter]|pair <from> <to>|all>; needs hyperfine, jq, .oracle/; see README.md.
set -uo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$DIR/lib.sh"
SURFACES="reader writer e2e startup size"

[ $# -ge 1 ] || { echo "usage: run.sh <surface|all|pair> [args]" >&2; exit 2; }

require_tools
ensure_release_binary
bash "$DIR/gen-fixtures.sh" || exit 1

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

case "$1" in
  all)
    rc=0
    for surface in $SURFACES; do run_surface "$surface" || rc=1; done
    exit "$rc"
    ;;
  pair)
    [ $# -eq 3 ] || { echo "usage: run.sh pair <from> <to>" >&2; exit 2; }
    run_surface e2e "$2:$3"
    ;;
  *)
    run_surface "$@"
    ;;
esac
