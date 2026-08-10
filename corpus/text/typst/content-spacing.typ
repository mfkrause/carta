A #link("https://example.com")[ linked body ] keeps the padding of its content.

A box holds #box[ its own spacing ] within the line.

An underlined #underline[ padded body ] sits beside plain words.

#quote(block: true, attribution: [ A. Author ])[ A spaced quotation. ]

A footnote body is trimmed #footnote[ Trimmed on both ends. ] here.

#figure(image("diagram.png"), caption: [ Figure caption. ])

Loop rounds join with their padding: #for value in (1, 2, 3) [ #value ].

A measurement of #(2 * 3em), a ratio of #(50.5%), and #(1.5e-2) exact.

#image("diagram.png", width: 1e3pt)

#let outer = "kept"

#{ let inner = "scoped" }

The binding #outer survives its statement.
