# Grid tables

A header row, set off by an `=` divider whose colons fix each column's
alignment, with widths drawn from the border spans:

+---------------+:-------------+-------------:+
| Fruit         | Note         | Quantity     |
+===============+:============:+=============:+
| Bananas       | ripe         | 12           |
+---------------+--------------+--------------+
| Oranges       | seedless     | 5            |
+---------------+--------------+--------------+

A table with no divider is all body:

+--------+--------+
| left   | right  |
+--------+--------+
| a      | b      |
+--------+--------+

Cells hold block content and span multiple lines. A blank line inside a cell
makes it loose, so its text renders as paragraphs:

+---------------+---------------+
| Bullet list:  | One           |
|               |               |
| - first       | Two           |
| - second      |               |
+---------------+---------------+

Adjacent borders frame an empty row, and a `Table:` caption attaches to the
table above it:

+-----+-----+
|  x  |  y  |
+=====+=====+
|  1  |  2  |
+-----+-----+
|  3  |  4  |
+-----+-----+

Table: a small grid with a caption

A second `=` divider before a `=` closing border marks a footer row:

+-------+--------+
| Item  | Price  |
+=======+========+
| Eggs  | 5      |
+-------+--------+
| Spam  | 3      |
+=======+========+
| Total | 8      |
+=======+========+
