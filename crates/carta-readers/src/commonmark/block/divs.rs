//! Fenced div closing: matching a colon fence against the open div stack.

use super::{Extension, Kind, Parser, div_close_fence};

impl Parser {
    /// If the current line closes an open fenced div, close that div and everything nested inside it
    /// (a colon fence preempts even an open code block) and return `true`. It is honored only when
    /// the descent reached the innermost open div, so a fence shallower than a still-open nested div
    /// stays ordinary content (which an enclosing div may then hold) rather than closing that div or
    /// an ancestor. `div_path` holds the divs matched on this line, innermost last, each paired with
    /// the line as it stood at that div's indentation.
    pub(super) fn close_fenced_div(
        &mut self,
        container: usize,
        div_path: &[(usize, String)],
    ) -> bool {
        if !self.extensions.contains(Extension::FencedDivs) {
            return false;
        }
        let Some((inner, inner_line)) = div_path.last() else {
            return false;
        };
        if self.innermost_open_div() != Some(*inner) {
            return false;
        }
        let Some(count) = div_close_fence(inner_line) else {
            return false;
        };
        let Some(target) = self.div_close_target(div_path, count) else {
            return false;
        };
        let tip = self.deepest_open(container);
        let stop = self.parent(target);
        self.close_chain(tip, stop);
        true
    }

    /// The innermost open fenced div anywhere in the tree, or `None` when none is open.
    pub(super) fn innermost_open_div(&self) -> Option<usize> {
        let mut node = self.deepest_open(0);
        loop {
            if matches!(self.kind(node), Some(Kind::FencedDiv(..))) {
                return Some(node);
            }
            let parent = self.parent(node);
            if parent == node {
                return None;
            }
            node = parent;
        }
    }

    /// Choose which fenced div a closing run of `count` colons shuts. The matched divs are read
    /// innermost first, each paired with the closing line as it stood at that div's indentation; the
    /// first div long enough to be closed by `count` colons is the target. A div the closing line
    /// sits more than three columns into is unreachable and, with every div outside it indented at
    /// least as far, ends the search: the line is ordinary content rather than a fence.
    pub(super) fn div_close_target(
        &self,
        div_path: &[(usize, String)],
        count: usize,
    ) -> Option<usize> {
        for (node, line) in div_path.iter().rev() {
            let leading = line.len() - line.trim_start_matches(' ').len();
            if leading > 3 {
                return None;
            }
            if let Some(Kind::FencedDiv(info)) = self.kind(*node)
                && info.fence <= count
            {
                return Some(*node);
            }
        }
        None
    }
}
