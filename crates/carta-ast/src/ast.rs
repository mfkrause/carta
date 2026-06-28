//! The document model: the typed tree that every reader produces and every writer consumes.
//!
//! The Block/Inline split is load-bearing — encoding them as separate enums makes invalid nesting
//! (a block inside link text, say) unrepresentable. The node families and the small enums carry
//! their JSON tag via `#[serde(tag = "t", content = "c")]`; the array-shaped records (`Attr`,
//! `Document`, the table parts) have hand-written codecs in [`crate::serde_impls`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Defines a node enum together with `$tags`, the slice of its variant names. These enums use
/// `#[serde(tag = "t")]` with no per-variant rename, so each variant's identifier *is* its JSON
/// `t` tag; generating the tag list from the same definition keeps it from drifting out of sync
/// with the variants (the round-trip coverage test consumes it as the set of tags to exercise).
macro_rules! node_enum {
    (
        $(#[$enum_meta:meta])*
        $vis:vis enum $name:ident {
            $(
                $(#[$variant_meta:meta])*
                $variant:ident $( ( $($payload:ty),* ) )?
            ),* $(,)?
        }
        tags: $tags:ident
    ) => {
        $(#[$enum_meta])*
        $vis enum $name {
            $(
                $(#[$variant_meta])*
                $variant $( ( $($payload),* ) )?
            ),*
        }

        /// JSON `t` tags of every variant of the enum above, in declaration order.
        $vis const $tags: &[&str] = &[$(stringify!($variant)),*];
    };
}

/// Every textual payload in the tree. Owned today; aliased so it can be swapped for a
/// compact-string type later without touching call sites.
pub type Text = String;

/// The AST schema version carried by an interchange document, as an integer component list
/// (e.g. `[1, 23, 1, 2]`). Stored verbatim so a parsed document re-serializes losslessly; freshly
/// constructed documents default to [`crate::CURRENT_API_VERSION`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ApiVersion(pub Vec<u32>);

impl Default for ApiVersion {
    fn default() -> Self {
        Self(crate::CURRENT_API_VERSION.to_vec())
    }
}

/// A whole document: schema version, metadata, and the block sequence.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Document {
    pub api_version: ApiVersion,
    pub meta: BTreeMap<Text, MetaValue>,
    pub blocks: Vec<Block>,
}

/// Identifier, classes, and ordered key/value pairs attached to many nodes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Attr {
    pub id: Text,
    pub classes: Vec<Text>,
    pub attributes: Vec<(Text, Text)>,
}

/// A link or image destination: URL and title.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Target {
    pub url: Text,
    pub title: Text,
}

/// The name of a raw-passthrough format (e.g. `html`, `latex`) for [`Inline::RawInline`] and
/// [`Block::RawBlock`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Format(pub Text);

node_enum! {
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "t", content = "c")]
    pub enum Block {
        Plain(Vec<Inline>),
        Para(Vec<Inline>),
        LineBlock(Vec<Vec<Inline>>),
        CodeBlock(Attr, Text),
        RawBlock(Format, Text),
        BlockQuote(Vec<Block>),
        OrderedList(ListAttributes, Vec<Vec<Block>>),
        BulletList(Vec<Vec<Block>>),
        DefinitionList(Vec<(Vec<Inline>, Vec<Vec<Block>>)>),
        Header(i32, Attr, Vec<Inline>),
        HorizontalRule,
        Table(Box<Table>),
        Figure(Attr, Caption, Vec<Block>),
        Div(Attr, Vec<Block>),
    }
    tags: BLOCK_TAGS
}

node_enum! {
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "t", content = "c")]
    pub enum Inline {
        Str(Text),
        Emph(Vec<Inline>),
        Underline(Vec<Inline>),
        Strong(Vec<Inline>),
        Strikeout(Vec<Inline>),
        Superscript(Vec<Inline>),
        Subscript(Vec<Inline>),
        SmallCaps(Vec<Inline>),
        Quoted(QuoteType, Vec<Inline>),
        Cite(Vec<Citation>, Vec<Inline>),
        Code(Attr, Text),
        Space,
        SoftBreak,
        LineBreak,
        Math(MathType, Text),
        RawInline(Format, Text),
        Link(Attr, Vec<Inline>, Target),
        Image(Attr, Vec<Inline>, Target),
        Note(Vec<Block>),
        Span(Attr, Vec<Inline>),
    }
    tags: INLINE_TAGS
}

node_enum! {
    /// A metadata value. Documents carry a `String`-keyed map of these.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "t", content = "c")]
    pub enum MetaValue {
        MetaMap(BTreeMap<Text, MetaValue>),
        MetaList(Vec<MetaValue>),
        MetaBool(bool),
        MetaString(Text),
        MetaInlines(Vec<Inline>),
        MetaBlocks(Vec<Block>),
    }
    tags: META_VALUE_TAGS
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum QuoteType {
    SingleQuote,
    DoubleQuote,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum MathType {
    InlineMath,
    DisplayMath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum ListNumberStyle {
    DefaultStyle,
    Example,
    Decimal,
    LowerRoman,
    UpperRoman,
    LowerAlpha,
    UpperAlpha,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum ListNumberDelim {
    DefaultDelim,
    Period,
    OneParen,
    TwoParens,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum Alignment {
    AlignLeft,
    AlignRight,
    AlignCenter,
    AlignDefault,
}

/// A table column's width: an explicit fraction of the available width, or the renderer's default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "c")]
pub enum ColWidth {
    ColWidth(f64),
    ColWidthDefault,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum CitationMode {
    AuthorInText,
    SuppressAuthor,
    NormalCitation,
}

/// The leading-marker configuration of an ordered list: start number, numeral style, delimiter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListAttributes {
    pub start: i32,
    pub style: ListNumberStyle,
    pub delim: ListNumberDelim,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Citation {
    #[serde(rename = "citationId")]
    pub id: Text,
    #[serde(rename = "citationPrefix")]
    pub prefix: Vec<Inline>,
    #[serde(rename = "citationSuffix")]
    pub suffix: Vec<Inline>,
    #[serde(rename = "citationMode")]
    pub mode: CitationMode,
    #[serde(rename = "citationNoteNum")]
    pub note_num: i32,
    #[serde(rename = "citationHash")]
    pub hash: i32,
}

/// A table (or figure) caption: an optional short form plus the full block-level caption.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Caption {
    pub short: Option<Vec<Inline>>,
    pub long: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColSpec {
    pub align: Alignment,
    pub width: ColWidth,
}

/// A table: its attributes, caption, per-column specs, header, bodies, and footer. Boxed inside
/// [`Block::Table`] so the common, table-free blocks stay small.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Table {
    pub attr: Attr,
    pub caption: Caption,
    pub col_specs: Vec<ColSpec>,
    pub head: TableHead,
    pub bodies: Vec<TableBody>,
    pub foot: TableFoot,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TableHead {
    pub attr: Attr,
    pub rows: Vec<Row>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TableBody {
    pub attr: Attr,
    pub row_head_columns: i32,
    pub head: Vec<Row>,
    pub body: Vec<Row>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TableFoot {
    pub attr: Attr,
    pub rows: Vec<Row>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    pub attr: Attr,
    pub cells: Vec<Cell>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub attr: Attr,
    pub align: Alignment,
    pub row_span: i32,
    pub col_span: i32,
    pub content: Vec<Block>,
}

/// Flatten an inline sequence to plain text, discarding markup: textual content
/// ([`Inline::Str`], [`Inline::Code`], [`Inline::Math`]) is kept, breaks and spaces become a single
/// space, container inlines are walked through, and raw passthrough and notes contribute nothing.
#[must_use]
pub fn to_plain_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    push_plain_text(inlines, &mut out);
    out
}

/// Flatten an inline sequence to its plain textual content while preserving quotation: markup
/// wrappers are unwrapped to their contents, textual payloads ([`Inline::Str`], [`Inline::Code`],
/// [`Inline::Math`]) collapse to [`Inline::Str`], breaks and spaces become a single space, and
/// [`Inline::Quoted`] is kept intact so a downstream writer renders the format's quote glyphs around
/// the stripped text. Raw passthrough and notes contribute nothing. Adjacent runs are left as
/// separate [`Inline::Str`] nodes rather than coalesced.
#[must_use]
pub fn to_plain_inlines(inlines: &[Inline]) -> Vec<Inline> {
    let mut out = Vec::new();
    push_plain_inlines(inlines, &mut out);
    out
}

/// The inline content of a block sequence used where inline text is required — a document title, a
/// short header field. A lone paragraph or plain block contributes its inlines; any other shape —
/// an empty sequence, several blocks, or a single block that is not a paragraph — has no inline form
/// and yields an empty slice.
#[must_use]
pub fn single_block_inlines(blocks: &[Block]) -> &[Inline] {
    match blocks {
        [Block::Para(inlines) | Block::Plain(inlines)] => inlines,
        _ => &[],
    }
}

/// Derive a heading identifier from plain text: a non-breaking space is treated as an ordinary
/// space, only alphanumerics, whitespace, and `_`, `-`, `.` are kept, the result is lowercased,
/// whitespace runs collapse to single hyphens, and any leading non-letter characters are dropped.
/// The result is empty when no alphabetic character survives.
#[must_use]
pub fn slug(text: &str) -> String {
    let mut filtered = String::new();
    for ch in text.chars() {
        let ch = if ch == '\u{a0}' { ' ' } else { ch };
        if ch.is_alphanumeric() || ch.is_whitespace() || matches!(ch, '_' | '-' | '.') {
            filtered.extend(ch.to_lowercase());
        }
    }
    let joined = filtered.split_whitespace().collect::<Vec<_>>().join("-");
    joined
        .chars()
        .skip_while(|ch| !ch.is_alphabetic())
        .collect()
}

/// Derive a heading identifier in the `gfm_auto_identifiers` style: full-Unicode lowercasing, keep
/// only alphanumerics, `_`, and `-`, turn each whitespace character into a single `-`, and drop
/// everything else (including `.`). Unlike [`slug`], whitespace runs are not collapsed and no
/// leading characters are stripped, so punctuation removed between words leaves its surrounding
/// separators in place.
#[must_use]
pub fn slug_gfm(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .filter_map(|ch| {
            if ch.is_alphanumeric() || matches!(ch, '_' | '-') {
                Some(ch)
            } else if ch.is_whitespace() {
                Some('-')
            } else {
                None
            }
        })
        .collect()
}

fn push_plain_text(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Str(text) | Inline::Code(_, text) | Inline::Math(_, text) => out.push_str(text),
            Inline::Space | Inline::SoftBreak | Inline::LineBreak => out.push(' '),
            Inline::Emph(xs)
            | Inline::Underline(xs)
            | Inline::Strong(xs)
            | Inline::Strikeout(xs)
            | Inline::Superscript(xs)
            | Inline::Subscript(xs)
            | Inline::SmallCaps(xs)
            | Inline::Quoted(_, xs)
            | Inline::Cite(_, xs)
            | Inline::Link(_, xs, _)
            | Inline::Image(_, xs, _)
            | Inline::Span(_, xs) => push_plain_text(xs, out),
            Inline::RawInline(..) | Inline::Note(_) => {}
        }
    }
}

fn push_plain_inlines(inlines: &[Inline], out: &mut Vec<Inline>) {
    for inline in inlines {
        match inline {
            Inline::Str(text) | Inline::Code(_, text) | Inline::Math(_, text) => {
                out.push(Inline::Str(text.clone()));
            }
            Inline::Space | Inline::SoftBreak | Inline::LineBreak => {
                out.push(Inline::Str(" ".to_owned()));
            }
            Inline::Quoted(quote, xs) => {
                out.push(Inline::Quoted(quote.clone(), to_plain_inlines(xs)));
            }
            Inline::Emph(xs)
            | Inline::Underline(xs)
            | Inline::Strong(xs)
            | Inline::Strikeout(xs)
            | Inline::Superscript(xs)
            | Inline::Subscript(xs)
            | Inline::SmallCaps(xs)
            | Inline::Cite(_, xs)
            | Inline::Link(_, xs, _)
            | Inline::Image(_, xs, _)
            | Inline::Span(_, xs) => push_plain_inlines(xs, out),
            Inline::RawInline(..) | Inline::Note(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_inlines_unwraps_markup_keeps_quotation_and_drops_passthrough() {
        let inlines = vec![
            Inline::Emph(vec![Inline::Str("emph".to_owned())]),
            Inline::Space,
            Inline::Quoted(
                QuoteType::DoubleQuote,
                vec![Inline::Strong(vec![Inline::Str("loud".to_owned())])],
            ),
            Inline::Code(Attr::default(), "code".to_owned()),
            Inline::Math(MathType::InlineMath, "x".to_owned()),
            Inline::RawInline(Format("html".to_owned()), "<br>".to_owned()),
            Inline::Note(vec![Block::Para(vec![Inline::Str("note".to_owned())])]),
        ];
        let plain = to_plain_inlines(&inlines);
        // The emphasis wrapper is gone, the space collapses to a `Str`, the quotation survives with
        // its own markup stripped inside, code and math keep their text, and raw passthrough and the
        // note contribute nothing.
        assert_eq!(
            plain,
            vec![
                Inline::Str("emph".to_owned()),
                Inline::Str(" ".to_owned()),
                Inline::Quoted(QuoteType::DoubleQuote, vec![Inline::Str("loud".to_owned())]),
                Inline::Str("code".to_owned()),
                Inline::Str("x".to_owned()),
            ]
        );
    }

    #[test]
    fn single_block_inlines_takes_a_lone_paragraph_and_nothing_else() {
        let para = vec![Block::Para(vec![
            Inline::Str("Multi".to_owned()),
            Inline::SoftBreak,
            Inline::Str("line".to_owned()),
        ])];
        assert_eq!(
            single_block_inlines(&para),
            &[
                Inline::Str("Multi".to_owned()),
                Inline::SoftBreak,
                Inline::Str("line".to_owned()),
            ]
        );

        // A lone plain block is also unwrapped.
        let plain = vec![Block::Plain(vec![Inline::Str("p".to_owned())])];
        assert_eq!(single_block_inlines(&plain), &[Inline::Str("p".to_owned())]);

        // No inline form: empty, several blocks, or a single non-paragraph block.
        assert!(single_block_inlines(&[]).is_empty());
        assert!(
            single_block_inlines(&[
                Block::Para(vec![Inline::Str("a".to_owned())]),
                Block::Para(vec![Inline::Str("b".to_owned())]),
            ])
            .is_empty()
        );
        assert!(single_block_inlines(&[Block::HorizontalRule]).is_empty());
    }

    #[test]
    fn slug_gfm_drops_dots_keeps_digits_lowercases_and_does_not_collapse() {
        assert_eq!(slug_gfm("Hello, World!"), "hello-world");
        assert_eq!(slug_gfm("1.2 Section A.B"), "12-section-ab");
        assert_eq!(slug_gfm("Foo & Bar"), "foo--bar");
        assert_eq!(slug_gfm("a - b"), "a---b");
        assert_eq!(slug_gfm("Über Café"), "über-café");
        // No fallback: an all-punctuation heading slugs to the empty string, and the caller decides
        // what to substitute.
        assert_eq!(slug_gfm("!!!"), "");
    }
}
