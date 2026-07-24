//! List-marker classification and pending-list accumulation for `man` items.

use carta_ast::{Block, Inline, ListAttributes, ListNumberDelim, ListNumberStyle};

use crate::roman::roman_value_loose_forward;

/// What kind of list a `.IP` marker introduces.
pub(super) enum Mark {
    None,
    Bullet,
    Ordered(ListAttributes),
    Text,
}

/// Classifies a `.IP` marker, already reduced to plain text: a bullet glyph, an enumerator (decimal,
/// alphabetic, or roman), or arbitrary text that becomes a definition term.
pub(super) fn classify_mark(mark: &str) -> Mark {
    if mark.is_empty() {
        return Mark::None;
    }
    if matches!(mark, "*" | "\u{2022}" | "\u{00b7}" | "-" | "+") {
        return Mark::Bullet;
    }
    if let Some(attrs) = parse_enumerator(mark) {
        return Mark::Ordered(attrs);
    }
    Mark::Text
}

/// Parses an ordered-list enumerator (`1.`, `a)`, `(iv)`, a bare letter, …) into its list
/// attributes, or returns `None` when the marker is not an enumerator.
pub(super) fn parse_enumerator(mark: &str) -> Option<ListAttributes> {
    if let Some(inner) = mark.strip_prefix('(').and_then(|m| m.strip_suffix(')')) {
        return enumerator_body(inner, ListNumberDelim::TwoParens);
    }
    let (body, delim) = match mark.strip_suffix('.') {
        Some(body) => (body, ListNumberDelim::Period),
        None => match mark.strip_suffix(')') {
            Some(body) => (body, ListNumberDelim::OneParen),
            None => (mark, ListNumberDelim::DefaultDelim),
        },
    };
    enumerator_body(body, delim)
}

/// Parses the numeric/alphabetic/roman body of an enumerator, with its delimiter already determined,
/// into list attributes, or returns `None` when the body is not an enumerator.
fn enumerator_body(body: &str, delim: ListNumberDelim) -> Option<ListAttributes> {
    if body.is_empty() {
        return None;
    }
    if body.chars().all(|c| c.is_ascii_digit()) {
        let start = body.parse().ok()?;
        return Some(ListAttributes {
            start,
            style: ListNumberStyle::Decimal,
            delim,
        });
    }
    if let Some(start) = roman_value_loose_forward(body) {
        let style = if body.chars().next().is_some_and(char::is_uppercase) {
            ListNumberStyle::UpperRoman
        } else {
            ListNumberStyle::LowerRoman
        };
        return Some(ListAttributes {
            start,
            style,
            delim,
        });
    }
    let mut chars = body.chars();
    if let (Some(c), None) = (chars.next(), chars.next())
        && c.is_ascii_alphabetic()
    {
        let start = i32::from((c.to_ascii_lowercase() as u8) - b'a') + 1;
        let style = if c.is_ascii_uppercase() {
            ListNumberStyle::UpperAlpha
        } else {
            ListNumberStyle::LowerAlpha
        };
        return Some(ListAttributes {
            start,
            style,
            delim,
        });
    }
    None
}

/// The accumulating list of the current kind. Consecutive same-kind items append to it; a
/// different kind flushes it first.
pub(super) enum Pending {
    Definition(Vec<(Vec<Inline>, Vec<Vec<Block>>)>),
    Bullet(Vec<Vec<Block>>),
    Ordered(ListAttributes, Vec<Vec<Block>>),
}

pub(super) fn flush_pending(pending: &mut Option<Pending>, out: &mut Vec<Block>) {
    match pending.take() {
        Some(Pending::Definition(items)) => out.push(Block::DefinitionList(items)),
        Some(Pending::Bullet(items)) => out.push(Block::BulletList(items)),
        Some(Pending::Ordered(attrs, items)) => out.push(Block::OrderedList(attrs, items)),
        None => {}
    }
}

pub(super) fn push_definition(
    pending: &mut Option<Pending>,
    out: &mut Vec<Block>,
    term: Vec<Inline>,
    body: Vec<Block>,
) {
    if let Some(Pending::Definition(items)) = pending {
        items.push((term, vec![body]));
        return;
    }
    flush_pending(pending, out);
    *pending = Some(Pending::Definition(vec![(term, vec![body])]));
}

pub(super) fn push_bullet(pending: &mut Option<Pending>, out: &mut Vec<Block>, body: Vec<Block>) {
    if let Some(Pending::Bullet(items)) = pending {
        items.push(body);
        return;
    }
    flush_pending(pending, out);
    *pending = Some(Pending::Bullet(vec![body]));
}

pub(super) fn push_ordered(
    pending: &mut Option<Pending>,
    out: &mut Vec<Block>,
    attrs: ListAttributes,
    body: Vec<Block>,
) {
    if let Some(Pending::Ordered(_, items)) = pending {
        items.push(body);
        return;
    }
    flush_pending(pending, out);
    *pending = Some(Pending::Ordered(attrs, vec![body]));
}
