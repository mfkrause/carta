#show "wordmark": "carta"
#show emph: strong
#show regex("[0-9]+"): [N]
#show heading.where(level: 2): it => [Section: #it.body]
#show: body => [Wrapped: #body]

A wordmark, _emphasis_, and 42 here.

== Second level
