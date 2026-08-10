#let classify(n) = {
  if n < 0 { return "negative" }
  if n == 0 { return "zero" }
  "positive"
}
Signs: #classify(-4), #classify(0), #classify(7).

#let first-even(numbers) = {
  for n in numbers {
    if calc.rem(n, 2) == 0 { return n }
  }
  return -1
}
First even: #first-even((3, 5, 8, 10)).

#let capped = 0
#for n in range(100) {
  if n > 4 { break }
  capped += n
}
Capped sum #capped.

#let odds = 0
#for n in range(10) {
  if calc.rem(n, 2) == 0 { continue }
  odds += 1
}
Counted #odds odd values.

#let rounds = 0
#while true {
  rounds += 1
  if rounds >= 3 { break }
}
Ran #rounds rounds.

#let quiet() = {
  return // nothing follows
}
Quiet: #quiet()done.

#let commented(n) = {
  return /* the count */ n
}
Commented: #commented(2).
