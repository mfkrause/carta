# Paragraphs interrupted by an HTML block

A paragraph that runs straight into a block-level HTML element — with no blank
line between them — is interrupted as a block. The interrupted paragraph then
reads tight, rendering as plain inline content rather than a standalone
paragraph:

text before a div
<div>
content inside the div
</div>

The same tightening applies when the element keeps its tags as raw HTML around
its parsed content, as a section does:

text before a section
<section>
content inside the section
</section>

A blank line between the paragraph and the element leaves the paragraph loose,
so it stays a full paragraph:

text standing on its own

<div>
content after a blank line
</div>

Inside a block quote the rule is the same — the interrupted line reads tight:

> quoted text before
> <div>
> quoted content
> </div>
