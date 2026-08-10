#let accent = emph
#let boxed = text.with(fill: red)
#let twice = (body) => [#body #body]

Renamed: #accent[slanted].
Fixed: #boxed[tinted].
Applied: #(emph)[direct], #(twice)[echo].
Passed: #((1, 2, 3).map(x => x + 1)).
