#!/usr/bin/env bash
# Vendor the CommonMark spec (CC-BY-SA 4.0, an allowed source of truth): its examples are markdown inputs only;
# expected output comes from the pinned oracle, never the spec's HTML. Re-run only to bump SPEC_VERSION.
set -euo pipefail

SPEC_VERSION="0.31.2"
REPO="commonmark/commonmark-spec"
DEST_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/vendor/commonmark"

base="https://raw.githubusercontent.com/${REPO}/${SPEC_VERSION}"

mkdir -p "$DEST_DIR"
echo "Fetching CommonMark spec ${SPEC_VERSION} into ${DEST_DIR}"
curl -fsSL "${base}/spec.txt" -o "${DEST_DIR}/spec.txt"
curl -fsSL "${base}/LICENSE" -o "${DEST_DIR}/LICENSE"
printf '%s\n' "$SPEC_VERSION" > "${DEST_DIR}/VERSION"

examples="$(grep -c '^`\{32\} example' "${DEST_DIR}/spec.txt" || true)"
echo "Vendored spec.txt (${examples} examples), LICENSE, VERSION=${SPEC_VERSION}"
