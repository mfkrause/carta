#!/usr/bin/env bash
# Generate benchmark inputs into $FIXTURES (gitignored). Idempotent: existing files are reused unless
# BENCH_REGEN=1. Produces, per size in BENCH_SIZES:
#   commonmark.<size>.md      seed.md repeated to ~size              (reader/e2e input)
#   html|native|json.<size>.* the seed input rendered by carta      (reader input, derived)
#   ast.<size>.json           curated corpus AST cycled to ~size     (writer input)
# Plus fixed near-empty startup inputs. Derived inputs are carta's own output and never committed.
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

regen="${BENCH_REGEN:-0}"
fresh() { [ "$regen" != "1" ] && [ -s "$1" ]; }

seed_bytes=$(wc -c <"$SEED" | tr -d ' ')

# Build the curated AST base once; emit base blocks JSON and record its size/length.
base_blocks="$FIXTURES/.ast-base.json"
if ! fresh "$base_blocks"; then
  files=""
  for rel in $WRITER_AST_FILES; do files="$files $CORPUS/ast/$rel.json"; done
  # shellcheck disable=SC2086
  jq -s 'map(.blocks) | add' $files >"$base_blocks"
fi
base_len=$(jq 'length' "$base_blocks")
base_bytes=$(wc -c <"$base_blocks" | tr -d ' ')
api_src="$CORPUS/ast/common/paragraph.json"

gen_commonmark() { # <out> <target_bytes>
  local out="$1" target="$2" copies
  fresh "$out" && return 0
  copies=$(( (target + seed_bytes - 1) / seed_bytes ))
  [ "$copies" -lt 1 ] && copies=1
  : >"$out"
  local i=0
  while [ "$i" -lt "$copies" ]; do cat "$SEED" >>"$out"; i=$((i + 1)); done
}

gen_ast() { # <out> <target_bytes>
  local out="$1" target="$2" per n
  fresh "$out" && return 0
  per=$(( base_bytes / base_len ))
  [ "$per" -lt 1 ] && per=1
  n=$(( (target + per - 1) / per ))
  [ "$n" -lt 1 ] && n=1
  jq -n --argjson b "$(cat "$base_blocks")" --argjson n "$n" \
    --argjson ver "$(jq '.["pandoc-api-version"]' "$api_src")" \
    '{ "pandoc-api-version": $ver, meta: {}, blocks: ([range($n)] | map($b[. % ($b|length)])) }' \
    >"$out"
}

derive() { # <in> <fmt> <out>
  fresh "$3" && return 0
  "$OX" -f commonmark -t "$2" <"$1" >"$3"
}

for size in $(sizes_list); do
  bytes=$(size_to_bytes "$size")
  md="$FIXTURES/commonmark.$size.md"
  gen_commonmark "$md" "$bytes"
  derive "$md" html   "$FIXTURES/html.$size.html"
  derive "$md" native "$FIXTURES/native.$size.native"
  derive "$md" json   "$FIXTURES/json.$size.json"
  gen_ast "$FIXTURES/ast.$size.json" "$bytes"
done

# Near-empty startup inputs.
startup_md="$FIXTURES/startup.md"
if ! fresh "$startup_md"; then printf 'A startup probe paragraph.\n' >"$startup_md"; fi
derive "$startup_md" json "$FIXTURES/startup.ast.json"

echo "fixtures ready in $FIXTURES" >&2
