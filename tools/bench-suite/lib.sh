# Times carta against pinned pandoc on identical work (same -f/-t; pandoc normalized: no
# highlighting, MathJax). Never diffs output; results are machine-specific and never committed.

[ -n "${BENCH_LIB_SOURCED:-}" ] && return 0
BENCH_LIB_SOURCED=1

# Deterministic number formatting (no locale decimal commas) and stable tool output.
export LC_ALL=C

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$BENCH_DIR/../shared.sh"
ORACLE_VERSION_FILE="$ROOT/.oracle/PANDOC_VERSION"
OX="${CARTA_BIN:-$ROOT/target/release/carta}"
SEED="$CORPUS/bench/seed.md"

BENCH_SIZES="${BENCH_SIZES:-10k,100k,1m}"
BENCH_WARMUP="${BENCH_WARMUP:-3}"
BENCH_RUNS="${BENCH_RUNS:-}" # empty => hyperfine adaptive (with a min-runs floor)
BENCH_OUT="${BENCH_OUT:-$ROOT/target/bench}"
# One JSON object per line, appended by whichever surface subprocess measured it; run.sh assembles
# the file once every surface has finished. Empty means markdown-only, the default.
BENCH_ROWS="${BENCH_ROWS:-}"
FIXTURES="$BENCH_OUT/fixtures"
mkdir -p "$BENCH_OUT" "$FIXTURES"

# Writer-surface AST subset; order is load-bearing: cycling truncates from the front, so tables
# sit early to reach the smallest size.
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

ensure_release_binary() {
  echo "building carta --release ..." >&2
  if ! (cd "$ROOT" && cargo build --release -p carta >&2); then
    echo "error: failed to build carta --release" >&2
    exit 1
  fi
}

oracle_version() { [ -f "$ORACLE_VERSION_FILE" ] && cat "$ORACLE_VERSION_FILE" || echo "unknown"; }

carta_version() { "$OX" --version 2>/dev/null | awk 'NR==1 { print $NF }'; }

hyperfine_version() { hyperfine --version 2>/dev/null | awk 'NR==1 { print $NF }'; }

# One line naming the machine the numbers came from; BENCH_RUNNER overrides it for hosts where the
# probes below read as something unhelpfully generic (CI images, VMs).
runner_description() {
  [ -n "${BENCH_RUNNER:-}" ] && { echo "$BENCH_RUNNER"; return; }
  local cpu cores bytes os
  case "$(uname -s)" in
    Darwin)
      cpu=$(sysctl -n machdep.cpu.brand_string 2>/dev/null)
      cores=$(sysctl -n hw.ncpu 2>/dev/null)
      bytes=$(sysctl -n hw.memsize 2>/dev/null)
      os="macOS $(sw_vers -productVersion 2>/dev/null)"
      ;;
    Linux)
      cpu=$(awk -F': ' '/^model name/ { print $2; exit }' /proc/cpuinfo 2>/dev/null)
      cores=$(nproc 2>/dev/null)
      bytes=$(awk '/^MemTotal/ { print $2 * 1024; exit }' /proc/meminfo 2>/dev/null)
      os=$(awk -F'"' '/^PRETTY_NAME=/ { print $2; exit }' /etc/os-release 2>/dev/null)
      ;;
  esac
  [ -n "$cpu" ] || cpu="unknown CPU"
  [ -n "$os" ] || os="$(uname -s)"
  local ram=""
  [ -n "$bytes" ] && ram=$(awk -v b="$bytes" 'BEGIN { printf "%.0f GB RAM, ", b/1073741824 }')
  printf '%s%s, %s%s (%s)\n' "$cpu" \
    "$([ -n "$cores" ] && printf ' (%s cores)' "$cores")" "$ram" "$os" "$(uname -m)"
}

size_to_bytes() { # 10k / 100k / 1m / 2048 -> bytes
  local s="$1" n unit
  n="${s%[kKmM]}"
  unit="${s#"$n"}"
  case "$unit" in
    k | K) echo $((n * 1024)) ;;
    m | M) echo $((n * 1024 * 1024)) ;;
    *) echo "$n" ;;
  esac
}

human_bytes() {
  awk -v b="$1" 'BEGIN {
    if (b >= 1048576) printf "%.1f MB", b/1048576;
    else if (b >= 1024) printf "%.1f KB", b/1024;
    else printf "%d B", b;
  }'
}

# /usr/bin/time flavor: BSD -l (bytes) vs GNU -v (kbytes); sets TIME_FLAG/TIME_RSS_SCALE, TIME_FLAG="" when neither works
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

# Time carta vs pandoc on one input and append a table row; --input gives both identical stdin under --shell=none.
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
  local x_mean x_sd p_mean p_sd runs
  x_mean=$(jq -r '.results[] | select(.command=="carta")  | .mean'   "$json")
  x_sd=$(jq   -r '.results[] | select(.command=="carta")  | .stddev' "$json")
  p_mean=$(jq -r '.results[] | select(.command=="pandoc") | .mean'   "$json")
  p_sd=$(jq   -r '.results[] | select(.command=="pandoc") | .stddev' "$json")
  runs=$(jq   -r '[.results[].times | length] | min'                 "$json")

  local x_rss p_rss
  x_rss=$(measure_rss "$input" $OX $xargs)
  p_rss=$(measure_rss "$input" $ORACLE $oargs)

  emit_row "$label" "$bytes" "$x_mean" "$x_sd" "$p_mean" "$p_sd" "$x_rss" "$p_rss"
  record_timing "$bytes" "$x_mean" "$x_sd" "$p_mean" "$p_sd" "$x_rss" "$p_rss" "$runs"
}

# Appends one timing row, labelled with the table it was printed under.
# Usage: record_timing <bytes> <carta_mean_s> <carta_sd_s> <pandoc_mean_s> <pandoc_sd_s> <carta_rss> <pandoc_rss> <runs>
record_timing() {
  [ -n "$BENCH_ROWS" ] || return 0
  jq -nc \
    --arg surface "$BENCH_SURFACE" \
    --arg group "$BENCH_GROUP" \
    --argjson bytes "$1" \
    --argjson carta_s "$2" --argjson carta_sd_s "$3" \
    --argjson pandoc_s "$4" --argjson pandoc_sd_s "$5" \
    --arg carta_rss "$6" --arg pandoc_rss "$7" \
    --argjson runs "$8" \
    '{
       kind: "timing", surface: $surface, group: $group, bytes: $bytes, runs: $runs,
       carta_ms:     ($carta_s     * 1000 * 100 | round / 100),
       carta_sd_ms:  ($carta_sd_s  * 1000 * 100 | round / 100),
       pandoc_ms:    ($pandoc_s    * 1000 * 100 | round / 100),
       pandoc_sd_ms: ($pandoc_sd_s * 1000 * 100 | round / 100),
       carta_rss:    (if $carta_rss  == "" then null else ($carta_rss  | tonumber) end),
       pandoc_rss:   (if $pandoc_rss == "" then null else ($pandoc_rss | tonumber) end)
     }' >>"$BENCH_ROWS"
}

# Appends the binary-size row the `size` surface reports.
record_binary() { # <carta_bytes> <pandoc_bytes>
  [ -n "$BENCH_ROWS" ] || return 0
  jq -nc --argjson carta "$1" --argjson pandoc "$2" \
    '{ kind: "binary", carta: $carta, pandoc: $pandoc }' >>"$BENCH_ROWS"
}

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

# Opens a result table and names the rows that follow. Usage: table_header <surface> <group>
table_header() {
  BENCH_SURFACE="$1"
  BENCH_GROUP="$2"
  echo
  echo "## $1 — $2"
  echo
  echo "| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |"
  echo "|--------|---------------------|---------------------|---------|------------|-----------|------------|"
}

# The table currently being filled, so recorded rows carry the heading's own labels.
BENCH_SURFACE=""
BENCH_GROUP=""

# Any benchmark error flips the suite return code; each surface exits with it.
SUITE_RC=0
note_err() { SUITE_RC=1; echo "ERR  $1: $2" >&2; }

sizes_list() { printf '%s\n' "$BENCH_SIZES" | tr ',' ' '; }

fixture_for() { # <fmt> <size>
  case "$1" in
    commonmark | markdown) echo "$FIXTURES/commonmark.$2.md" ;;
    html | html5) echo "$FIXTURES/html.$2.html" ;;
    native) echo "$FIXTURES/native.$2.native" ;;
    json) echo "$FIXTURES/json.$2.json" ;;
  esac
}

file_bytes() { wc -c <"$1" | tr -d ' '; }
