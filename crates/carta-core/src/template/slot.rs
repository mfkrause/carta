//! Where an interpolated value lands in a template, read off the parsed tree without rendering.

use super::node::{Node, Template};
use crate::Slot;

/// The column a walk has reached, and whether the line so far is all spaces. `None` once the output
/// before it varies with the context, leaving the column undetermined.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Cursor {
    column: Option<usize>,
    spaces: bool,
}

impl Cursor {
    /// The start of the template's first line.
    const START: Self = Self {
        column: Some(0),
        spaces: true,
    };

    /// A cursor whose column no longer follows from the template alone.
    const UNKNOWN: Self = Self {
        column: None,
        spaces: false,
    };

    /// Move past literal `text`: a line break restarts the count, anything else consumes columns and
    /// a non-space ends the all-spaces prefix.
    fn advance(&mut self, text: &str) {
        for ch in text.chars() {
            if ch == '\n' {
                *self = Self::START;
            } else {
                self.column = self.column.map(|column| column + 1);
                self.spaces = self.spaces && ch == ' ';
            }
        }
    }
}

impl Template {
    /// Where the value interpolated for `name` lands, or `None` when the template does not
    /// interpolate it or the output before it leaves its column undetermined. The first
    /// interpolation of the name decides; a name reached only through a dotted path, or with a pipe
    /// that reshapes the value, is not a slot.
    #[must_use]
    pub fn slot(&self, name: &str) -> Option<Slot> {
        let mut cursor = Cursor::START;
        find_slot(&self.nodes, name, &mut cursor)
    }
}

/// Walk `nodes` in output order, threading `cursor` along, and return the slot of the first
/// interpolation of `name`.
fn find_slot(nodes: &[Node], name: &str, cursor: &mut Cursor) -> Option<Slot> {
    for node in nodes {
        match node {
            Node::Literal(text) => cursor.advance(text),
            Node::Var(expr) => {
                if expr.path.as_slice() == [name]
                    && expr.pipes.is_empty()
                    && let Some(column) = cursor.column
                {
                    return Some(Slot {
                        column,
                        indent: if cursor.spaces { column } else { 0 },
                    });
                }
                *cursor = Cursor::UNKNOWN;
            }
            // Every branch is entered at the column reached here, and the one taken decides where
            // the walk resumes: the branches agree on that or the column is lost.
            Node::If {
                branches,
                otherwise,
            } => {
                let bodies = branches.iter().map(|(_, body)| body.as_slice());
                if let Some(slot) =
                    walk_alternatives(bodies.chain([otherwise.as_slice()]), name, cursor)
                {
                    return Some(slot);
                }
            }
            // A loop runs its body zero or more times; with the body and the separator both ending
            // where they began, every iteration count leaves the same column.
            Node::For { body, sep, .. } => {
                let bodies = [body.as_slice(), sep.as_slice()];
                if let Some(slot) = walk_alternatives(bodies.into_iter(), name, cursor) {
                    return Some(slot);
                }
            }
            Node::Partial { .. } => *cursor = Cursor::UNKNOWN,
        }
    }
    None
}

/// Walk each alternative body from the cursor's position, returning the first slot any of them
/// holds. The cursor moves on to where the alternatives leave it when they all agree, the entry
/// position counting as the outcome where none of them runs; otherwise the column is lost.
fn walk_alternatives<'a>(
    bodies: impl Iterator<Item = &'a [Node]>,
    name: &str,
    cursor: &mut Cursor,
) -> Option<Slot> {
    let start = *cursor;
    let outcome = start;
    for body in bodies {
        let mut walked = start;
        if let Some(slot) = find_slot(body, name, &mut walked) {
            return Some(slot);
        }
        if walked != outcome {
            *cursor = Cursor::UNKNOWN;
            return None;
        }
    }
    *cursor = outcome;
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(source: &str, name: &str) -> Option<Slot> {
        Template::parse(source).ok()?.slot(name)
    }

    #[test]
    fn space_prefix_indents_continuations() {
        assert_eq!(
            slot("head\n  $body$\n", "body"),
            Some(Slot {
                column: 2,
                indent: 2
            })
        );
    }

    #[test]
    fn markup_prefix_leaves_continuations_at_the_margin() {
        assert_eq!(
            slot("    <title>$title$</title>\n", "title"),
            Some(Slot {
                column: 11,
                indent: 0
            })
        );
    }

    #[test]
    fn conditional_and_loop_bodies_are_reached() {
        assert_eq!(
            slot("$if(date)$\n  <date>$date$</date>\n$endif$\n", "date"),
            Some(Slot {
                column: 8,
                indent: 0
            })
        );
        assert_eq!(
            slot("$for(author)$\n    $author$\n$endfor$\n", "author"),
            Some(Slot {
                column: 4,
                indent: 4
            })
        );
    }

    #[test]
    fn a_whole_line_conditional_leaves_the_column_where_it_found_it() {
        assert_eq!(
            slot("$if(x)$\n  <x/>\n$endif$\n  $body$\n", "body"),
            Some(Slot {
                column: 2,
                indent: 2
            })
        );
    }

    #[test]
    fn unknown_column_after_an_earlier_value_on_the_line() {
        assert_eq!(slot("$title$ $body$\n", "body"), None);
        assert_eq!(slot("$if(x)$a$endif$  $body$\n", "body"), None);
        assert_eq!(slot("no such variable\n", "body"), None);
    }
}
