# Shared primitives for the conformance suite: path discovery, oracle normalization, output
# comparison, spec-example extraction, and result tallying. Sourced by run.sh and every surface.
#
# The suite diffs oxidoc against the pinned pandoc binary across each conversion surface. pandoc's
# output is the reference; on any non-`json` target it carries a single trailing newline that is
# stripped from both sides before comparison, and JSON targets are compared after canonical key
# sorting so object-key order never registers as a divergence.

# Guard against double-sourcing.
[ -n "${CONF_LIB_SOURCED:-}" ] && return 0
CONF_LIB_SOURCED=1

CONF_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$CONF_DIR/../.." && pwd)"
ORACLE="$ROOT/.oracle/bin/pandoc"
OX="${OXIDOC_BIN:-$ROOT/target/debug/oxidoc}"
SPEC="$ROOT/vendor/commonmark/spec.txt"
CORPUS="$ROOT/corpus"
EXCLUSIONS="$CORPUS/exclusions.tsv"
FETCHED="$ROOT/.oracle/tests/test"
WORK="${CONF_WORK:-${TMPDIR:-/tmp}/oxidoc-conformance}"
mkdir -p "$WORK"

# Fail loudly with provisioning instructions when a prerequisite is missing.
require_tools() {
  local missing=0
  if [ ! -x "$ORACLE" ]; then
    printf 'error: pandoc oracle not found at %s\n  provision it: tools/install-pandoc.sh\n' "$ORACLE" >&2
    missing=1
  fi
  if [ ! -x "$OX" ]; then
    printf 'error: oxidoc binary not found at %s\n  build it: cargo build -p oxidoc\n' "$OX" >&2
    missing=1
  fi
  if ! command -v jq >/dev/null 2>&1; then
    printf 'error: jq not found on PATH (used for JSON comparison)\n' >&2
    missing=1
  fi
  [ "$missing" -eq 0 ] || exit 1
}

# Oracle flags that neutralize target nondeterminism oxidoc does not reproduce: HTML suppresses
# syntax highlighting and renders math via MathJax; LaTeX suppresses syntax highlighting. Applied to
# the pandoc side only.
oracle_norm() {
  case "$1" in
    html | html5) echo "--syntax-highlighting=none --mathjax" ;;
    latex) echo "--syntax-highlighting=none" ;;
  esac
}

# A target whose output is compared structurally as JSON rather than byte-for-byte.
is_json_target() { [ "$1" = "json" ]; }

# 0 when (target, feature) is listed as not-yet-implemented in exclusions.tsv.
is_excluded() {
  local target="$1" feature="$2"
  [ -f "$EXCLUSIONS" ] || return 1
  grep -v '^[[:space:]]*#' "$EXCLUSIONS" | grep -q "^${target}	${feature}\$"
}

# Compare two JSON files after canonical key sorting. Prints a brief diff and returns 1 on mismatch.
compare_json() {
  local a="$WORK/.cmp.oracle.json" b="$WORK/.cmp.ox.json"
  jq -S . "$1" >"$a" 2>/dev/null || { echo "oracle JSON unparsable"; return 1; }
  jq -S . "$2" >"$b" 2>/dev/null || { echo "oxidoc JSON unparsable"; return 1; }
  cmp -s "$a" "$b" && return 0
  diff "$a" "$b" | head -n 8
  return 1
}

# Compare two text files modulo one trailing newline on each side. Brief diff + 1 on mismatch.
compare_text() {
  local a b
  a=$(cat "$1"; printf x); a=${a%x}; a=${a%$'\n'}
  b=$(cat "$2"; printf x); b=${b%x}; b=${b%$'\n'}
  [ "$a" = "$b" ] && return 0
  diff <(printf '%s\n' "$a") <(printf '%s\n' "$b") | head -n 8
  return 1
}

# Per-surface tally. Reset before each (surface, format) group.
conf_reset() {
  PASS=0 FAIL=0 ERR=0 SKIP=0
  SURFACE_LOG="$WORK/${1}.log"
  : >"$SURFACE_LOG"
}

note_fail() { FAIL=$((FAIL + 1)); { echo "FAIL $1"; printf '%s\n' "$2" | sed 's/^/    /'; } >>"$SURFACE_LOG"; }
note_err() { ERR=$((ERR + 1)); { echo "ERR  $1"; printf '%s\n' "$2" | sed 's/^/    /'; } >>"$SURFACE_LOG"; }

report() { echo "RESULT $1 $2 pass=$PASS fail=$FAIL err=$ERR skip=$SKIP"; }

# Suite-level return code, raised to 1 by any group that recorded a failure or error. A surface
# script seeds it to 0, calls tally_group after each report, and exits with it.
SUITE_RC=0
tally_group() { if [ "$FAIL" -gt 0 ] || [ "$ERR" -gt 0 ]; then SUITE_RC=1; fi; }

# One differential case: convert `input` with the oracle and with oxidoc, then compare.
# Usage: run_diff <json|text> <label> <input_file> <oracle_arg_string> <oxidoc_arg_string>
# Word-splitting of the arg strings is intentional (they are space-separated flags we control).
run_diff() {
  local mode="$1" label="$2" input="$3" oargs="$4" xargs="$5"
  local ofile="$WORK/.run.oracle" xfile="$WORK/.run.ox" efile="$WORK/.run.err"
  # shellcheck disable=SC2086
  if ! "$ORACLE" $oargs <"$input" >"$ofile" 2>/dev/null; then
    SKIP=$((SKIP + 1))
    return
  fi
  # shellcheck disable=SC2086
  if ! "$OX" $xargs <"$input" >"$xfile" 2>"$efile"; then
    note_err "$label" "$(head -n 3 "$efile")"
    return
  fi
  local detail
  if detail=$("compare_$mode" "$ofile" "$xfile"); then
    PASS=$((PASS + 1))
  else
    note_fail "$label" "$detail"
  fi
}

# Extract every worked example's markdown input from the CommonMark spec into <dir>/NNNN.md,
# restoring the spec's → placeholder to a real tab. Cached: a populated dir is reused.
extract_spec() {
  local out="$1"
  mkdir -p "$out"
  [ -n "$(ls -A "$out" 2>/dev/null)" ] && return 0
  awk -v out="$out" '
    state == 0 && /^`+ example$/ { n++; md = ""; state = 1; next }
    state == 1 {
      if ($0 == ".") { printf "%s", md > sprintf("%s/%04d.md", out, n); close(sprintf("%s/%04d.md", out, n)); state = 2; next }
      line = $0; gsub(/→/, "\t", line); md = md line "\n"; next
    }
    state == 2 { if ($0 ~ /^`+$/) state = 0; next }
  ' "$SPEC"
}
