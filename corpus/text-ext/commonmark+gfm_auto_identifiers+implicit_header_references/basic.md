# Some Heading

A shortcut reference resolves to the heading: see [Some Heading]. Case folds, so
[SOME HEADING] resolves too, and runs of whitespace collapse in [Some    Heading].

A full reference keeps its own text in [the intro][Some Heading], while a
collapsed reference reuses the label in [Some Heading][]. An image reference
points at the same anchor: ![Some Heading].

# Heading with *emphasis*

The label is matched on its source, so [Heading with *emphasis*] resolves but
the unmarked [Heading with emphasis] stays literal.

# Foo: Bar

Punctuation belongs to the label: [Foo: Bar] resolves while [Foo Bar] does not.

# Defined Twice

# Defined Twice

A repeated heading is reachable only through the first: [Defined Twice].

# Linked Elsewhere

[Linked Elsewhere]: https://example.com/elsewhere

An explicit definition outranks the heading: [Linked Elsewhere].

A reference may precede its heading: [Later Section] resolves all the same.

# Later Section
