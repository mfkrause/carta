# Greedy paragraphs

In this dialect a paragraph runs on until a blank line stops it. A following
> line that on its own would open a block quote,
# a line that reads as a heading,
- a line shaped like a list item,
***
or a thematic break between two lines of prose, all fold into the paragraph as
ordinary text rather than starting a block of their own.

Only a blank line ends the paragraph. After one, each construct stands on its
own again:

> Now this is genuinely a block quote.

A fenced code block is the exception: it ends the paragraph even with no blank
line before it.
```
this text is code, not prose
```

A list opens normally once a blank line precedes it, and a sublist may open
under an item whatever number it starts from:

1. first item
   3. a sublist, opened despite starting at three
2. second item
