use super::super::AsciidocReader;
use super::super::Parser;
use carta_ast::{Block, Inline};
use carta_core::{Reader, ReaderOptions};

#[test]
fn raw_blocks_become_raw_inlines_between_html_blocks() {
    assert_eq!(
        Parser::html_inlines("<article>x</article><p>y</p>"),
        [
            Inline::RawInline(carta_ast::Format("html".into()), "<article>".into()),
            Inline::LineBreak,
            Inline::Str("x".into()),
            Inline::LineBreak,
            Inline::RawInline(carta_ast::Format("html".into()), "</article>".into()),
            Inline::LineBreak,
            Inline::Str("y".into()),
        ]
    );
}

#[test]
fn html_blocks_convert_to_inlines_with_pandoc_separators() {
    assert_eq!(
        Parser::html_inlines("<p>one</p><hr/><p>two</p><pre>code</pre>"),
        [
            Inline::Str("one".into()),
            Inline::LineBreak,
            Inline::LineBreak,
            Inline::Str("two".into()),
            Inline::LineBreak,
            Inline::Code(Box::default(), "code".into()),
        ]
    );
}

#[test]
fn html_lists_and_tables_flatten_in_order() {
    assert_eq!(
        Parser::html_inlines(
            "<ul><li>one</li><li>two</li></ul><table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>"
        ),
        [
            Inline::Str("onetwo".into()),
            Inline::LineBreak,
            Inline::Str("AB".into()),
            Inline::LineBreak,
            Inline::Str("12".into()),
        ]
    );
}

#[test]
fn passthrough_boundaries_follow_pandoc_line_break_semigroup() {
    let reader = AsciidocReader;
    for (source, expected) in [
        (
            "Before pass:[<br></br>] after\n",
            vec![
                Inline::Str("Before".into()),
                Inline::LineBreak,
                Inline::Str("after".into()),
            ],
        ),
        (
            "+++<br> </br>+++\n",
            vec![
                Inline::LineBreak,
                Inline::RawInline(carta_ast::Format("html".into()), "</br>".into()),
            ],
        ),
    ] {
        let document = reader.read(source, &ReaderOptions::default()).unwrap();
        assert_eq!(document.blocks, [Block::Para(expected)], "{source:?}");
    }
}
