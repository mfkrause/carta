# Indented simple-table headers

A header row need not begin at the same column as its ruling. Each column's
alignment is read from where the header text sits relative to that column's dash
run, so an indented header is measured against its own position rather than the
ruling's left margin. Here the header is shifted two columns in: every column's
dashes are flush with the header text on the right and reach past it on the
left, so the columns align right, right, and center:

  Right     Left     Center
-------   ------   ----------
     12     34        56

The ruling may instead be the indented line while the header stays flush left.
The first column's text then spills past its dash run on both sides and reads as
the default alignment, while a later column whose dashes overhang only on the
right reads as left:

Right     Left     Center
  -------   ------   ----------
     12     34        56
