//! Raw TeX environment blocks opened by `\begin{...}` and gathered to their `\end{...}`.

use super::{
    Cursor, Extension, Kind, Node, Parser, is_math_environment, raw_tex_env_name, raw_tex_scan,
};

impl Parser {
    /// Feed a continuation line to an open raw TeX environment, returning `true` when the line was
    /// absorbed. Reachable only when every container matched, so the verbatim text stays aligned.
    pub(super) fn continue_raw_tex(
        &mut self,
        container: usize,
        all_matched: bool,
        cursor: &Cursor,
    ) -> bool {
        if !all_matched {
            return false;
        }
        let Some(leaf) = self
            .last_open_child(container)
            .filter(|&c| matches!(self.kind(c), Some(Kind::RawTex { .. })))
        else {
            return false;
        };
        self.feed_raw_tex(leaf, cursor.remaining());
        true
    }

    /// Open a raw TeX environment when the cursor sits on a `\begin{NAME}` at the line start. The
    /// environment gathers lines verbatim through its matching `\end{NAME}` and renders as a
    /// `RawBlock` for `tex`. Math environments stay inline, so they fall through to a paragraph here.
    /// Unlike the foldable openers this one interrupts an open paragraph; a `\begin` that never finds
    /// its `\end` is settled back into a paragraph at end of input.
    ///
    /// Known limitations, both niche and exact in the common free-standing form:
    /// - When the environment directly interrupts an open paragraph with no blank line between, that
    ///   preceding paragraph renders as `Para` rather than the tighter `Plain`.
    /// - A math environment hands its body to the inline phase rather than being gathered verbatim
    ///   here, so a non-math `\begin{…}` sitting at column 0 *inside* a math environment opens a
    ///   fresh block environment there instead of staying part of the enclosing inline math span. A
    ///   nested environment indented or sharing a line with surrounding math (the usual way it is
    ///   written) stays within the span.
    pub(super) fn open_raw_tex(&mut self, container: usize, cursor: &mut Cursor) -> Option<usize> {
        if !self.extensions.contains(Extension::RawTex) {
            return None;
        }
        let name = raw_tex_env_name(cursor.remaining(), b"begin")?;
        if is_math_environment(&name) {
            return None;
        }
        let line = cursor.rest();
        cursor.advance_chars(line.chars().count());
        let kind = Kind::RawTex { name, depth: 0 };
        let parent = self.place(container, &kind);
        let index = self.append_child(parent, Node::new(kind));
        self.feed_raw_tex(index, &line);
        Some(index)
    }

    /// Append one source `line` to an open raw TeX environment and advance its nesting depth. Each
    /// `\begin{NAME}` of the opener's own name deepens the nesting and each `\end{NAME}` lifts it;
    /// when the depth returns to zero the environment closes at that `\end`, dropping the trailing
    /// newline. Any content after the closing `\end` on the same line is re-fed as a fresh line.
    pub(super) fn feed_raw_tex(&mut self, index: usize, line: &str) {
        let Some(Kind::RawTex { name, depth }) = self.kind(index) else {
            return;
        };
        let (new_depth, close_at) = raw_tex_scan(line, name, *depth);
        if let Some(end) = close_at {
            self.append_text(index, line.get(..end).unwrap_or(line));
            self.set_raw_tex_depth(index, 0);
            self.close(index);
            let trailing = line.get(end..).unwrap_or("").to_owned();
            if !trailing.is_empty() {
                self.process_line(&trailing, &[], None);
            }
            return;
        }
        self.append_text(index, line);
        self.append_text(index, "\n");
        self.set_raw_tex_depth(index, new_depth);
    }

    pub(super) fn set_raw_tex_depth(&mut self, index: usize, value: usize) {
        if let Some(node) = self.nodes.get_mut(index)
            && let Kind::RawTex { depth, .. } = &mut node.kind
        {
            *depth = value;
        }
    }
}
