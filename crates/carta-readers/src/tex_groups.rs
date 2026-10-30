//! Index TeX groups while keeping escaped delimiters and comment text out of their nesting.

/// Visits characters backwards, skipping escaped characters except physical line breaks.
pub(crate) fn unescaped_reverse(
    characters: impl DoubleEndedIterator<Item = (usize, char)>,
) -> impl Iterator<Item = (usize, char)> {
    let mut characters = characters.rev().peekable();
    std::iter::from_fn(move || {
        loop {
            let (position, character) = characters.next()?;
            let mut escapes = 0usize;
            while characters
                .peek()
                .is_some_and(|(_, character)| *character == '\\')
            {
                characters.next();
                escapes += 1;
            }
            if escapes.is_multiple_of(2) || character == '\n' {
                return Some((position, character));
            }
        }
    })
}

/// Pairs group openers with the offset after each closing token.
pub(crate) fn group_pairs(
    characters: impl DoubleEndedIterator<Item = (usize, char)>,
    comments: bool,
) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    let mut closing = Vec::new();
    let (mut braces, mut brackets) = (None, None);
    let (mut line_braces, mut line_brackets) = (None, None);
    // Saved stack positions skip comments without rescanning or copying groups.
    for (position, character) in unescaped_reverse(characters) {
        match character {
            '\n' => (line_braces, line_brackets) = (braces, brackets),
            '%' if comments => (braces, brackets) = (line_braces, line_brackets),
            '}' => {
                closing.push((position + 1, braces, brackets));
                braces = Some(closing.len() - 1);
                brackets = None;
            }
            ']' => {
                closing.push((position + 1, brackets, None));
                brackets = Some(closing.len() - 1);
            }
            '{' => {
                if let Some(&(end, parent, outside)) = braces.and_then(|index| closing.get(index)) {
                    pairs.push((position, end));
                    braces = parent;
                    brackets = outside;
                } else {
                    brackets = None;
                }
            }
            '[' => {
                if let Some(&(end, parent, _)) = brackets.and_then(|index| closing.get(index)) {
                    pairs.push((position, end));
                    brackets = parent;
                }
            }
            _ => {}
        }
    }
    pairs
}
