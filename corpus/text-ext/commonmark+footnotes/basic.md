A reference like this[^1] becomes a note, and the definition is lifted out
of the body.

[^1]: The note content is a full block: it can hold *emphasis* and `code`.

A label may repeat a reference[^note] and resolve every time[^note] to the
same content.

[^note]: Shared between both references.

Definitions can span several blocks when their continuation lines are
indented four columns:

Long note[^long] here.

[^long]: The first paragraph of the note.

    A second paragraph, still part of the note.

    > Even a block quote belongs to it.

A definition may be referenced before it appears[^later] in the source.

[^later]: Order of definition does not matter.

An undefined reference [^missing] stays literal text, falling back to the
ordinary bracket rules.

Inside a note's own body a reference to another note[^outer] does not nest.

[^outer]: This note mentions [^inner] without nesting it.

[^inner]: The inner note's own content.

Labels are matched case-insensitively and with collapsed whitespace, so
[^Folded Label] finds its definition.

[^folded   label]: Resolved despite the differing case and spacing.
