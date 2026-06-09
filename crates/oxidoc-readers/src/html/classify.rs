//! Element classification: maps an element name to the block or inline construct it produces.

#[derive(Clone, Copy)]
pub(super) enum BlockKind {
    Para,
    Header(i32),
    BulletList,
    OrderedList,
    BlockQuote,
    Pre,
    HorizontalRule,
    Div { sectioning: bool },
    DefinitionList,
    Table,
    Figure,
}

pub(super) fn block_kind(name: &str) -> Option<BlockKind> {
    Some(match name {
        "p" => BlockKind::Para,
        "h1" => BlockKind::Header(1),
        "h2" => BlockKind::Header(2),
        "h3" => BlockKind::Header(3),
        "h4" => BlockKind::Header(4),
        "h5" => BlockKind::Header(5),
        "h6" => BlockKind::Header(6),
        "ul" | "menu" => BlockKind::BulletList,
        "ol" => BlockKind::OrderedList,
        "blockquote" => BlockKind::BlockQuote,
        "pre" => BlockKind::Pre,
        "hr" => BlockKind::HorizontalRule,
        "div" => BlockKind::Div { sectioning: false },
        "section" | "header" | "aside" => BlockKind::Div { sectioning: true },
        "dl" => BlockKind::DefinitionList,
        "table" => BlockKind::Table,
        "figure" => BlockKind::Figure,
        _ => return None,
    })
}

pub(super) enum InlineKind {
    Emph,
    Strong,
    Strikeout,
    Underline,
    Superscript,
    Subscript,
    Quoted,
    LineBreak,
    Span,
    Bdo,
    SpanClass,
    Code(Option<&'static str>),
    Anchor,
    Image,
    Style,
    Script,
    Input,
    Transparent,
}

pub(super) fn inline_kind(name: &str) -> InlineKind {
    match name {
        "em" | "i" => InlineKind::Emph,
        "strong" | "b" => InlineKind::Strong,
        "del" | "s" | "strike" => InlineKind::Strikeout,
        "ins" | "u" => InlineKind::Underline,
        "sup" => InlineKind::Superscript,
        "sub" => InlineKind::Subscript,
        "q" => InlineKind::Quoted,
        "br" => InlineKind::LineBreak,
        "span" => InlineKind::Span,
        "bdo" => InlineKind::Bdo,
        "mark" | "small" | "abbr" | "kbd" | "dfn" => InlineKind::SpanClass,
        "code" | "tt" => InlineKind::Code(None),
        "samp" => InlineKind::Code(Some("sample")),
        "var" => InlineKind::Code(Some("variable")),
        "a" => InlineKind::Anchor,
        "img" => InlineKind::Image,
        "style" => InlineKind::Style,
        "script" => InlineKind::Script,
        "input" => InlineKind::Input,
        _ => InlineKind::Transparent,
    }
}

pub(super) fn is_inline_element(name: &str) -> bool {
    !matches!(inline_kind(name), InlineKind::Transparent)
}
