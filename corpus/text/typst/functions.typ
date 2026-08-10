#let greet(name, greeting: "Hello") = [#greeting, #name!]
#greet("Ada")
#greet("Bob", greeting: "Hi")

#let pair = ("left", "right")
#let join(a, b) = [#a and #b]
#join(..pair)

#let tally(..entries) = [#entries.pos().len() positional, #entries.named().at("unit")]
#tally(1, 2, 3, unit: "cm")

#let head(first, ..rest) = [#first leads #rest.pos().len() others]
#head("one", "two", "three")

#table(
  columns: 2,
  align: (column, row) => if column == 0 { left } else { right },
  [A], [B],
  [C], [D],
)

#list(marker: n => [dot], [alpha], [beta])
