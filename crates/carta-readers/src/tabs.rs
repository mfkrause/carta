//! Tab expansion on a configured column grid.

use std::borrow::Cow;

/// Expands tabs to column stops, resetting at newlines. Other characters advance one column.
/// Zero preserves tabs.
pub(crate) fn expand_tabs(input: &str, tab_stop: usize) -> Cow<'_, str> {
    if tab_stop == 0 || !input.contains('\t') {
        return Cow::Borrowed(input);
    }
    let mut expanded = String::with_capacity(input.len());
    let mut column = 0;
    for character in input.chars() {
        if character == '\t' {
            let spaces = tab_stop - column % tab_stop;
            expanded.extend(std::iter::repeat_n(' ', spaces));
            column += spaces;
        } else {
            expanded.push(character);
            column = if character == '\n' { 0 } else { column + 1 };
        }
    }
    Cow::Owned(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_stops_reset_at_each_newline() {
        assert_eq!(
            expand_tabs("ab\tc\nx\ty\r\n\u{e9}\tz", 4),
            "ab  c\nx   y\r\n\u{e9}   z"
        );
    }

    #[test]
    fn zero_preserves_tab_characters() {
        assert_eq!(expand_tabs("a\tb\n\tc", 0), "a\tb\n\tc");
    }
}
