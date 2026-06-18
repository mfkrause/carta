A colon fence with a bare word names a single class:

::: warning
Heed this paragraph.
:::

A brace spec carries an identifier, classes, and key/value pairs:

::: {#alert .danger .boxed role=note}
The block keeps every attribute.
:::

Fences nest, and the inner block closes before the outer one:

::: outer
Outer opening.

::: inner
Inner body.
:::

Outer closing.
:::

A longer opening fence needs at least as many colons to close, so a
shorter run inside it is ordinary text:

:::: wide
A line with ::: three colons stays in the block.
::::

The content is parsed as blocks, so lists and quotes work inside:

::: examples
- first
- second

> A quote within the division.
:::

An indented opening fence re-bases its content to that column:

  ::: shifted
  Still a paragraph, not a code block.
  :::
