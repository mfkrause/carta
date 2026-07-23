//! Tab expansion on a fixed column grid, shared by the line-oriented readers.

/// Expand every tab in `line` to spaces, advancing to the next multiple of `tab_stop`. Each non-tab
/// character counts as one column.
pub(crate) fn expand_tabs(line: &str, tab_stop: usize) -> String {
    let mut out = String::with_capacity(line.len());
    let mut col = 0;
    for ch in line.chars() {
        if ch == '\t' {
            let next = (col / tab_stop + 1) * tab_stop;
            while col < next {
                out.push(' ');
                col += 1;
            }
        } else {
            out.push(ch);
            col += 1;
        }
    }
    out
}
