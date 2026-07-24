//! Fixed-width lines, drawers, and horizontal rules.

pub(super) fn is_horizontal_rule(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 5 && t.chars().all(|c| c == '-')
}

pub(super) fn is_fixed_width(line: &str) -> bool {
    let t = line.trim_start();
    t == ":" || t.starts_with(": ")
}

pub(super) fn collect_fixed_width(lines: &[&str], start: usize) -> (String, usize) {
    let mut text = String::new();
    let mut i = start;
    while let Some(&line) = lines.get(i) {
        if !is_fixed_width(line) {
            break;
        }
        let t = line.trim_start();
        let content = t
            .strip_prefix(": ")
            .or_else(|| t.strip_prefix(':'))
            .unwrap_or("");
        text.push_str(content);
        text.push('\n');
        i += 1;
    }
    (text, i - start)
}

/// The drawer name of a `:NAME:` line (excluding `:END:`), or `None` when the line is not a drawer.
pub(super) fn drawer_open(line: &str) -> Option<String> {
    let t = line.trim();
    let inner = t.strip_prefix(':')?.strip_suffix(':')?;
    if inner.is_empty()
        || inner.contains(':')
        || inner.eq_ignore_ascii_case("END")
        || !inner
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '@' | '#' | '%'))
    {
        return None;
    }
    Some(inner.to_owned())
}

pub(super) fn collect_drawer<'a>(lines: &[&'a str], start: usize) -> (Vec<&'a str>, usize) {
    let mut inner = Vec::new();
    let mut i = start + 1;
    while let Some(&line) = lines.get(i) {
        if line.trim().eq_ignore_ascii_case(":END:") {
            i += 1;
            break;
        }
        inner.push(line);
        i += 1;
    }
    (inner, i - start)
}
