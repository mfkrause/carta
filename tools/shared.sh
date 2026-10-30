[ -n "${TOOLS_SHARED_SOURCED:-}" ] && return 0
TOOLS_SHARED_SOURCED=1

SHARED_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SHARED_DIR/.." && pwd)"
ORACLE="$ROOT/.oracle/bin/pandoc"
CORPUS="$ROOT/corpus"
