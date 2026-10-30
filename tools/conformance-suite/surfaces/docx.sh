#!/usr/bin/env bash
# Compare unpacked DOCX content after XML normalization. Usage: surfaces/docx.sh
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

# pin SOURCE_DATE_EPOCH so property dates match on both sides (carta defaults to 1 when unset)
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1}"

docx_skip_part() {
  case "$1" in
    word/styles.xml | word/settings.xml | word/webSettings.xml | word/fontTable.xml | \
      word/theme/theme1.xml | docProps/app.xml) return 0 ;;
  esac
  return 1
}

# Normalize XML attributes, whitespace, namespaces, anchors, geometry, and dates.
docx_canon() {
  python3 - "$1" <<'PY'
import sys, re
from pathlib import Path
import xml.etree.ElementTree as ET

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
# not content: heading anchors and inherited section geometry
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
    # an element holding only dropped nodes canonicalizes as genuinely empty
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
path = Path(sys.argv[1])
if path.name in {'footnotes.xml.rels', 'comments.xml.rels'}:
    part = path.parent.parent / path.name.removesuffix('.rels')
    used = {
        value for node in ET.parse(part).iter() for key, value in node.attrib.items()
        if key.startswith('{http://schemas.openxmlformats.org/officeDocument/2006/relationships}')
    }
    for relation in list(root):
        if relation.get('Id') not in used:
            root.remove(relation)
serialize(root, out)
s = ''.join(out)
# property dates are free; fold so only presence and placement compare
s = re.sub(r'\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ', 'TIMESTAMP', s)
s = re.sub(r'><', '>\n<', s)
sys.stdout.write(s + '\n')
PY
}

# Unused comment relationships do not change document content.
docx_parts() {
  python3 - "$1" <<'PY'
from pathlib import Path
import sys
import xml.etree.ElementTree as ET

root = Path(sys.argv[1])
comments = root / 'word/comments.xml'
used = comments.is_file() and any(
    key.startswith('{http://schemas.openxmlformats.org/officeDocument/2006/relationships}')
    for node in ET.parse(comments).iter() for key in node.attrib
)
for path in sorted(root.rglob('*')):
    if not path.is_file():
        continue
    relative = path.relative_to(root).as_posix()
    if relative == 'word/_rels/comments.xml.rels' and not used:
        continue
    print('./' + relative)
PY
}

docx_diff() {
  local odir="$1" xdir="$2" label="$3" rel bad="" d fl control
  fl=$(diff <(docx_parts "$odir") <(docx_parts "$xdir"))
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
    if [ -n "$d" ] && \
        control=$(python3 "$CONF_DIR/correctness.py" docx "$label/$(basename "$rel" .xml)" "$xdir/$rel"); then
      printf 'CONTROL %s\n' "$control" >>"$SURFACE_LOG"
      d=""
    fi
    [ -n "$d" ] && bad="${bad:+$bad
}--- $rel ---
$d"
  done < <(cd "$odir" && find . -type f | sed 's|^\./||' | sort)
  printf '%s' "$bad"
}

docx_case() {
  local label="$1" target="$2" input="$3" onorm="$4" work detail repro
  work="$WORK/docx/${label//\//-}"
  rm -rf "$work"
  mkdir -p "$work/o" "$work/x"
  repro="repro: $OX -f json -t $target $input -o out.docx  (then unzip)"
  # shellcheck disable=SC2086
  if ! "$ORACLE" -f json -t "$target" --syntax-highlighting=none $onorm "$input" -o "$work/o.docx" 2>/dev/null; then
    SKIP=$((SKIP + 1))
    return
  fi
  if ! "$OX" -f json -t "$target" --syntax-highlighting=none "$input" -o "$work/x.docx" 2>"$work/err"; then
    note_err "$label" "$repro
$(head -n 3 "$work/err")"
    return
  fi
  (cd "$work/o" && unzip -oq ../o.docx) 2>/dev/null
  (cd "$work/x" && unzip -oq ../x.docx) 2>/dev/null
  detail=$(docx_diff "$work/o" "$work/x" "$label")
  if [ -z "$detail" ]; then
    PASS=$((PASS + 1))
  else
    note_fail "$label" "$repro
$detail"
  fi
  rm -rf "$work"
}

run_docx() {
  local input feature stem
  conf_reset "docx"
  for input in "$CORPUS"/ast/*/*.json; do
    feature=$(basename "$(dirname "$input")")
    stem=$(basename "$input" .json)
    if is_excluded docx "$feature" "$stem"; then
      SKIP=$((SKIP + 1))
      continue
    fi
    docx_case "docx/$feature/$stem" docx "$input" ""
  done
  for input in "$CORPUS"/ast-ext/docx*/*.json; do
    [ -e "$input" ] || continue
    feature=$(basename "$(dirname "$input")")
    stem=$(basename "$input" .json)
    if is_excluded docx "$feature" "$stem"; then
      SKIP=$((SKIP + 1))
      continue
    fi
    docx_case "docx/$feature/$stem" "$feature" "$input" ""
  done
  CONTROLS=$(grep -c '^CONTROL ' "$SURFACE_LOG" || true)
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
