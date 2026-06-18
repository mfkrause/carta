An image alone in a paragraph becomes a figure, and its alt text is the caption:

![A lone gull over the bay](gull.png)

Formatting in the alt text carries into the caption verbatim:

![a photo of `code` in the wild](snippet.png)

An empty alt leaves the paragraph untouched, even with a title:

![](spacer.png "decorative")

The title is never the caption; the alt text always is:

![the real caption](cover.png "tooltip, not caption")

Any extra inline disqualifies the paragraph, so this stays prose:

look at ![this](inline.png)

A link wrapping the image makes the link the sole inline, so no figure:

[![clickable](thumb.png)](https://example.com)

A figure nests inside a block quote:

> ![quoted scene](quote.png)

A loose list item is a paragraph, so its image-only entry becomes a figure:

- ![first slide](one.png)

- ![second slide](two.png)
