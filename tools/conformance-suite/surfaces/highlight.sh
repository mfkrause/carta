#!/usr/bin/env bash
# Highlight surface: differential parity of the syntax-highlighting command surface and rendered output.
#
# Four groups, each a live carta-vs-oracle diff (nothing committed):
#
#   - catalog:  `--list-highlight-languages` and `--list-highlight-styles` must byte-match.
#   - styles:   `--print-highlight-style <name>` must byte-match for every built-in style.
#   - render:   a multi-language document, colorized with each built-in style, must byte-match in HTML
#               and LaTeX. HTML uses `--wrap=none` so the comparison isolates the colorized code from
#               the writer's unrelated line-filling of surrounding markup.
#   - modes:    the `none` and `idiomatic` presentation modes, and explicit line numbering, must match.
#
# Emits one `RESULT highlight <group>` line per group; non-zero exit on any fail/err.
#
# Usage: surfaces/highlight.sh
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require_tools

# The built-in styles, taken from the oracle so the sweep tracks whatever it ships.
STYLES=$("$ORACLE" --list-highlight-styles 2>/dev/null)

# A document exercising several languages and token kinds: keywords, comments, strings, numbers in a
# few different grammars, plus a line-numbered block.
DOC="$WORK/.hl.md"
cat >"$DOC" <<'EOF'
```python
def f(x): return x + 1  # comment
```

```rust
fn main() { let s = "hi"; }
```

```c
int x = 0x1F; /* hex */
```

```haskell
main = putStrLn "hi"
```

```{.python .numberLines startFrom="5"}
a = 1
b = 2
```
EOF

# --- catalog: the language and style listings ---
conf_reset "highlight-catalog"
"$ORACLE" --list-highlight-languages >"$WORK/.hl.olang" 2>/dev/null
"$OX" --list-highlight-languages >"$WORK/.hl.xlang" 2>"$WORK/.hl.err"
if detail=$(compare_text "$WORK/.hl.olang" "$WORK/.hl.xlang"); then
  PASS=$((PASS + 1))
else
  note_fail "highlight/list-languages" "$detail"
fi
"$ORACLE" --list-highlight-styles >"$WORK/.hl.osty" 2>/dev/null
"$OX" --list-highlight-styles >"$WORK/.hl.xsty" 2>"$WORK/.hl.err"
if detail=$(compare_text "$WORK/.hl.osty" "$WORK/.hl.xsty"); then
  PASS=$((PASS + 1))
else
  note_fail "highlight/list-styles" "$detail"
fi
report highlight catalog
tally_group

# --- styles: the printed JSON theme for each built-in style ---
conf_reset "highlight-styles"
empty="$WORK/.hl.empty"
: >"$empty"
for style in $STYLES; do
  run_diff text "highlight/print-style/$style" "$empty" \
    "--print-highlight-style=$style" "--print-highlight-style=$style"
done
report highlight styles
tally_group

# --- render: colorized HTML and LaTeX for each built-in style ---
conf_reset "highlight-render"
for style in $STYLES; do
  run_diff text "highlight/html/$style" "$DOC" \
    "-f markdown -t html --wrap=none --highlight-style=$style" \
    "-f markdown -t html --wrap=none --highlight-style=$style"
  run_diff text "highlight/latex/$style" "$DOC" \
    "-f markdown -t latex --highlight-style=$style" \
    "-f markdown -t latex --highlight-style=$style"
done
report highlight render
tally_group

# --- modes: none, idiomatic, and standalone preambles ---
conf_reset "highlight-modes"
run_diff text "highlight/none/html" "$DOC" \
  "-f markdown -t html --wrap=none --no-highlight" \
  "-f markdown -t html --wrap=none --no-highlight"
run_diff text "highlight/none/latex" "$DOC" \
  "-f markdown -t latex --no-highlight" \
  "-f markdown -t latex --no-highlight"
run_diff text "highlight/idiomatic/latex" "$DOC" \
  "--listings -f markdown -t latex" \
  "-f markdown -t latex --syntax-highlighting=idiomatic"
run_diff text "highlight/idiomatic/beamer" "$DOC" \
  "--listings -f markdown -t beamer" \
  "-f markdown -t beamer --syntax-highlighting=idiomatic"
report highlight modes
tally_group

exit "$SUITE_RC"
