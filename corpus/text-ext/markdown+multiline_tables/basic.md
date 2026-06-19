# Multiline tables

A dash ruling opens the table, a header line sits below it, and a second ruling
closes the header off. Rows then follow, separated by blank lines, so one row may
span several physical lines that fold together into a single cell. The header's
text fixes each column's alignment, and every column keeps the fractional width
the ruling lays out:

--------   --------   --------
Left         Center      Right
--------   --------   --------
a            b            c

spread       over         two
             lines
--------   --------   --------

With the header omitted, a ruling opens the table and a matching ruling closes
it; the first row then fixes each column's alignment instead. Cells still wrap
across physical lines until the blank that ends the row:

-------------   -------------------------------------------------
Carriage        A wheeled vehicle, here standing in for any cell
                whose text wraps onto a second line.

Sledge          A vehicle on runners, drawn over snow or ice.
-------------   -------------------------------------------------

A single-column table is written the same way, one ruling above the rows and one
below, with blank lines splitting the rows:

----------
alpha

beta
gamma
----------
