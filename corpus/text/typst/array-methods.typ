#let squares = ()
#{ for n in range(4) { squares.push(n * n) } }
Grown: #squares, #squares.len().

#let queue = (1, 2, 3)
#{ queue.insert(1, 9) }
Removed: #queue.remove(0), popped: #queue.pop(), left: #queue.

Folded: #((1, 2, 3).fold(0, (a, b) => a + b)), #((2, 3).product()).
Chosen: #((1, 2, 3).any(x => x > 2)), #((1, 2, 3).all(x => x > 0)), #((1, 2, 3).position(x => x == 2)).
Shaped: #((1, 2).zip((3, 4))), #(((1, 2), (3,)).flatten()), #((3, 1, 2).sorted(key: x => -x)).
