//! Gather consecutive TeX macro definitions as verbatim blocks.

use super::{Cursor, Kind, Node, Parser};

#[derive(Debug, Clone)]
pub(super) enum Definition {
    Body { scanner: Groups, valid_end: usize },
    Between(String),
}

#[derive(Debug, Clone)]
pub(super) struct Groups {
    remaining: Option<usize>,
    braces: usize,
    brackets: usize,
    optional: bool,
    escaped: bool,
    comment: bool,
}

impl Groups {
    fn new(parameters: bool) -> Self {
        Self {
            remaining: parameters.then_some(1),
            braces: 0,
            brackets: 0,
            optional: !parameters,
            escaped: false,
            comment: false,
        }
    }

    fn feed(&mut self, line: &str) -> Option<usize> {
        for (position, character) in line.char_indices() {
            if self.comment {
                continue;
            }
            if self.escaped {
                self.escaped = false;
                continue;
            }
            if character == '%' {
                self.comment = true;
                continue;
            }
            if self.remaining.is_none() && !character.is_whitespace() && character != '*' {
                self.remaining = Some(if character == '{' { 2 } else { 1 });
            }
            match character {
                '\\' => self.escaped = true,
                '[' if self.optional && self.braces == 0 => self.brackets += 1,
                ']' if self.optional && self.braces == 0 => {
                    self.brackets = self.brackets.saturating_sub(1);
                }
                '{' => self.braces += 1,
                '}' if self.braces > 0 => {
                    self.braces -= 1;
                    if self.braces == 0
                        && self.brackets == 0
                        && let Some(remaining) = self.remaining.as_mut()
                    {
                        *remaining = remaining.saturating_sub(1);
                        if *remaining == 0 {
                            return Some(position + character.len_utf8());
                        }
                    }
                }
                _ => {}
            }
        }
        self.comment = false;
        self.escaped = false;
        None
    }
}

fn opening(line: &str) -> Option<(usize, bool)> {
    let command = line.trim_start_matches([' ', '\t']);
    let name = command
        .strip_prefix('\\')?
        .split(|c: char| !c.is_ascii_alphanumeric())
        .next()?;
    matches!(
        name,
        "newcommand" | "renewcommand" | "providecommand" | "DeclareRobustCommand" | "def"
    )
    .then_some((line.len() - command.len() + name.len() + 1, name == "def"))
}

impl Parser {
    pub(super) fn open_tex_definition(
        &mut self,
        container: usize,
        cursor: &mut Cursor,
        following: &[&str],
    ) -> Option<usize> {
        if matches!(self.last_open_leaf_kind(container), Some(Kind::Paragraph)) {
            return None;
        }
        opening(cursor.remaining())?;
        let line = cursor.rest();
        cursor.advance_chars(line.chars().count());
        let kind = Kind::TexDefinition(Definition::Between(String::new()));
        let parent = self.place(container, &kind);
        let index = self.append_child(parent, Node::new(kind));
        self.feed_tex_definition(index, &line, following);
        Some(index)
    }

    pub(super) fn continue_tex_definition(
        &mut self,
        index: usize,
        line: &str,
        following: &[&str],
    ) -> bool {
        if matches!(
            self.kind(index),
            Some(Kind::TexDefinition(Definition::Between(_)))
        ) && opening(line).is_none()
        {
            self.close(index);
            return false;
        }
        self.feed_tex_definition(index, line, following);
        true
    }

    fn feed_tex_definition(&mut self, index: usize, mut line: &str, following: &[&str]) {
        loop {
            if matches!(
                self.kind(index),
                Some(Kind::TexDefinition(Definition::Between(_)))
            ) {
                let Some((end, parameters)) = opening(line) else {
                    self.close(index);
                    if !line.trim().is_empty() {
                        self.process_fragment(
                            self.parent(index),
                            line.trim_start_matches([' ', '\t']),
                            following,
                        );
                    }
                    return;
                };
                if let Some(node) = self.nodes.get_mut(index)
                    && let Kind::TexDefinition(state @ Definition::Between(_)) = &mut node.kind
                {
                    let previous = std::mem::replace(
                        state,
                        Definition::Body {
                            scanner: Groups::new(parameters),
                            valid_end: node.text.len(),
                        },
                    );
                    if let Definition::Between(pending) = previous {
                        node.text.push_str(&pending);
                    }
                    node.text.push_str(line.get(..end).unwrap_or_default());
                }
                line = line.get(end..).unwrap_or_default();
            }
            let end = self
                .nodes
                .get_mut(index)
                .and_then(|node| match &mut node.kind {
                    Kind::TexDefinition(Definition::Body { scanner, .. }) => scanner.feed(line),
                    _ => None,
                });
            let Some(end) = end else {
                self.append_text(index, line);
                self.append_text(index, "\n");
                return;
            };
            self.append_text(index, line.get(..end).unwrap_or_default());
            line = line.get(end..).unwrap_or_default();
            let pending = if line.trim().is_empty() {
                format!("{line}\n")
            } else {
                String::new()
            };
            if let Some(node) = self.nodes.get_mut(index) {
                node.kind = Kind::TexDefinition(Definition::Between(pending));
            }
            if line.trim().is_empty() {
                return;
            }
        }
    }
}
