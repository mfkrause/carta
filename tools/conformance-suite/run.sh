#!/usr/bin/env bash
# Conformance-suite dispatcher: `run.sh <surface> [arg]` (arg narrows to a format/target/group) or `run.sh all`.
# Prints `RESULT <surface> <group> pass=N fail=N err=N skip=N` per group, exits non-zero on any fail/err; needs .oracle/ and a built carta binary (see lib.sh).
set -uo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SURFACES="reader writer e2e roundtrip commands extensions templates standalone media epub docx odt highlight"

# One exported scratch dir per invocation: surface children share the extracted-spec cache, concurrent runs stay disjoint; explicit CONF_WORK wins.
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

# Package-shaped targets have dedicated surfaces; the writer text diff cannot compare them, so redirect.
if [ "$1" = "writer" ] && [ $# -ge 2 ]; then
  case "$2" in
    epub*) echo "error: $2 targets are exercised by the epub surface: run.sh epub" >&2; exit 2 ;;
    docx) echo "error: docx targets are exercised by the docx surface: run.sh docx" >&2; exit 2 ;;
    odt) echo "error: odt targets are exercised by the odt surface: run.sh odt" >&2; exit 2 ;;
  esac
fi

if [ "$1" = "all" ]; then
  rc=0
  for surface in $SURFACES; do
    run_one "$surface" || rc=1
  done
  exit "$rc"
fi

run_one "$@"
