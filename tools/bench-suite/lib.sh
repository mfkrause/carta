# Benchmark-suite primitives: release-build, timing via hyperfine, peak-RSS measurement, and table
# formatting. Path anchors and the oracle contract come from tools/shared.sh. Sourced by run.sh,
# gen-fixtures.sh and every surface.
#
# The suite times carta against the pinned pandoc binary on equivalent work — identical -f/-t flags,
# pandoc normalized so both produce the same output (no syntax highlighting, MathJax for HTML). It
# never diffs output; correctness is the conformance suite's job. Results are machine-specific and are
# never committed (raw JSON lands in the gitignored output dir).

# Guard against double-sourcing.
[ -n "${BENCH_LIB_SOURCED:-}" ] && return 0
BENCH_LIB_SOURCED=1

# Deterministic number formatting (no locale decimal commas) and stable tool output.
export LC_ALL=C

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$BENCH_DIR/../shared.sh"
ORACLE_VERSION_FILE="$ROOT/.oracle/PANDOC_VERSION"
OX="${OXIDOC_BIN:-$ROOT/target/release/carta}"
SEED="$CORPUS/bench/seed.md"

# Tunables (env-overridable).
BENCH_SIZES="${BENCH_SIZES:-10k,100k,1m}"
BENCH_WARMUP="${BENCH_WARMUP:-3}"
BENCH_RUNS="${BENCH_RUNS:-}" # empty => hyperfine adaptive (with a min-runs floor)
BENCH_OUT="${BENCH_OUT:-$ROOT/target/bench}"
FIXTURES="$BENCH_OUT/fixtures"
mkdir -p "$BENCH_OUT" "$FIXTURES"

# Curated AST subset for the writer surface (see docs/plans/benchmark-suite.md §6.4). A representative
# mix from the universally-renderable set, tables placed early so even the smallest size exercises
# table layout. Order is load-bearing: cycling truncates at a block boundary from the front.
WRITER_AST_FILES="
common/paragraph
common/headers
table/table-simple
common/bullet-list-loose
common/blockquote
table/table-aligned
common/code-block-lang
common/ordered-nested
common/emphasis-family
table/table-colspan
common/link-title-attr
common/raw-html-block
common/definition-list-loose
"

# Fail loudly with provisioning hints when a prerequisite is missing.
require_tools() {
  local missing=0
  if ! command -v hyperfine >/dev/null 2>&1; then
    printf 'error: hyperfine not found on PATH\n  install it: brew install hyperfine  (or: cargo install hyperfine)\n' >&2
    missing=1
  fi
  if [ ! -x "$ORACLE" ]; then
    printf 'error: pandoc oracle not found at %s\n  provision it: tools/install-pandoc.sh\n' "$ORACLE" >&2
    missing=1
  fi
  if ! command -v jq >/dev/null 2>&1; then
    printf 'error: jq not found on PATH (used to build fixtures and parse results)\n' >&2
    missing=1
  fi
  [ "$missing" -eq 0 ] || exit 1
}

# Build the optimized binary the suite measures. A no-op when already fresh; guarantees we never
# publish numbers from a stale or debug build.
ensure_release_binary() {
  echo "building carta --release ..." >&2
  if ! (cd "$ROOT" && cargo build --release -p carta >&2); then
    echo "error: failed to build carta --release" >&2
    exit 1
  fi
}

oracle_version() { [ -f "$ORACLE_VERSION_FILE" ] && cat "$ORACLE_VERSION_FILE" || echo "unknown"; }

# Parse one size token (e.g. 10k, 100k, 1m, 2048) into bytes on stdout.
size_to_bytes() {
  local s="$1" n unit
  n="${s%[kKmM]}"
  unit="${s#"$n"}"
  case "$unit" in
    k | K) echo $((n * 1024)) ;;
    m | M) echo $((n * 1024 * 1024)) ;;
    *) echo "$n" ;;
  esac
}

# Human-readable byte count (e.g. 1.5 MB) on stdout.
human_bytes() {
  awk -v b="$1" 'BEGIN {
    if (b >= 1048576) printf "%.1f MB", b/1048576;
    else if (b >= 1024) printf "%.1f KB", b/1024;
    else printf "%d B", b;
  }'
}

# Detect the /usr/bin/time flavor once: BSD/macOS (`-l`, RSS in bytes) vs GNU (`-v`, RSS in kbytes).
# Sets TIME_FLAG and TIME_RSS_SCALE, or TIME_FLAG="" when neither works (RSS then unavailable).
detect_time_flavor() {
  [ -n "${TIME_FLAG+x}" ] && return 0
  if /usr/bin/time -l true >/dev/null 2>&1; then
    TIME_FLAG="-l"
    TIME_RSS_SCALE=1
  elif /usr/bin/time -v true >/dev/null 2>&1; then
    TIME_FLAG="-v"
    TIME_RSS_SCALE=1024
  else
    TIME_FLAG=""
    TIME_RSS_SCALE=1
  fi
}

# Peak RSS in bytes for one run of a command reading `input` on stdin; empty string if unmeasurable.
# Usage: measure_rss <input_file> <argv...>
measure_rss() {
  detect_time_flavor
  [ -n "$TIME_FLAG" ] || { echo ""; return; }
  local input="$1"
  shift
  local report value
  # /usr/bin/time writes its report to stderr; the program's stdout is discarded.
  report=$({ /usr/bin/time "$TIME_FLAG" "$@" <"$input" >/dev/null; } 2>&1)
  value=$(printf '%s\n' "$report" | grep -i 'maximum resident set size' | grep -oE '[0-9]+' | head -1)
  [ -n "$value" ] || { echo ""; return; }
  echo $((value * TIME_RSS_SCALE))
}

# Time carta vs pandoc on one (input, flag-pair) and append a result row to the current table.
# Reads input via hyperfine's --input so both commands see identical stdin with --shell=none.
# Usage: bench_pair <label> <input_file> <input_bytes> <oracle_args> <carta_args>
bench_pair() {
  local label="$1" input="$2" bytes="$3" oargs="$4" xargs="$5"
  local json="$BENCH_OUT/$(printf '%s' "$label" | tr '/ ' '__').json"
  local runs_arg=""
  [ -n "$BENCH_RUNS" ] && runs_arg="--min-runs $BENCH_RUNS --max-runs $BENCH_RUNS"
  # shellcheck disable=SC2086
  if ! hyperfine --shell=none --warmup "$BENCH_WARMUP" $runs_arg \
    --input "$input" --export-json "$json" \
    --command-name carta  "$OX $xargs" \
    --command-name pandoc "$ORACLE $oargs" \
    >/dev/null 2>"$BENCH_OUT/.hf.err"; then
    note_err "$label" "$(head -n 3 "$BENCH_OUT/.hf.err")"
    return
  fi
  local x_mean x_sd p_mean p_sd
  x_mean=$(jq -r '.results[] | select(.command=="carta")  | .mean'   "$json")
  x_sd=$(jq   -r '.results[] | select(.command=="carta")  | .stddev' "$json")
  p_mean=$(jq -r '.results[] | select(.command=="pandoc") | .mean'   "$json")
  p_sd=$(jq   -r '.results[] | select(.command=="pandoc") | .stddev' "$json")

  local x_rss p_rss
  x_rss=$(measure_rss "$input" $OX $xargs)
  p_rss=$(measure_rss "$input" $ORACLE $oargs)

  emit_row "$label" "$bytes" "$x_mean" "$x_sd" "$p_mean" "$p_sd" "$x_rss" "$p_rss"
}

# Render one table row from raw seconds/bytes. Derives speedup, throughput, and human units.
emit_row() {
  local label="$1" bytes="$2" xm="$3" xsd="$4" pm="$5" psd="$6" xr="$7" pr="$8"
  awk -v label="$label" -v bytes="$bytes" -v xm="$xm" -v xsd="$xsd" -v pm="$pm" -v psd="$psd" \
      -v xr="$xr" -v pr="$pr" '
    function ms(s) { return sprintf("%.2f", s*1000) }
    function mb(b) { if (b=="" || b=="null") return "-"; if (b>=1048576) return sprintf("%.1f MB", b/1048576); if (b>=1024) return sprintf("%.1f KB", b/1024); return sprintf("%d B", b) }
    BEGIN {
      sz = (bytes>=1048576) ? sprintf("%.0f MB", bytes/1048576) : (bytes>=1024 ? sprintf("%.0f KB", bytes/1024) : sprintf("%d B", bytes));
      speedup = (xm>0) ? sprintf("%.1fx", pm/xm) : "-";
      thru = (xm>0) ? sprintf("%.1f", (bytes/1048576)/xm) : "-";
      printf "| %-6s | %8s ms ± %-5s | %8s ms ± %-5s | %7s | %10s | %9s | %10s |\n",
        sz, ms(xm), ms(xsd), ms(pm), ms(psd), speedup, thru, mb(xr), mb(pr);
    }'
}

table_header() {
  echo
  echo "## $1"
  echo
  echo "| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |"
  echo "|--------|---------------------|---------------------|---------|------------|-----------|------------|"
}

# Any benchmark error flips the suite return code; each surface exits with it.
SUITE_RC=0
note_err() { SUITE_RC=1; echo "ERR  $1: $2" >&2; }

# Comma list -> space list (for iterating BENCH_SIZES).
sizes_list() { printf '%s\n' "$BENCH_SIZES" | tr ',' ' '; }

# Path of the generated input file for a reader format at a given size.
fixture_for() { # <fmt> <size>
  case "$1" in
    commonmark | markdown) echo "$FIXTURES/commonmark.$2.md" ;;
    html | html5) echo "$FIXTURES/html.$2.html" ;;
    native) echo "$FIXTURES/native.$2.native" ;;
    json) echo "$FIXTURES/json.$2.json" ;;
  esac
}

# Byte length of a file (whitespace-trimmed).
file_bytes() { wc -c <"$1" | tr -d ' '; }
