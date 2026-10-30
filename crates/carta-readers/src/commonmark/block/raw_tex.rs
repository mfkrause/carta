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

    /// Open a line-leading raw TeX environment through its matching `\end{NAME}`.
    ///
    /// Math environments stay inline; unclosed raw environments become paragraphs.
    /// A line-leading non-math opener inside inline math can open a separate block.
    pub(super) fn open_raw_tex(&mut self, container: usize, cursor: &mut Cursor) -> Option<usize> {
        let raw_tex = self.extensions.contains(Extension::RawTex);
        let literate_code = self.extensions.contains(Extension::LiterateHaskell)
            && cursor
                .remaining()
                .strip_prefix("\\begin{code}")
                .is_some_and(|rest| rest.trim().is_empty());
        if !raw_tex && !literate_code {
            return None;
        }
        let name = raw_tex_env_name(cursor.remaining(), b"begin")?;
        if is_math_environment(&name) {
            return None;
        }
        let line = cursor.rest();
        cursor.advance_chars(line.chars().count());
        let kind = Kind::RawTex {
            name,
            depth: 0,
            literate_code,
        };
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
        let Some(Kind::RawTex {
            name,
            depth,
            literate_code,
        }) = self.kind(index)
        else {
            return;
        };
        let (new_depth, close_at) = if *literate_code {
            (
                0,
                line.find("\\end{code}")
                    .map(|start| start + "\\end{code}".len()),
            )
        } else {
            raw_tex_scan(line, name, *depth)
        };
        if let Some(end) = close_at {
            if line.get(end..).is_some_and(|rest| !rest.trim().is_empty())
                && let Some(node) = self.nodes.get_mut(index)
                && let Kind::RawTex { literate_code, .. } = &mut node.kind
            {
                *literate_code = false;
            }
            self.append_text(index, line.get(..end).unwrap_or(line));
            self.set_raw_tex_depth(index, 0);
            if self.greedy_paragraphs
                && let Some(previous) = self
                    .nodes
                    .get(index)
                    .and_then(|node| self.nodes.get(node.parent))
                    .and_then(|parent| parent.children.iter().rev().nth(1).copied())
                && let Some(node) = self.nodes.get_mut(previous)
                && matches!(node.kind, Kind::Paragraph)
                && !node.last_line_blank
            {
                node.as_plain = true;
            }
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
