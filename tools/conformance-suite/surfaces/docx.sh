#!/usr/bin/env bash
# DOCX surface: exercise carta's DOCX writer structurally against the oracle.
#
# Group `docx/docx` converts each corpus/ast case with both tools, unpacks the two Office Open XML
# packages, and diffs every part that carries document content after canonicalizing the XML — the one
# comparison that survives the format's cosmetic freedom. Canonicalization (a small standard-library
# parser, no external tool) parses each part, sorts every element's attributes, drops insignificant
# inter-element whitespace while keeping leaf text under xml:space="preserve", assigns each namespace a
# fixed prefix, and re-serializes in document order. On top of that it folds the few values a writer
# picks freely (a reproducible timestamp) and removes two design artifacts that are not content — the
# navigation bookmarks around headings and the section geometry (page size, margins) a document
# inherits from its styling — so the structural comparison sees only genuine divergences.
#
# The styling design each tool ships is its own and is not compared: the styles, settings, web
# settings, font table, theme, and the extended-properties summary are skipped. Every other part —
# the content types, the relationship graphs, the document body, footnotes, comments, list numbering,
# and the core/custom properties — is compared; embedded binary resources (images) are compared
# byte-for-byte. Cases excluded via corpus/exclusions.tsv are skipped and counted.
#
# Usage: surfaces/docx.sh
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

# Both tools stamp the package's property dates from SOURCE_DATE_EPOCH; pin it so the timestamps are
# reproducible and identical on both sides (carta uses 1 when it is unset).
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1}"

# Styling-design parts: each tool's own look, not compared.
docx_skip_part() {
  case "$1" in
    word/styles.xml | word/settings.xml | word/webSettings.xml | word/fontTable.xml | \
      word/theme/theme1.xml | docProps/app.xml) return 0 ;;
  esac
  return 1
}

# Canonicalize one XML part to a stable, comparable form on stdout. A whole-file parse (standard
# library only) sorts attributes, folds insignificant whitespace, pins namespace prefixes, strips the
# heading bookmarks and the inherited section geometry, and normalizes the reproducible timestamp.
docx_canon() {
  python3 - "$1" <<'PY'
import sys, re
import xml.etree.ElementTree as ET

# Every namespace a DOCX part uses, each pinned to a fixed prefix so both sides serialize identically
# regardless of how the emitter declared them.
NS = {
    'http://schemas.openxmlformats.org/wordprocessingml/2006/main': 'w',
    'http://schemas.openxmlformats.org/officeDocument/2006/math': 'm',
    'http://schemas.openxmlformats.org/officeDocument/2006/relationships': 'r',
    'urn:schemas-microsoft-com:office:office': 'o',
    'urn:schemas-microsoft-com:vml': 'v',
    'urn:schemas-microsoft-com:office:word': 'w10',
    'http://schemas.openxmlformats.org/drawingml/2006/main': 'a',
    'http://schemas.openxmlformats.org/drawingml/2006/picture': 'pic',
    'http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing': 'wp',
    'http://schemas.openxmlformats.org/package/2006/content-types': 'ct',
    'http://schemas.openxmlformats.org/package/2006/relationships': 'rel',
    'http://purl.org/dc/elements/1.1/': 'dc',
    'http://purl.org/dc/terms/': 'dcterms',
    'http://purl.org/dc/dcmitype/': 'dcmitype',
    'http://schemas.openxmlformats.org/package/2006/metadata/core-properties': 'cp',
    'http://www.w3.org/2001/XMLSchema-instance': 'xsi',
    'http://schemas.openxmlformats.org/officeDocument/2006/extended-properties': 'ep',
    'http://schemas.openxmlformats.org/officeDocument/2006/custom-properties': 'cust',
    'http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes': 'vt',
    'http://www.w3.org/XML/1998/namespace': 'xml',
}

W = '{http://schemas.openxmlformats.org/wordprocessingml/2006/main}'
PRESERVE = '{http://www.w3.org/XML/1998/namespace}space'
# Not content: heading navigation anchors and the section geometry inherited from the styling.
DROP = {W + 'bookmarkStart', W + 'bookmarkEnd', W + 'sectPr'}


def qname(tag):
    if isinstance(tag, str) and tag.startswith('{'):
        uri, local = tag[1:].split('}', 1)
        p = NS.get(uri)
        return p + ':' + local if p else local
    return tag


def esc_text(s):
    return s.replace('&', '&amp;').replace('<', '&lt;').replace('>', '&gt;')


def esc_attr(s):
    return esc_text(s).replace('"', '&quot;')


def is_ws(s):
    return s is None or s.strip() == ''


def keep(text, preserve):
    return text if (text and (not is_ws(text) or preserve)) else ''


def serialize(el, out):
    if not isinstance(el.tag, str) or el.tag in DROP:
        return
    preserve = el.attrib.get(PRESERVE) == 'preserve'
    out.append('<' + qname(el.tag))
    for k, v in sorted((qname(k), v) for k, v in el.attrib.items()):
        out.append(' %s="%s"' % (k, esc_attr(v)))
    text = keep(el.text, preserve)
    # An element left with nothing but dropped nodes canonicalizes the same as a genuinely empty one,
    # so the ignored bookmarks and section geometry stay invisible to the comparison.
    kids = [c for c in el if isinstance(c.tag, str) and c.tag not in DROP]
    if not text and not kids:
        out.append('/>')
        return
    out.append('>')
    out.append(esc_text(text))
    for c in el:
        serialize(c, out)
        out.append(esc_text(keep(c.tail, preserve)))
    out.append('</' + qname(el.tag) + '>')


try:
    root = ET.parse(sys.argv[1]).getroot()
except Exception as e:  # a malformed part must surface as a divergence, not a crash
    sys.stdout.write('CANON-PARSE-ERROR: %s\n' % e)
    sys.exit(0)

out = []
serialize(root, out)
s = ''.join(out)
# A property date is reproducible but free; fold it so only its presence and placement are compared.
s = re.sub(r'\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ', 'TIMESTAMP', s)
# One element per line keeps a divergence diff readable.
s = re.sub(r'><', '>\n<', s)
sys.stdout.write(s + '\n')
PY
}

# Diff one case's two unpacked packages; prints the differing parts, empty when they agree.
docx_diff() {
  local odir="$1" xdir="$2" rel bad="" d fl
  fl=$(diff <(cd "$odir" && find . -type f | sort) <(cd "$xdir" && find . -type f | sort))
  [ -n "$fl" ] && bad="file list differs:
$fl"
  while IFS= read -r rel; do
    docx_skip_part "$rel" && continue
    [ -f "$xdir/$rel" ] || continue
    case "$rel" in
      word/media/*)
        cmp -s "$odir/$rel" "$xdir/$rel" || bad="${bad:+$bad
}binary part differs: $rel"
        continue ;;
    esac
    d=$(diff <(docx_canon "$odir/$rel") <(docx_canon "$xdir/$rel"))
    [ -n "$d" ] && bad="${bad:+$bad
}--- $rel ---
$d"
  done < <(cd "$odir" && find . -type f | sed 's|^\./||' | sort)
  printf '%s' "$bad"
}

# Convert one case with both tools under the given target spec, unpack the two packages, and diff
# them. The oracle takes normalization flags; carta takes the bare spec.
docx_case() {
  local label="$1" target="$2" input="$3" onorm="$4" work detail repro
  work="$WORK/docx/${label//\//-}"
  rm -rf "$work"
  mkdir -p "$work/o" "$work/x"
  repro="repro: $OX -f json -t $target $input -o out.docx  (then unzip)"
  # shellcheck disable=SC2086
  if ! "$ORACLE" -f json -t "$target" $onorm "$input" -o "$work/o.docx" 2>/dev/null; then
    SKIP=$((SKIP + 1))
    return
  fi
  if ! "$OX" -f json -t "$target" "$input" -o "$work/x.docx" 2>"$work/err"; then
    note_err "$label" "$repro
$(head -n 3 "$work/err")"
    return
  fi
  (cd "$work/o" && unzip -oq ../o.docx) 2>/dev/null
  (cd "$work/x" && unzip -oq ../x.docx) 2>/dev/null
  detail=$(docx_diff "$work/o" "$work/x")
  if [ -z "$detail" ]; then
    PASS=$((PASS + 1))
  else
    note_fail "$label" "$repro
$detail"
  fi
  rm -rf "$work"
}

run_docx() {
  local onorm input feature stem
  conf_reset "docx"
  onorm=$(oracle_norm docx)
  # Bare-format cases render with the default extension set.
  for input in "$CORPUS"/ast/*/*.json; do
    feature=$(basename "$(dirname "$input")")
    stem=$(basename "$input" .json)
    if is_excluded docx "$feature" "$stem"; then
      SKIP=$((SKIP + 1))
      continue
    fi
    docx_case "docx/$feature/$stem" docx "$input" "$onorm"
  done
  # Extension-toggle cases render with the spec their directory names (e.g. docx+native_numbering).
  for input in "$CORPUS"/ast-ext/docx*/*.json; do
    [ -e "$input" ] || continue
    feature=$(basename "$(dirname "$input")")
    stem=$(basename "$input" .json)
    if is_excluded docx "$feature" "$stem"; then
      SKIP=$((SKIP + 1))
      continue
    fi
    docx_case "docx/$feature/$stem" "$feature" "$input" "$onorm"
  done
  report docx docx
  tally_group
}

group="${1:-all}"
case "$group" in
  all | "" | docx)
    require_tools
    command -v unzip >/dev/null 2>&1 || { echo "error: unzip not found on PATH" >&2; exit 1; }
    command -v python3 >/dev/null 2>&1 || { echo "error: python3 not found on PATH" >&2; exit 1; }
    run_docx ;;
  *)
    echo "unknown docx group: $group (want docx)" >&2
    exit 2 ;;
esac
exit "$SUITE_RC"
