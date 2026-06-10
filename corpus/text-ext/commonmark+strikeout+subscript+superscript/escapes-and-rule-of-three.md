An escaped \~~a~~ and \^a^ stay literal.

Run pairings whose lengths sum to a multiple of three only resolve when the
opener follows whitespace: ~a~~ resolves but x~a~~ and .~a~~ do not.

The same holds for carets: ^a^^ resolves while x^a^^ stays literal.
