#!/usr/bin/env bash
# Compare EPUB archives and validate with EPUBCheck. Usage: surfaces/epub.sh [epub3|epub2|epubcheck]
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

# Pin SOURCE_DATE_EPOCH so both tools stamp the same reproducible timestamp (carta defaults to 1).
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1}"

# The shared AST cases name pic.png without carrying the image bytes.
RESOURCE_DIR="$WORK/epub-resources"
mkdir -p "$RESOURCE_DIR"
python3 - "$CORPUS/ast/image-dimensions/image-embedded-png.json" "$RESOURCE_DIR/pic.png" <<'PY'
import base64
import json
from pathlib import Path
import sys

document = json.loads(Path(sys.argv[1]).read_text())
source = document['blocks'][0]['c'][-1]['c'][2][0]
Path(sys.argv[2]).write_bytes(base64.b64decode(source.split(',', 1)[1]))
PY

# Normalize free values and spec-valid placeholders; see docs/STATUS.md.
epub_norm() {
  sed -e 's/content="pandoc"/content="carta"/' \
      -e 's|urn:uuid:[0-9A-Fa-f-]\{36\}|urn:uuid:NORM|g' \
      -e 's/lang="\(C\|en-US\)"/lang="L"/g' \
      -e 's@<dc:language>\(C\|en-US\)</dc:language>@<dc:language>L</dc:language>@g' \
      -e '/<dc:title id="epub-title-1">UNTITLED<\/dc:title>/d' \
      -e 's|<a href="\([^"]*\)">UNTITLED</a>|<a href="\1" />|g' \
      -e '/<div class="titlepage"><\/div>/d'
}

epub_diff() {
  local odir="$1" xdir="$2" label="$3" rel bad="" d fl control
  fl=$(diff <(cd "$odir" && find . -type f | sort) <(cd "$xdir" && find . -type f | sort))
  [ -n "$fl" ] && bad="file list differs:
$fl"
  while IFS= read -r rel; do
    case "$rel" in */stylesheet*.css) continue ;; esac
    [ -f "$xdir/$rel" ] || continue
    case "$rel" in
      *.png | *.jpg | *.jpeg | *.gif | *.svg | *.otf | *.ttf | *.woff | *.woff2)
        cmp -s "$odir/$rel" "$xdir/$rel" || bad="${bad:+$bad
}binary entry differs: $rel"
        continue ;;
    esac
    d=$(diff <(epub_norm <"$odir/$rel") <(epub_norm <"$xdir/$rel"))
    if [ -n "$d" ] && [ "$rel" = EPUB/text/ch001.xhtml ] && \
        control=$(python3 "$CONF_DIR/correctness.py" epub "$label" "$xdir/$rel"); then
      printf 'CONTROL %s\n' "$control" >>"$SURFACE_LOG"
      d=""
    fi
    [ -n "$d" ] && bad="${bad:+$bad
}--- $rel ---
$d"
  done < <(cd "$odir" && find . -type f | sed 's|^\./||' | sort)
  printf '%s' "$bad"
}

run_dialect() {
  local dialect="$1" input feature stem label work detail repro
  conf_reset "epub-$dialect"
  for input in "$CORPUS"/ast/*/*.json; do
    feature=$(basename "$(dirname "$input")")
    stem=$(basename "$input" .json)
    label="epub/$dialect/$feature/$stem"
    if is_excluded "$dialect" "$feature" "$stem"; then
      SKIP=$((SKIP + 1))
      continue
    fi
    work="$WORK/epub/$dialect/$feature-$stem"
    rm -rf "$work"
    mkdir -p "$work/o" "$work/x"
    repro="repro: $OX -f json -t $dialect $input -o out.epub  (then unzip)"
    # shellcheck disable=SC2086
    if ! "$ORACLE" -f json -t "$dialect" --resource-path="$RESOURCE_DIR" "$input" -o "$work/o.epub" 2>/dev/null; then
      SKIP=$((SKIP + 1))
      continue
    fi
    if ! "$OX" -f json -t "$dialect" --resource-path="$RESOURCE_DIR" "$input" -o "$work/x.epub" 2>"$work/err"; then
      note_err "$label" "$repro
$(head -n 3 "$work/err")"
      continue
    fi
    (cd "$work/o" && unzip -oq ../o.epub) 2>/dev/null
    (cd "$work/x" && unzip -oq ../x.epub) 2>/dev/null
    detail=$(epub_diff "$work/o" "$work/x" "$label")
    if [ -z "$detail" ]; then
      PASS=$((PASS + 1))
    else
      note_fail "$label" "$repro
$detail"
    fi
    rm -rf "$work"
  done
  CONTROLS=$(grep -c '^CONTROL ' "$SURFACE_LOG" || true)
  report epub "$dialect"
  tally_group
}

# Subprocesses report through STATUS files and exported variables.
epubcheck_one() {
  local dialect="$1" input="$2" feature stem label out res slug status=0 kind detail
  feature=$(basename "$(dirname "$input")")
  stem=$(basename "$input" .json)
  label="epub/epubcheck/$dialect/$feature/$stem"
  slug="$dialect-$feature-$stem"
  out="$EC_WORK/$slug.epub"
  if ! "$OX" -f json -t "$dialect" --resource-path="$RESOURCE_DIR" "$input" -o "$out" 2>/dev/null; then
    printf 'ERR\t%s\tcarta failed to convert\n' "$label" >"$EC_RESDIR/$slug"
    return
  fi
  res=$("$EC_JAVA" -jar "$EC_JAR" "$out" 2>&1) || status=$?
  rm -f "$out"
  if [ "$status" -ne 0 ] || printf '%s' "$res" | grep -qE 'ERROR|FATAL'; then
    kind=FAIL
    detail=$(printf '%s' "$res" | grep -E 'ERROR|FATAL' | head -n 4 | tr '\n' '~')
    if [ -z "$detail" ]; then
      kind=ERR
      detail=$(printf '%s' "$res" | head -n 4 | tr '\n' '~')
    fi
    printf '%s\t%s\t%s\n' "$kind" "$label" \
      "${detail:-EPUBCheck exited with status $status}" >"$EC_RESDIR/$slug"
  else
    printf 'PASS\t%s\t\n' "$label" >"$EC_RESDIR/$slug"
  fi
}

run_epubcheck() {
  conf_reset "epub-epubcheck"
  local jar java resdir ecwork jobs f status label detail unresolved
  jar="${EPUBCHECK_JAR:-$ROOT/.oracle/epubcheck/epubcheck.jar}"
  java="${JAVA_BIN:-java}"
  if ! command -v "$java" >/dev/null 2>&1 || [ ! -f "$jar" ]; then
    report epub epubcheck
    echo "  note: EPUBCheck unavailable — set JAVA_BIN and EPUBCHECK_JAR (or install to .oracle/epubcheck/)" >&2
    return
  fi
  # Unresolved offline resources yield references that EPUBCheck rejects.
  unresolved=" common/image-external common/link-in-link "
  resdir="$WORK/epub/epubcheck-results"
  ecwork="$WORK/epub/epubcheck-work"
  rm -rf "$resdir" "$ecwork"
  mkdir -p "$resdir" "$ecwork"
  jobs="$WORK/epub/epubcheck-jobs"
  : >"$jobs"
  local dialect input feature stem
  for dialect in epub3 epub2; do
    for input in "$CORPUS"/ast/*/*.json; do
      feature=$(basename "$(dirname "$input")")
      stem=$(basename "$input" .json)
      case "$unresolved" in *" $feature/$stem "*)
        SKIP=$((SKIP + 1))
        continue ;;
      esac
      printf '%s\t%s\n' "$dialect" "$input" >>"$jobs"
    done
  done
  export OX RESOURCE_DIR EC_JAVA="$java" EC_JAR="$jar" EC_WORK="$ecwork" EC_RESDIR="$resdir"
  export -f epubcheck_one
  local slots
  slots=$(( $(command -v nproc >/dev/null 2>&1 && nproc || echo 4) ))
  [ "$slots" -gt 6 ] && slots=6
  [ "$slots" -lt 1 ] && slots=1
  xargs -P "$slots" -a "$jobs" -n1 -I{} bash -c \
    'IFS=$'"'"'\t'"'"' read -r d i <<<"{}"; epubcheck_one "$d" "$i"' 2>/dev/null
  for f in "$resdir"/*; do
    [ -f "$f" ] || continue
    status=$(cut -f1 "$f")
    label=$(cut -f2 "$f")
    detail=$(cut -f3 "$f")
    case "$status" in
      PASS) PASS=$((PASS + 1)) ;;
      FAIL) note_fail "$label" "$(printf '%s' "$detail" | tr '~' '\n')" ;;
      ERR) note_err "$label" "$detail" ;;
    esac
  done
  rm -rf "$resdir" "$ecwork" "$jobs"
  report epub epubcheck
  tally_group
}

group="${1:-all}"
case "$group" in
  epub3)
    require_tools
    command -v unzip >/dev/null 2>&1 || { echo "error: unzip not found on PATH" >&2; exit 1; }
    run_dialect epub3 ;;
  epub2)
    require_tools
    command -v unzip >/dev/null 2>&1 || { echo "error: unzip not found on PATH" >&2; exit 1; }
    run_dialect epub2 ;;
  epubcheck)
    run_epubcheck ;;
  all | "")
    require_tools
    command -v unzip >/dev/null 2>&1 || { echo "error: unzip not found on PATH" >&2; exit 1; }
    run_dialect epub3
    run_dialect epub2
    run_epubcheck ;;
  *)
    echo "unknown epub group: $group (want epub3|epub2|epubcheck)" >&2
    exit 2 ;;
esac
exit "$SUITE_RC"
