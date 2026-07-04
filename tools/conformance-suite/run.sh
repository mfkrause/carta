#!/usr/bin/env bash
# Conformance-suite dispatcher.
#
# Usage:
#   run.sh <surface> [arg]   run one surface (reader|writer|e2e|roundtrip|commands|extensions|
#                            templates|standalone|media|epub), optional arg narrows it (a format,
#                            target, or group)
#   run.sh all               run every surface
#
# Each surface prints one `RESULT <surface> <group> pass=N fail=N err=N skip=N` line per group and
# exits non-zero if any group recorded a failure or error. `all` runs them in sequence and exits
# non-zero if any surface did. Requires the gitignored .oracle/ (binary + fetched corpus) and a
# built carta binary; see lib.sh for provisioning hints.
set -uo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SURFACES="reader writer e2e roundtrip commands extensions templates standalone media epub"

# Mint one scratch directory per top-level invocation and export it so every surface child (run as a
# separate process below) shares the same dir — and thus the extracted-spec cache reader/e2e reuse.
# Two concurrent runs get distinct dirs and cannot clobber each other. An explicit CONF_WORK wins.
if [ -z "${CONF_WORK:-}" ]; then
  CONF_WORK="$(mktemp -d "${TMPDIR:-/tmp}/carta-conformance.XXXXXX")"
  export CONF_WORK
fi

run_one() {
  local surface="$1"
  shift
  local script="$DIR/surfaces/$surface.sh"
  if [ ! -f "$script" ]; then
    echo "error: unknown surface '$surface' (expected one of: $SURFACES all)" >&2
    return 2
  fi
  bash "$script" "$@"
}

[ $# -ge 1 ] || { echo "usage: run.sh <surface|all> [arg]" >&2; exit 2; }

if [ "$1" = "all" ]; then
  rc=0
  for surface in $SURFACES; do
    run_one "$surface" || rc=1
  done
  exit "$rc"
fi

run_one "$@"
