#show "wordmark": "carta"
The wordmark appears here: wordmark.

#show emph: strong
Now _emphasis_ reads as strong.

#show regex("[0-9]+"): [N]
Numbers 12 and 345 stand in.

#show heading.where(level: 2): it => [Section: #it.body]

== Second level

#show link: it => [(#it.body)]

A #link("https://example.com")[labelled link] here.

#show raw: it => [code #it.text]

An inline `snippet` in a line.

#show strong: none

A *bold run* drops out.

#show: body => [Wrapped: #body]
The document tail.
