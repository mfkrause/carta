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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
