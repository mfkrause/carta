#table(
  columns: 3,
  table.header([Name], [Kind], [Count]),
  [alpha], [first], [1],
  [beta], [second], [2],
  table.footer([total], [], [3]),
)

#table(
  columns: (auto, 1fr),
  align: (left, right),
  [left cell], [right cell],
)

#grid(
  columns: 2,
  [a], [b],
  [c], [d],
)
