# Table captions

A caption sits in a paragraph of its own, set off from the table by a blank line.
A pipe table takes a caption written below it, introduced by a `Table:` marker; the
text after the marker becomes the caption and the colon is dropped:

| Fruit  | Price |
|:-------|------:|
| Apple  |  0.40 |
| Pear   |  0.55 |

Table: Prices at the market stall.

The marker may instead lead the table, in which case the blank line still separates
the two. Its first letter is the only one whose case is fixed, so a lowercase
`table:` reads just the same. Here a simple table follows its caption:

table: Ages recorded at the desk.

Name    Age
-----   ---
Ann      9
Ban     11

A lone colon is marker enough. Below a multiline table it opens a caption that may
run across several lines, each folding into the next:

--------------   -----------------------------------------------
Vehicle          Notes
--------------   -----------------------------------------------
Carriage         A wheeled vehicle, here standing in for any
                 cell whose text wraps onto a second line.

Sledge           A vehicle on runners, drawn over snow or ice.
--------------   -----------------------------------------------

: Conveyances of the period, with a caption long enough that it,
too, wraps across two lines of source.
