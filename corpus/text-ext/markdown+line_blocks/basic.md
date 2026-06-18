A line block keeps every source line as its own line:

| The limerick packs laughs anatomical
| Into space that is quite economical.
|     But the good ones I've seen
|     So seldom are clean
| And the clean ones so seldom are comical.

Leading spaces are preserved as non-breaking spaces, so the indentation of the
inner lines above survives into the output.

Each line still parses as inlines, with *emphasis*, `code`, and
[links](https://example.com) all honored:

| **A stanza heading**
| a plain second line

A line consisting of only the bar is an empty line, and a following line with no
bar but leading whitespace continues the line above it, joined by one space:

| First physical line that keeps going onto
  a continuation folded back in
|
| The line after the empty one

A line block does not interrupt a paragraph, so a bar that follows ordinary text
on the next line stays part of the paragraph:

ordinary text
| not a line block here
