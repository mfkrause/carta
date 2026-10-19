#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ref_dir="$repo_root/.oracle"
version_file="$ref_dir/PANDOC_VERSION"
dest="$ref_dir/tests"
tag_file="$ref_dir/TESTS_TAG"

update=0
for arg in "$@"; do
  case "$arg" in
    --update) update=1 ;;
    -h | --help)
      sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "unknown arg: $arg" >&2
      exit 2
      ;;
  esac
done

[ -f "$version_file" ] || {
  echo "no pinned pandoc version found — run tools/install-pandoc.sh first" >&2
  exit 1
}
version="$(cat "$version_file")"

if [ -d "$dest/.git" ] && [ "$update" -eq 0 ] &&
  [ -f "$tag_file" ] && [ "$(cat "$tag_file")" = "$version" ]; then
  echo "pandoc test corpus for $version already present at $dest"
  exit 0
fi

rm -rf "$dest"
echo "Fetching pandoc $version test corpus (sparse, test/ only) ..." >&2
if ! git clone --quiet --filter=blob:none --sparse --depth 1 \
  --branch "$version" https://github.com/jgm/pandoc.git "$dest"; then
  echo "could not clone pandoc at tag '$version' — does the release tag exist?" >&2
  exit 1
fi

git -C "$dest" sparse-checkout set test

find "$dest" -name '*.hs' -delete
if [ -d "$dest/src" ]; then
  echo "unexpected: src/ present after sparse checkout — removing" >&2
  rm -rf "$dest/src"
fi

printf '%s\n' "$version" >"$tag_file"

command_tests="$(find "$dest/test/command" -name '*.md' 2>/dev/null | wc -l | tr -d ' ')"
echo "Fetched test corpus -> $dest/test  (command tests: ${command_tests:-0})"
echo "Reminder: read test/ data only — never any pandoc source."
