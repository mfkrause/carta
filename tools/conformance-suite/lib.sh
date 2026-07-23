# Diffs carta against the pinned pandoc per surface: text targets compared modulo one trailing
# newline, JSON targets after canonical key sorting.

[ -n "${CONF_LIB_SOURCED:-}" ] && return 0
CONF_LIB_SOURCED=1

CONF_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$CONF_DIR/../shared.sh"
OX="${CARTA_BIN:-$ROOT/target/debug/carta}"
export CARTA_SYNTAX_DIR="${CARTA_SYNTAX_DIR-$ROOT/crates/carta-highlight/data/syntax-copyleft}"
SPEC="$ROOT/vendor/commonmark/spec.txt"
EXCLUSIONS="$CORPUS/exclusions.tsv"
FETCHED="$ROOT/.oracle/tests/test"
# Per-run scratch dir so concurrent runs never clobber each other; run.sh exports CONF_WORK so
# surface children share it. Not auto-deleted: .log files must survive for failure inspection.
WORK="${CONF_WORK:-$(mktemp -d "${TMPDIR:-/tmp}/carta-conformance.XXXXXX")}"
mkdir -p "$WORK"

require_tools() {
  local missing=0
  if [ ! -x "$ORACLE" ]; then
    printf 'error: pandoc oracle not found at %s\n  provision it: tools/install-pandoc.sh\n' "$ORACLE" >&2
    missing=1
  fi
  if [ ! -x "$OX" ]; then
    printf 'error: carta binary not found at %s\n  build it: cargo build -p carta\n' "$OX" >&2
    missing=1
  fi
  if ! command -v jq >/dev/null 2>&1; then
    printf 'error: jq not found on PATH (used for JSON comparison)\n' >&2
    missing=1
  fi
  [ "$missing" -eq 0 ] || exit 1
}

# Runtime intersection of both binaries' format lists, so a newly landed reader or writer enters
# conformance without a script edit.
shared_input_formats() {
  comm -12 \
    <("$OX" --list-input-formats | sort) \
    <("$ORACLE" --list-input-formats | sort) | tr '\n' ' '
}

# Shared outputs minus binary package targets (epub*/docx/odt have their own surfaces) and html5
# (an alias of html, covered by the html group).
shared_output_formats() {
  comm -12 \
    <("$OX" --list-output-formats | sort) \
    <("$ORACLE" --list-output-formats | sort) | grep -vE '^(epub|docx|odt|html5$)' | tr '\n' ' '
}

is_json_target() { [ "$1" = "json" ]; }

# Structural JSON for the AST, id-canonicalized JSON for notebooks (carta's cell ids are
# deterministic, the oracle's random), text modulo trailing newline otherwise.
compare_mode() {
  case "$1" in
    json) echo json ;;
    ipynb) echo ipynb ;;
    *) echo text ;;
  esac
}

# 0 when listed in exclusions.tsv: `target<TAB>feature` or `target<TAB>feature/case`.
is_excluded() {
  local target="$1" feature="$2" case="${3:-}"
  [ -f "$EXCLUSIONS" ] || return 1
  local active
  active="$(grep -v '^[[:space:]]*#' "$EXCLUSIONS")"
  printf '%s\n' "$active" | grep -q "^${target}	${feature}\$" && return 0
  [ -n "$case" ] && printf '%s\n' "$active" | grep -q "^${target}	${feature}/${case}\$"
}

compare_json() {
  local a="$WORK/.cmp.oracle.json" b="$WORK/.cmp.ox.json"
  jq -S . "$1" >"$a" 2>/dev/null || { echo "oracle JSON unparsable"; return 1; }
  jq -S . "$2" >"$b" 2>/dev/null || { echo "carta JSON unparsable"; return 1; }
  cmp -s "$a" "$b" && return 0
  # Bound the diff so a pathological (megabyte) mismatch stays reviewable.
  diff "$a" "$b" | head -n 200
  return 1
}

# Structural notebook compare after folding each cell's `id` to a constant (carta derives ids
# deterministically, the oracle draws random ones).
compare_ipynb() {
  local a="$WORK/.cmp.oracle.ipynb" b="$WORK/.cmp.ox.ipynb"
  jq -S '(.cells[]?.id) |= "id"' "$1" >"$a" 2>/dev/null || { echo "oracle notebook unparsable"; return 1; }
  jq -S '(.cells[]?.id) |= "id"' "$2" >"$b" 2>/dev/null || { echo "carta notebook unparsable"; return 1; }
  cmp -s "$a" "$b" && return 0
  diff "$a" "$b" | head -n 200
  return 1
}

# Compare each rich output's `metadata` and each cell's `attachments` keys in emitted order: just
# what the media bag drives on the write side, isolated from cell source and minted ids.
compare_ipynb_media() {
  local proj='[.cells[] | {output_metadata: [.outputs[]? | select(.metadata) | .metadata], attachment_keys: (.attachments // {} | keys_unsorted)}]'
  local a="$WORK/.cmp.oracle.media.json" b="$WORK/.cmp.ox.media.json"
  jq -c "$proj" "$1" >"$a" 2>/dev/null || { echo "oracle notebook unparsable"; return 1; }
  jq -c "$proj" "$2" >"$b" 2>/dev/null || { echo "carta notebook unparsable"; return 1; }
  cmp -s "$a" "$b" && return 0
  diff "$a" "$b" | head -n 200
  return 1
}

# Byte-exact compare (trailing newlines included) for verbatim output such as a filled template;
# diff via `cat -A` so whitespace stays visible.
compare_bytes() {
  cmp -s "$1" "$2" && return 0
  diff <(cat -A "$1") <(cat -A "$2") | head -n 200
  return 1
}

# Compare two text files modulo one trailing newline on each side.
compare_text() {
  local a b
  a=$(cat "$1"; printf x); a=${a%x}; a=${a%$'\n'}
  b=$(cat "$2"; printf x); b=${b%x}; b=${b%$'\n'}
  [ "$a" = "$b" ] && return 0
  diff <(printf '%s\n' "$a") <(printf '%s\n' "$b") | head -n 200
  return 1
}

# Either side may be absent (no media extracts nothing): both-absent equal, one-absent mismatch.
compare_dir() {
  local a="$1" b="$2" a_has=0 b_has=0
  [ -d "$a" ] && [ -n "$(ls -A "$a" 2>/dev/null)" ] && a_has=1
  [ -d "$b" ] && [ -n "$(ls -A "$b" 2>/dev/null)" ] && b_has=1
  [ "$a_has" = 0 ] && [ "$b_has" = 0 ] && return 0
  if [ "$a_has" != "$b_has" ]; then
    echo "extracted media present on one side only (oracle=$a_has carta=$b_has)"
    return 1
  fi
  diff -qr "$a" "$b" >/dev/null 2>&1 && return 0
  diff -qr "$a" "$b" 2>&1 | head -n 50
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

# Suite return code: seeded 0, raised by any group with a fail/err, used as the exit code.
SUITE_RC=0
tally_group() {
  if [ "$FAIL" -gt 0 ] || [ "$ERR" -gt 0 ]; then
    SUITE_RC=1
    echo "  details: $SURFACE_LOG" >&2
  fi
}

# One differential case; arg strings are flags we control, word-split intentionally.
# Usage: run_diff <json|text> <label> <input_file> <oracle_arg_string> <carta_arg_string>
run_diff() {
  local mode="$1" label="$2" input="$3" oargs="$4" xargs="$5"
  local ofile="$WORK/.run.oracle" xfile="$WORK/.run.ox" efile="$WORK/.run.err"
  # Exact invocations so a log entry is a self-contained repro.
  local repro="repro: $ORACLE $oargs <$input
       $OX $xargs <$input"
  # shellcheck disable=SC2086
  if ! "$ORACLE" $oargs <"$input" >"$ofile" 2>/dev/null; then
    SKIP=$((SKIP + 1))
    return
  fi
  # shellcheck disable=SC2086
  if ! "$OX" $xargs <"$input" >"$xfile" 2>"$efile"; then
    note_err "$label" "$repro
$(head -n 3 "$efile")"
    return
  fi
  local detail
  if detail=$("compare_$mode" "$ofile" "$xfile"); then
    PASS=$((PASS + 1))
  else
    note_fail "$label" "$repro
$detail"
  fi
}

# Extract each spec example's markdown into <dir>/NNNN.md (→ restored to a tab); a populated dir
# is reused as a cache.
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
