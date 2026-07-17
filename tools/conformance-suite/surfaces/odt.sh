#!/usr/bin/env bash
# ODT surface: exercise carta's ODT writer structurally against the oracle.
#
# Group `odt/odt` converts each corpus/ast case with both tools, unpacks the two OpenDocument
# packages, and diffs every part that carries document body content after canonicalizing the XML — the
# one comparison that survives the format's cosmetic freedom. Canonicalization (a small standard-library
# parser, no external tool) parses each part, sorts every element's attributes, drops insignificant
# inter-element whitespace while keeping leaf text under xml:space="preserve", assigns each namespace a
# fixed prefix, and re-serializes in document order. On top of that it folds the reproducible timestamp,
# normalizes the freely-chosen automatic list-style names to a positional token, and removes two design
# artifacts that are not body content — the scripting container and the frame graphic styles a document
# inherits from its styling — so the structural comparison sees only genuine divergences.
#
# The styling and packaging design each tool ships is its own and is not compared: the master styles,
# document metadata, the RDF manifest, the package manifest, and the mimetype marker are skipped. The
# document body (content.xml) is compared; embedded binary resources (images) are compared
# byte-for-byte, and the part file list is compared exactly. Cases excluded via corpus/exclusions.tsv
# are skipped and counted.
#
# Usage: surfaces/odt.sh
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

# Both tools stamp the package's metadata dates from SOURCE_DATE_EPOCH; pin it so the timestamps are
# reproducible and identical on both sides (carta uses 1 when it is unset).
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1}"

# Styling- and packaging-design parts: each tool's own look and container bookkeeping, not body content.
odt_skip_part() {
  case "$1" in
    styles.xml | meta.xml | manifest.rdf | META-INF/manifest.xml | mimetype) return 0 ;;
  esac
  return 1
}

# Canonicalize one XML part to a stable, comparable form on stdout. A whole-file parse (standard
# library only) sorts attributes, folds insignificant whitespace, pins namespace prefixes, drops the
# scripting container and frame graphic styles, folds automatic list-style names to a positional token,
# and normalizes the reproducible timestamp.
odt_canon() {
  python3 - "$1" <<'PY'
import sys, re
import xml.etree.ElementTree as ET

# Every namespace an ODT body part uses, each pinned to a fixed prefix so both sides serialize
# identically regardless of how the emitter declared them.
NS = {
    'urn:oasis:names:tc:opendocument:xmlns:office:1.0': 'office',
    'urn:oasis:names:tc:opendocument:xmlns:style:1.0': 'style',
    'urn:oasis:names:tc:opendocument:xmlns:text:1.0': 'text',
    'urn:oasis:names:tc:opendocument:xmlns:table:1.0': 'table',
    'urn:oasis:names:tc:opendocument:xmlns:drawing:1.0': 'draw',
    'urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0': 'fo',
    'http://www.w3.org/1999/xlink': 'xlink',
    'http://purl.org/dc/elements/1.1/': 'dc',
    'urn:oasis:names:tc:opendocument:xmlns:meta:1.0': 'meta',
    'urn:oasis:names:tc:opendocument:xmlns:datastyle:1.0': 'number',
    'urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0': 'svg',
    'http://www.w3.org/1998/Math/MathML': 'math',
    'urn:oasis:names:tc:opendocument:xmlns:manifest:1.0': 'manifest',
    'http://www.w3.org/XML/1998/namespace': 'xml',
}

STYLE = '{urn:oasis:names:tc:opendocument:xmlns:style:1.0}'
OFFICE = '{urn:oasis:names:tc:opendocument:xmlns:office:1.0}'
FAMILY = STYLE + 'family'
PRESERVE = '{http://www.w3.org/XML/1998/namespace}space'


# Not body content: the scripting container and the frame graphic styles chosen freely per document.
def drop(el):
    if not isinstance(el.tag, str):
        return True
    if el.tag == OFFICE + 'scripts':
        return True
    if el.tag == STYLE + 'style' and el.attrib.get(FAMILY) == 'graphic':
        return True
    return False


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
    if drop(el):
        return
    preserve = el.attrib.get(PRESERVE) == 'preserve'
    out.append('<' + qname(el.tag))
    for k, v in sorted((qname(k), v) for k, v in el.attrib.items()):
        out.append(' %s="%s"' % (k, esc_attr(v)))
    text = keep(el.text, preserve)
    kids = [c for c in el if not drop(c)]
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
# A metadata date is reproducible but free; fold it so only its presence and placement are compared.
s = re.sub(r'\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ', 'TIMESTAMP', s)
# The automatic style generated for each list is named freely; fold its two spellings to one token so
# only the list structure and the reference wiring, not the chosen name, are compared.
s = re.sub(r'Pandoc_5f_Numbering_5f_(\d+)', r'AUTOLIST_\1', s)
s = re.sub(r'"L(\d+)"', r'"AUTOLIST_\1"', s)
# One element per line keeps a divergence diff readable.
s = re.sub(r'><', '>\n<', s)
sys.stdout.write(s + '\n')
PY
}

# Diff one case's two unpacked packages; prints the differing parts, empty when they agree.
odt_diff() {
  local odir="$1" xdir="$2" rel bad="" d fl
  fl=$(diff <(cd "$odir" && find . -type f | sort) <(cd "$xdir" && find . -type f | sort))
  [ -n "$fl" ] && bad="file list differs:
$fl"
  while IFS= read -r rel; do
    odt_skip_part "$rel" && continue
    [ -f "$xdir/$rel" ] || continue
    case "$rel" in
      Pictures/*)
        cmp -s "$odir/$rel" "$xdir/$rel" || bad="${bad:+$bad
}binary part differs: $rel"
        continue ;;
    esac
    d=$(diff <(odt_canon "$odir/$rel") <(odt_canon "$xdir/$rel"))
    [ -n "$d" ] && bad="${bad:+$bad
}--- $rel ---
$d"
  done < <(cd "$odir" && find . -type f | sed 's|^\./||' | sort)
  printf '%s' "$bad"
}

# Convert one case with both tools under the given target spec, unpack the two packages, and diff them.
odt_case() {
  local label="$1" target="$2" input="$3" onorm="$4" work detail repro
  work="$WORK/odt/${label//\//-}"
  rm -rf "$work"
  mkdir -p "$work/o" "$work/x"
  repro="repro: $OX -f json -t $target $input -o out.odt  (then unzip)"
  # shellcheck disable=SC2086
  if ! "$ORACLE" -f json -t "$target" $onorm "$input" -o "$work/o.odt" 2>/dev/null; then
    SKIP=$((SKIP + 1))
    return
  fi
  if ! "$OX" -f json -t "$target" "$input" -o "$work/x.odt" 2>"$work/err"; then
    note_err "$label" "$repro
$(head -n 3 "$work/err")"
    return
  fi
  (cd "$work/o" && unzip -oq ../o.odt) 2>/dev/null
  (cd "$work/x" && unzip -oq ../x.odt) 2>/dev/null
  detail=$(odt_diff "$work/o" "$work/x")
  if [ -z "$detail" ]; then
    PASS=$((PASS + 1))
  else
    note_fail "$label" "$repro
$detail"
  fi
  rm -rf "$work"
}

run_odt() {
  local onorm input feature stem
  conf_reset "odt"
  onorm=$(oracle_norm odt)
  # Bare-format cases render with the default extension set.
  for input in "$CORPUS"/ast/*/*.json; do
    feature=$(basename "$(dirname "$input")")
    stem=$(basename "$input" .json)
    if is_excluded odt "$feature" "$stem"; then
      SKIP=$((SKIP + 1))
      continue
    fi
    odt_case "odt/$feature/$stem" odt "$input" "$onorm"
  done
  # Extension-toggle cases render with the spec their directory names (e.g. odt+empty_paragraphs).
  for input in "$CORPUS"/ast-ext/odt*/*.json; do
    [ -e "$input" ] || continue
    feature=$(basename "$(dirname "$input")")
    stem=$(basename "$input" .json)
    if is_excluded odt "$feature" "$stem"; then
      SKIP=$((SKIP + 1))
      continue
    fi
    odt_case "odt/$feature/$stem" "$feature" "$input" "$onorm"
  done
  report odt odt
  tally_group
}

group="${1:-all}"
case "$group" in
  all | "" | odt)
    require_tools
    command -v unzip >/dev/null 2>&1 || { echo "error: unzip not found on PATH" >&2; exit 1; }
    command -v python3 >/dev/null 2>&1 || { echo "error: python3 not found on PATH" >&2; exit 1; }
    run_odt ;;
  *)
    echo "unknown odt group: $group (want odt)" >&2
    exit 2 ;;
esac
exit "$SUITE_RC"
