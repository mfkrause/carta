#let width = 3; #let height = 4
Area: #(width * height).

#{ let negate = (n) => -n; negate(width) } is negative.

#let empty = ()
Empty: #empty.len(), #(-width), #(0 - height).
