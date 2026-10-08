[ -n "${TOOLS_SHARED_SOURCED:-}" ] && return 0
TOOLS_SHARED_SOURCED=1

SHARED_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SHARED_DIR/.." && pwd)"
ORACLE="$ROOT/.oracle/bin/pandoc"
CORPUS="$ROOT/corpus"

# Normalize HTML math with MathJax; both tools keep syntax highlighting enabled.
oracle_norm() {
  case "$1" in
    html | html5 | html4 | revealjs) echo "--mathjax" ;;
    epub | epub2 | epub3) echo "--mathjax" ;;
    docx | odt | latex | beamer) echo "" ;;
  esac
}
