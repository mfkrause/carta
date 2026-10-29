//! Plain metadata text with smart quotation marks.

use carta_ast::Inline;

/// Flatten inline metadata, preserving quotation marks and omitting notes and raw markup.
pub(crate) fn metadata_text(inlines: &[Inline]) -> String {
    let mut output = String::new();
    let mut pending = vec![(inlines.iter(), None)];
    while let Some((inlines, close)) = pending.last_mut() {
        let Some(inline) = inlines.next() else {
            if let Some(close) = close {
                output.push(*close);
            }
            pending.pop();
            continue;
        };
        match inline {
            Inline::Str(text) | Inline::Code(_, text) | Inline::Math(_, text) => {
                output.push_str(text);
            }
            Inline::Space | Inline::SoftBreak | Inline::LineBreak => output.push(' '),
            Inline::Quoted(kind, inner) => {
                let (open, close) = super::quote_marks(kind);
                output.push(open);
                pending.push((inner.iter(), Some(close)));
            }
            Inline::Emph(inner)
            | Inline::Underline(inner)
            | Inline::Strong(inner)
            | Inline::Strikeout(inner)
            | Inline::Superscript(inner)
            | Inline::Subscript(inner)
            | Inline::SmallCaps(inner)
            | Inline::Cite(_, inner)
            | Inline::Link(_, inner, _)
            | Inline::Image(_, inner, _)
            | Inline::Span(_, inner) => pending.push((inner.iter(), None)),
            Inline::RawInline(..) | Inline::Note(_) => {}
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use carta_ast::QuoteType;

    #[test]
    fn deeply_nested_metadata_keeps_quote_boundaries() {
        let mut inline = Inline::Quoted(QuoteType::DoubleQuote, vec![Inline::Str("word".into())]);
        for _ in 0..10_000 {
            inline = Inline::Emph(vec![inline]);
        }
        assert_eq!(
            metadata_text(&[inline, Inline::Str(" end".into())]),
            "“word” end"
        );
    }
}
