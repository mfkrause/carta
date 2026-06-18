Numbered examples are written with an `@` marker in place of a digit, so that prose can refer back
to a particular example by name. The marker takes the same three shapes an ordinary enumerator does:

(@) a bare example, wrapped in parentheses

@. another, closed by a period

@) and a third, closed by a single parenthesis

Each example is numbered from a single counter that runs across the whole document, so the three
above are numbered one, two, and three even though each stands in its own short list.

A marker may carry a label between the `@` and its delimiter. The label is set off here so that
later text can point at the example:

(@apple) An apple is a pome fruit.

(@pear) A pear is too.

Writing the label again inside a sentence resolves to that example number: the apple is (@apple) and
the pear is (@pear). A reference may appear before the example it names, so this forward pointer to
(@quince) still resolves once the example below is read.

(@quince) A quince is closely related to both.

Ordinary numbered lists keep their own counters and leave the example counter untouched:

1. first plain item
2. second plain item

(@medlar) The medlar rounds out the family.

So the running examples are apple (@apple), pear (@pear), quince (@quince), and medlar (@medlar),
numbered in the order their labels first appear.

A label can be reused. The second mention does not take a fresh number; it points back at the first,
while a bare reference such as @apple needs no parentheses and prints just the bare number.

(@apple) The same label, mentioned a second time.

References are skipped where text is taken verbatim: inside a code span `(@apple)` stays literal,
while in *emphasised text (@pear)* the reference still resolves.
