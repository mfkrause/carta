# Common harness primitives for the tools/ suites: repo path anchors, the pinned pandoc oracle
# location, and the normalization flags that put carta and pandoc on equivalent work. Sourced by each
# suite's lib.sh so the oracle contract is defined once. Idempotent.

[ -n "${TOOLS_SHARED_SOURCED:-}" ] && return 0
TOOLS_SHARED_SOURCED=1

SHARED_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SHARED_DIR/.." && pwd)"
ORACLE="$ROOT/.oracle/bin/pandoc"
CORPUS="$ROOT/corpus"

# pandoc flags that neutralize target nondeterminism carta does not reproduce, so both tools do the
# same work: HTML suppresses syntax highlighting and renders math via MathJax; LaTeX suppresses
# highlighting. Applied to the pandoc side only.
oracle_norm() {
  case "$1" in
    html | html5 | html4 | revealjs) echo "--syntax-highlighting=none --mathjax" ;;
    epub | epub2 | epub3) echo "--syntax-highlighting=none --mathjax" ;;
    latex | beamer) echo "--syntax-highlighting=none" ;;
  esac
}
