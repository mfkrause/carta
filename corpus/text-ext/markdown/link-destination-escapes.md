A bracketed destination may hold spaces and other unsafe characters, which are
percent-encoded: [a spaced link](<two words>) and [a braced one](<a{b}c>).

An image destination is encoded the same way: ![diagram](<my figure.png>).

An angle autolink encodes its destination while showing it verbatim:
<https://example.com/a^b>.

A reference destination is encoded when the link resolves: [ref][r].

[r]: <spaced ref.html>

An already-encoded destination is left as it stands, never doubled:
[encoded](<a%20b>). A backslash and non-ASCII text pass through untouched:
[kept](a\b/café).
