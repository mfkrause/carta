#!/usr/bin/env bash
# Highlight surface: live carta-vs-oracle byte diffs of the highlighting catalog, printed styles,
# rendered HTML/LaTeX per style (--wrap=none isolates colorized code), and the none/idiomatic modes.
set -uo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
require_tools

# built-in styles taken from the oracle so the sweep tracks what it ships
STYLES=$("$ORACLE" --list-highlight-styles 2>/dev/null)

# multi-grammar document: keywords, comments, strings, numbers, plus a line-numbered block
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

conf_reset "highlight-styles"
empty="$WORK/.hl.empty"
: >"$empty"
for style in $STYLES; do
  run_diff text "highlight/print-style/$style" "$empty" \
    "--print-highlight-style=$style" "--print-highlight-style=$style"
done
report highlight styles
tally_group

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
