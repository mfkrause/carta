#let digits = regex("[0-9]+")
Matches: #"a12b345".matches(digits).len().
First: #"a12b345".find(digits), at #"a12b345".position(digits).
Split: #"a1b2c".split(digits).join("-").
Replaced: #"a1b2c".replace(digits, "#", count: 1).
