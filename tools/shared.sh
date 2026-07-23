[ -n "${TOOLS_SHARED_SOURCED:-}" ] && return 0
TOOLS_SHARED_SOURCED=1

SHARED_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SHARED_DIR/.." && pwd)"
ORACLE="$ROOT/.oracle/bin/pandoc"
CORPUS="$ROOT/corpus"

# pandoc flags that neutralize target nondeterminism carta does not reproduce, so both tools do the
# same work: HTML renders math via MathJax. Syntax highlighting is left on; carta highlights code
# blocks to parity, so both sides colorize. Applied to the pandoc side only.
oracle_norm() {
  case "$1" in
    html | html5 | html4 | revealjs) echo "--mathjax" ;;
    epub | epub2 | epub3) echo "--mathjax" ;;
    docx | odt | latex | beamer) echo "" ;;
  esac
}
