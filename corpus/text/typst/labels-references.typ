= Introduction <intro>

See @intro for context.

#figure(
  image("plot.png"),
  caption: [A labelled figure.],
) <fig-plot>

Reference to @fig-plot as well.

A #ref(<intro>) written as a call.

A #link(<intro>)[link to a label] and a bare #link(<intro>).

A #ref(<intro>, supplement: [Chapter]) with its own supplement.

A #ref(label("intro")) built from a label call.
