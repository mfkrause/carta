//! Column-width measurement: the display width of a character or string, approximated without a
//! Unicode width table. Consumed by whichever text writers are enabled, so unused-item warnings are
//! allowed here rather than gated per item.
#![allow(dead_code)]

/// Display width of a string in columns, summed over its characters.
pub(crate) fn display_width(text: &str) -> usize {
    text.chars().map(char_width).sum()
}

/// Display width of a character: zero for common combining marks and controls, two for wide East
/// Asian characters, one otherwise. A self-contained column-width approximation.
pub(crate) fn char_width(ch: char) -> usize {
    let code = ch as u32;
    if is_control(code) {
        return 0;
    }
    if code < 0x0300 {
        return 1;
    }
    if is_zero_width(code) {
        return 0;
    }
    if is_wide(code) { 2 } else { 1 }
}

/// C0 controls, DEL, and C1 controls occupy no display columns.
fn is_control(code: u32) -> bool {
    code < 0x20 || (0x7F..=0x9F).contains(&code)
}

fn is_zero_width(code: u32) -> bool {
    matches!(code,
        0x0300..=0x036F
        | 0x0483..=0x0489
        | 0x0591..=0x05BD
        | 0x0610..=0x061A
        | 0x064B..=0x065F
        | 0x0670
        | 0x06D6..=0x06DC
        | 0x06DF..=0x06E4
        | 0x0E31
        | 0x0E34..=0x0E3A
        | 0x1AB0..=0x1AFF
        | 0x1DC0..=0x1DFF
        | 0x200B..=0x200F
        | 0x20D0..=0x20FF
        | 0xFE00..=0xFE0F
        | 0xFE20..=0xFE2F
    )
}

/// Whether a character occupies two display columns: the wide and fullwidth East Asian ranges.
pub(crate) fn is_wide(code: u32) -> bool {
    matches!(code,
        0x1100..=0x115F
        | 0x2329 | 0x232A
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE19
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1B000..=0x1B2FF
        | 0x1F200..=0x1F2FF
        | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x3FFFD
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_width_classifies_columns() {
        assert_eq!(char_width('a'), 1);
        assert_eq!(char_width('é'), 1);
        assert_eq!(char_width('Ї'), 1);
        assert_eq!(char_width('\n'), 0);
        assert_eq!(char_width('\t'), 0);
        assert_eq!(char_width('\u{7F}'), 0);
        assert_eq!(char_width('\u{85}'), 0);
        assert_eq!(char_width('\u{0301}'), 0);
        assert_eq!(char_width('\u{200B}'), 0);
        assert_eq!(char_width('\u{4E00}'), 2);
        assert_eq!(char_width('\u{FF21}'), 2);
        assert_eq!(char_width('\u{1F600}'), 2);
    }

    #[test]
    fn display_width_sums_characters() {
        assert_eq!(display_width(""), 0);
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("a\u{4E00}b"), 4);
        assert_eq!(display_width("e\u{0301}"), 1);
    }
}
