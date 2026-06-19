# Simple tables

A header row sits above a dash ruling. Each column spans from one dash run to
the next, and the header text's offset within its run sets the alignment — flush
both sides stays default, free on the right reads as left, free on the left as
right, free on both as centered:

Name   City         Mid       Max
----   --------   ------   ------
Ann    Paris       mid        90
Bob    Rome        c           5

The body runs until a blank line, so a table needs no closing ruling. A trailing
ruling is allowed too, and is dropped rather than read as a row:

Item     Count
------   -------
spam     3
eggs     5
------   -------

With the header line omitted, a ruling opens the table and a matching ruling
closes it; the first row then fixes each column's alignment:

----------   ------   ----------
left          mid          right
more          x              y
----------   ------   ----------

A single-column table is written the same way, one ruling above the rows:

----------
alpha
beta
----------
