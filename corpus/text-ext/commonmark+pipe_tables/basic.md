| Fruit | Colour | Notes |
| ----- | :----: | -----: |
| apple | red | crisp and tart |
| lemon | yellow | very sour |

Alignments come only from the delimiter colons:

| left | centre | right |
| :--- | :----: | ----: |
| a | b | c |

A table needs no outer pipes, and cells parse as inlines:

name | detail
--- | ---
*emphasis* and `code` | a [link](https://example.com)
a literal x \| y pipe | plain text

Short rows pad on the right and wide rows are truncated:

| one | two | three |
| --- | --- | --- |
| 1 | 2 |
| 1 | 2 | 3 | 4 |

A header and delimiter alone make a table with no body:

| empty | body |
| ----- | ---- |

Once a table is under way, a row whose first cell starts like a list
marker stays a table row rather than opening a list:

| key | value |
| --- | ----- |
| - dash | kept in the table |
| plus | also kept |
