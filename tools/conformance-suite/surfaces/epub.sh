#!/usr/bin/env bash
# EPUB surface: exercise carta's EPUB writer two ways — structurally against the oracle, and against
# the EPUB specification with EPUBCheck.
#
# Groups `epub/epub3` and `epub/epub2` convert each corpus/ast case with both tools, unpack the two
# archives, and diff every text entry after normalizing the few values a converter chooses freely
# (the generator name, the content-hash identifier, the C/en-US language tag) and folding three
# deliberate, documented spec-validity deviations (see docs/STATUS.md → EPUB):
#   - a `dc:title` is always emitted (a placeholder for an untitled work); a package without one is
#     invalid, so the placeholder line is folded away for the comparison.
#   - an untitled section's navigation anchor carries placeholder text; an empty anchor is invalid,
#     so the placeholder is folded back to the empty form.
#   - an untitled EPUB 2 title page carries a placeholder block; an empty XHTML 1.1 body is invalid,
#     so the placeholder block is folded away.
# The default stylesheet each tool ships is its own design and is not compared; embedded binary
# resources (images, fonts) are compared byte-for-byte. Cases excluded via corpus/exclusions.tsv are
# skipped and counted.
#
# Group `epub/epubcheck` validates every archive carta emits with EPUBCheck, expecting zero errors
# and zero fatals. A fresh JVM is spawned per archive, so the group runs its checks in parallel and
# is the slowest of the suite. It is skipped entirely when a Java runtime or the EPUBCheck jar is
# unavailable (set JAVA_BIN and EPUBCHECK_JAR, or install the jar to .oracle/epubcheck/). Two classes
# of case are skipped, since neither reflects a writer defect (see docs/STATUS.md → EPUB):
#   - a source that names a resource no offline converter can resolve — a remote image, an absent
#     local image, or a link to a nonexistent target — since no writer can embed what it cannot fetch;
#   - under EPUB 2 only, content with no valid XHTML 1.1 form (a start attribute, a mark or u element,
#     a block-level table caption, an empty table, a task-list checkbox) — a legacy-format limitation
#     EPUB 3's XHTML5 content model does not have.
#
# Usage: surfaces/epub.sh [epub3|epub2|epubcheck]
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

# Both tools stamp the EPUB's modification time from SOURCE_DATE_EPOCH; pin it so the timestamp is
# reproducible and identical on both sides (carta uses 1 when it is unset).
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1}"

# Values a converter picks freely, then the three documented validity folds, so the structural
# comparison sees only genuine divergences. Applied to both sides.
epub_norm() {
  sed -e 's/content="pandoc"/content="carta"/' \
      -e 's|urn:uuid:[0-9A-Fa-f-]\{36\}|urn:uuid:NORM|g' \
      -e 's/lang="\(C\|en-US\)"/lang="L"/g' \
      -e 's@<dc:language>\(C\|en-US\)</dc:language>@<dc:language>L</dc:language>@g' \
      -e '/<dc:title id="epub-title-1">UNTITLED<\/dc:title>/d' \
      -e 's|<a href="\([^"]*\)">UNTITLED</a>|<a href="\1" />|g' \
      -e '/<div class="titlepage"><\/div>/d'
}

# Diff one case's two unpacked archives; prints the differing entries, empty when they agree.
epub_diff() {
  local odir="$1" xdir="$2" rel bad="" d fl
  # Both tools lay out the same container: the file sets must match (the stylesheet aside).
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
    [ -n "$d" ] && bad="${bad:+$bad
}--- $rel ---
$d"
  done < <(cd "$odir" && find . -type f | sed 's|^\./||' | sort)
  printf '%s' "$bad"
}

# The structural differential for one dialect (epub3|epub2).
run_dialect() {
  local dialect="$1" onorm input feature stem label work detail repro
  conf_reset "epub-$dialect"
  onorm=$(oracle_norm "$dialect")
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
    if ! "$ORACLE" -f json -t "$dialect" $onorm "$input" -o "$work/o.epub" 2>/dev/null; then
      SKIP=$((SKIP + 1))
      continue
    fi
    if ! "$OX" -f json -t "$dialect" "$input" -o "$work/x.epub" 2>"$work/err"; then
      note_err "$label" "$repro
$(head -n 3 "$work/err")"
      continue
    fi
    (cd "$work/o" && unzip -oq ../o.epub) 2>/dev/null
    (cd "$work/x" && unzip -oq ../x.epub) 2>/dev/null
    detail=$(epub_diff "$work/o" "$work/x")
    if [ -z "$detail" ]; then
      PASS=$((PASS + 1))
    else
      note_fail "$label" "$repro
$detail"
    fi
    rm -rf "$work"
  done
  report epub "$dialect"
  tally_group
}

# Validate one archive carta emits for one dialect with EPUBCheck, writing a single tab-separated
# result line (STATUS<TAB>label<TAB>detail) to the aggregation directory. Runs in a subshell under
# xargs, so it communicates only through that file and the exported environment.
epubcheck_one() {
  local dialect="$1" input="$2" feature stem label out res slug
  feature=$(basename "$(dirname "$input")")
  stem=$(basename "$input" .json)
  label="epub/epubcheck/$dialect/$feature/$stem"
  slug="$dialect-$feature-$stem"
  out="$EC_WORK/$slug.epub"
  if ! "$OX" -f json -t "$dialect" "$input" -o "$out" 2>/dev/null; then
    printf 'ERR\t%s\tcarta failed to convert\n' "$label" >"$EC_RESDIR/$slug"
    return
  fi
  res=$("$EC_JAVA" -jar "$EC_JAR" "$out" 2>&1)
  rm -f "$out"
  if printf '%s' "$res" | grep -qE 'ERROR|FATAL'; then
    # Fold the multi-line message list onto one line (~) so it survives the single result line.
    printf 'FAIL\t%s\t%s\n' "$label" \
      "$(printf '%s' "$res" | grep -E 'ERROR|FATAL' | head -n 4 | tr '\n' '~')" >"$EC_RESDIR/$slug"
  else
    printf 'PASS\t%s\t\n' "$label" >"$EC_RESDIR/$slug"
  fi
}

# The EPUBCheck validity group across both dialects.
run_epubcheck() {
  conf_reset "epub-epubcheck"
  local jar java resdir ecwork jobs f status label detail unresolved legacy_epub2
  jar="${EPUBCHECK_JAR:-$ROOT/.oracle/epubcheck/epubcheck.jar}"
  java="${JAVA_BIN:-java}"
  if ! command -v "$java" >/dev/null 2>&1 || [ ! -f "$jar" ]; then
    report epub epubcheck
    echo "  note: EPUBCheck unavailable — set JAVA_BIN and EPUBCHECK_JAR (or install to .oracle/epubcheck/)" >&2
    return
  fi
  # Cases whose source names a resource no offline writer can resolve — a remote image, an absent
  # local image, or a link to a nonexistent target; both tools emit a dangling reference EPUBCheck
  # rejects, in either dialect (see docs/STATUS.md → EPUB).
  unresolved=" common/image-external common/image-inline common/link-in-link figure/figure-captioned figure/figure-no-alt figure/figure-with-dims image-dimensions/image-inline-dims "
  # Cases whose content has no valid XHTML 1.1 form — a start attribute on a list, a mark or u
  # element, block content in a table caption, an empty table, a bare tfoot, or a task-list checkbox.
  # EPUBCheck rejects these under EPUB 2 only; both tools emit them, a legacy-format limitation of
  # XHTML 1.1 that EPUB 3's XHTML5 content model does not share (see docs/STATUS.md → EPUB).
  legacy_epub2=" common/ordered-start common/span-semantic common/underline-smallcaps table/table-caption-blocks table/table-empty table/table-foot task-list/checkboxes "
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
      if [ "$dialect" = epub2 ]; then
        case "$legacy_epub2" in *" $feature/$stem "*)
          SKIP=$((SKIP + 1))
          continue ;;
        esac
      fi
      printf '%s\t%s\n' "$dialect" "$input" >>"$jobs"
    done
  done
  export OX EC_JAVA="$java" EC_JAR="$jar" EC_WORK="$ecwork" EC_RESDIR="$resdir"
  export -f epubcheck_one
  # A fresh JVM per archive dominates the cost, so validate in parallel; each job reports through its
  # own result file, tallied sequentially below.
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
