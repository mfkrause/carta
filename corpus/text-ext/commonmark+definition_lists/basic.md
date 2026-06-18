A definition list pairs a term with one or more definitions. The term sits on
its own line and each definition follows on a line opened by a colon:

apple
: A common pome fruit.

orange
: A citrus fruit.

A single term may carry several definitions, and the marker may be a colon or a
tilde:

water
: A clear liquid.
~ Essential for life.

When the term and its first definition are separated by a blank line, the list
is loose and every definition is wrapped in its own paragraph:

planet

: A body that orbits a star.

A definition can hold more than one block. Indenting the continuation under the
marker keeps it inside the same definition:

essay
: The opening paragraph introduces the idea.

  The second paragraph develops it.

Block-level content nests inside a definition when indented to the marker's
content column, so a definition can contain a list of its own:

shapes
: The basic ones are:

    - circle
    - square

A term folds several physical lines into one when they are written without a
blank line between them:

first line
second line
: Both lines above make up the term.

Inline markup is parsed in both the term and the definition, so *emphasis*,
`code`, and [links](https://example.com) all render:

*compile*
: Translate source into a `binary`, as described [here](https://example.com).
