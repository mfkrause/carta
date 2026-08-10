#let level = 2

#if level > 1 { [High] } else { [Low] } level.

#let label = if level > 1 { "many" } else { "one" }
Chosen: #label.

#for n in (1, 2, 3) { [Item #n. ] }

#let total = 0
#let step = 0
#while step < 3 {
  step += 1
  total += step
}
Total #total after #step steps.

#{
  let scratch = 5
  scratch = scratch * 2
  [Scratch is #scratch.]
}

#let counted = 0
#for n in range(600) {
  counted += 1
}
Counted #counted values.
