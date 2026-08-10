#let name = "Typst"

Hello, #name!

#let double(x) = x * 2

Twice three is #double(3).

#if true [Shown when true.] else [Hidden.]

#for value in (1, 2, 3) [
  Item #value.
]

Sum: #(1 + 2 * 3).

#context [Read where it stands.]

Two plus two is #context 2 + 2.

#let total = 1 + 2 * 3
Total: #total.

#let flag = 2 > 1 and 3 <= 4
Flag: #flag.
