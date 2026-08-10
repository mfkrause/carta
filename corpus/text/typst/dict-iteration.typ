#let sizes = (small: 1, large: 3)
#for (name, size) in sizes [
  - #name is #size
]

#for entry in sizes [#entry.at(0)/#entry.at(1) ]
