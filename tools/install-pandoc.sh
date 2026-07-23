#!/usr/bin/env bash
# Install a pinned pandoc binary as a black-box oracle into .oracle/ (gitignored): differential
# testing only, never read its source or commit it (clean-room rule, AGENTS.md).
# Usage: install-pandoc.sh [--update | --version=3.10]   (no arg: pinned, or latest stable first run)
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ref_dir="$repo_root/.oracle"
bin_dir="$ref_dir/bin"
version_file="$ref_dir/PANDOC_VERSION"

update=0
want_version="${PANDOC_VERSION:-}"
for arg in "$@"; do
  case "$arg" in
    --update) update=1 ;;
    --version=*) want_version="${arg#*=}" ;;
    -h | --help)
      sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "unknown arg: $arg" >&2
      exit 2
      ;;
  esac
done

resolve_latest() {
  curl -fsSL "https://api.github.com/repos/jgm/pandoc/releases/latest" |
    sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1
}

# Target version: explicit arg/env > pinned file (unless --update) > latest stable
if [ -n "$want_version" ]; then
  version="$want_version"
elif [ -f "$version_file" ] && [ "$update" -eq 0 ]; then
  version="$(cat "$version_file")"
else
  echo "Resolving latest stable pandoc release..." >&2
  version="$(resolve_latest)"
fi
[ -n "$version" ] || {
  echo "could not determine pandoc version" >&2
  exit 1
}

if [ "$update" -eq 0 ] && [ -x "$bin_dir/pandoc" ] &&
  [ -f "$version_file" ] && [ "$(cat "$version_file")" = "$version" ]; then
  echo "pandoc $version already installed at $bin_dir/pandoc"
  "$bin_dir/pandoc" --version | head -1
  exit 0
fi

os="$(uname -s)"
arch="$(uname -m)"
case "$os/$arch" in
  Darwin/arm64) asset="pandoc-$version-arm64-macOS.zip" kind=zip ;;
  Darwin/x86_64) asset="pandoc-$version-x86_64-macOS.zip" kind=zip ;;
  Linux/x86_64) asset="pandoc-$version-linux-amd64.tar.gz" kind=tar ;;
  Linux/aarch64 | Linux/arm64) asset="pandoc-$version-linux-arm64.tar.gz" kind=tar ;;
  *)
    echo "unsupported platform: $os/$arch" >&2
    exit 1
    ;;
esac

url="https://github.com/jgm/pandoc/releases/download/$version/$asset"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $asset ..." >&2
curl -fSL --progress-bar "$url" -o "$tmp/$asset"

echo "Extracting ..." >&2
mkdir -p "$tmp/extract"
if [ "$kind" = zip ]; then
  unzip -q "$tmp/$asset" -d "$tmp/extract"
else
  tar -xzf "$tmp/$asset" -C "$tmp/extract"
fi

src_bin="$(find "$tmp/extract" -type f -name pandoc | head -1)"
[ -n "$src_bin" ] || {
  echo "pandoc binary not found in archive" >&2
  exit 1
}

mkdir -p "$bin_dir"
install -m 0755 "$src_bin" "$bin_dir/pandoc"
printf '%s\n' "$version" >"$version_file"

# Record the JSON AST api-version; our serialization must match its major.minor.
api_version="$(printf '' | "$bin_dir/pandoc" -f markdown -t json 2>/dev/null |
  sed -n 's/.*"pandoc-api-version":\(\[[0-9,]*\]\).*/\1/p' | head -1)"
printf '%s\n' "${api_version:-unknown}" >"$ref_dir/API_VERSION"

echo "Installed pandoc $version -> $bin_dir/pandoc"
"$bin_dir/pandoc" --version | head -1
echo "pandoc-api-version: ${api_version:-unknown}"
