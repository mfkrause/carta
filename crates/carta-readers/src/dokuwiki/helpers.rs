//! Character-slice and line utilities shared across the reader's parsing stages.

/// Replace Windows and classic-Mac line endings with `\n` so the line-oriented scanner sees one
/// newline convention.
pub(super) fn normalize_newlines(input: &str) -> String {
    input.replace("\r\n", "\n").replace('\r', "\n")
}

/// Whether `chars` from index `i` begins with the characters of `needle`.
pub(super) fn matches_at(chars: &[char], i: usize, needle: &str) -> bool {
    needle
        .chars()
        .enumerate()
        .all(|(k, ch)| chars.get(i + k) == Some(&ch))
}

/// Count of leading space characters on a line.
pub(super) fn leading_spaces(line: &str) -> usize {
    line.chars().take_while(|&c| c == ' ').count()
}

/// The width of one tab stop, in columns. A tab advances to the next multiple of this width.
pub(super) const TAB_STOP: usize = 4;

/// The column at which a line's first non-whitespace character sits, counting a tab as the width to
/// the next tab stop.
pub(super) fn leading_columns(line: &str) -> usize {
    let mut col = 0;
    for c in line.chars() {
        match c {
            '\t' => col = (col / TAB_STOP + 1) * TAB_STOP,
            ' ' => col += 1,
            _ => break,
        }
    }
    col
}

/// The index just past the first occurrence of `needle` in `chars` at or after `from`.
pub(super) fn find_subsequence(chars: &[char], from: usize, needle: &str) -> Option<usize> {
    let len = needle.chars().count();
    (from..=chars.len().saturating_sub(len)).find(|&i| matches_at(chars, i, needle))
}

/// The number of consecutive `ch` at `pos`.
pub(super) fn run_length(chars: &[char], pos: usize, ch: char) -> usize {
    let mut n = 0;
    while chars.get(pos + n) == Some(&ch) {
        n += 1;
    }
    n
}

/// The character before `pos`, if any.
fn before_char(chars: &[char], pos: usize) -> Option<char> {
    pos.checked_sub(1).and_then(|p| chars.get(p)).copied()
}

/// Whether a character slice is empty or all whitespace.
pub(super) fn is_blank(chars: &[char]) -> bool {
    chars.iter().all(|c| c.is_whitespace())
}

/// Whether `pos` sits at a non-alphanumeric boundary (the start of a word for autolink purposes).
pub(super) fn boundary_before(chars: &[char], pos: usize) -> bool {
    before_char(chars, pos).is_none_or(|c| !c.is_alphanumeric())
}
