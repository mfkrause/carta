//! Small character-slice helpers shared by the block and inline layers.

/// The separators this format recognises: ASCII space, tab, and line feed. Code points that Unicode
/// classes as whitespace (a no-break space, em space, form feed, vertical tab, and the like) are
/// ordinary characters here, kept inside the surrounding word rather than splitting it.
pub(super) fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n')
}

pub(super) fn matches_at(chars: &[char], pos: usize, needle: &str) -> bool {
    needle
        .chars()
        .enumerate()
        .all(|(k, ch)| chars.get(pos + k) == Some(&ch))
}

/// Whether a parameterless block macro begins at `pos`. These tokens introduce a block wherever they
/// occur, so they end any paragraph that runs into them.
pub(super) fn bare_block_macro_at(chars: &[char], pos: usize) -> bool {
    matches_at(chars, pos, "{code}")
        || matches_at(chars, pos, "{noformat}")
        || matches_at(chars, pos, "{quote}")
        || matches_at(chars, pos, "{panel}")
}

pub(super) fn find_token(chars: &[char], from: usize, token: &str) -> Option<usize> {
    let token_len = token.chars().count();
    let upper = chars.len().saturating_sub(token_len);
    (from..=upper).find(|&k| matches_at(chars, k, token))
}

pub(super) fn slice_to_string(chars: &[char], start: usize, end: usize) -> String {
    chars.get(start..end).unwrap_or_default().iter().collect()
}

/// Trims leading and trailing whitespace from `start..end`, returning the narrowed range.
pub(super) fn trim(chars: &[char], start: usize, end: usize) -> (usize, usize) {
    let mut s = start;
    while s < end && chars.get(s).is_some_and(|&c| is_space(c)) {
        s += 1;
    }
    let mut e = end;
    while e > s && chars.get(e - 1).is_some_and(|&c| is_space(c)) {
        e -= 1;
    }
    (s, e)
}

/// The end of `start..end` with trailing whitespace removed, leaving any leading whitespace in place.
pub(super) fn trim_end(chars: &[char], start: usize, end: usize) -> usize {
    let mut e = end;
    while e > start && chars.get(e - 1).is_some_and(|&c| is_space(c)) {
        e -= 1;
    }
    e
}
