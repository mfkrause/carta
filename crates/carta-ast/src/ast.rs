//! The document model: the typed tree that every reader produces and every writer consumes.
//!
//! The Block/Inline split is load-bearing: encoding them as separate enums makes invalid nesting
//! (a block inside link text, say) unrepresentable. The node families and the small enums carry
//! their JSON tag via `#[serde(tag = "t", content = "c")]`; the array-shaped records (`Attr`,
//! `Document`, the table parts) have hand-written codecs in [`crate::serde_impls`].

use std::collections::BTreeMap;

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

/// Every textual payload in the tree. A small-string type that stores payloads up to 24 bytes
/// inline, avoiding a heap allocation for the short words that dominate prose, while keeping the
/// same 24-byte footprint and string serialization as an owned `String`.
pub type Text = compact_str::CompactString;

/// The AST schema version carried by an interchange document, as an integer component list
/// (e.g. `[1, 23, 1, 2]`). Stored verbatim so a parsed document re-serializes losslessly; freshly
/// constructed documents default to [`crate::CURRENT_API_VERSION`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct ApiVersion(pub Vec<u32>);

impl Default for ApiVersion {
    fn default() -> Self {
        Self(crate::CURRENT_API_VERSION.to_vec())
    }
}

/// A whole document: schema version, metadata, and the block sequence.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub struct Document {
    /// The AST schema version the document carries.
    pub api_version: ApiVersion,
    /// Document metadata, keyed by field name.
    pub meta: BTreeMap<Text, MetaValue>,
    /// The document body.
    pub blocks: Vec<Block>,
}

/// Identifier, classes, and ordered key/value pairs attached to many nodes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub struct Attr {
    /// The element's identifier (the `#id` of an attribute block).
    pub id: Text,
    /// The element's classes, in source order.
    pub classes: Vec<Text>,
    /// The remaining key/value pairs, in source order.
    pub attributes: Vec<(Text, Text)>,
}

/// A link or image destination: URL and title.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub struct Target {
    /// The destination URL.
    pub url: Text,
    /// The destination's title; empty when the source carried none.
    pub title: Text,
}

/// The name of a raw-passthrough format (e.g. `html`, `latex`) for [`Inline::RawInline`] and
/// [`Block::RawBlock`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct Format(pub Text);

node_enum! {
    /// A block-level node of the document body.
    #[derive(Debug, Clone, PartialEq)]
    #[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
    #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
    #[cfg_attr(feature = "serde", serde(tag = "t", content = "c"))]
    pub enum Block {
        /// Inline content standing alone without paragraph semantics (a tight list item, a
        /// table cell).
        Plain(Vec<Inline>),
        /// A paragraph.
        Para(Vec<Inline>),
        /// A sequence of lines whose divisions are preserved.
        LineBlock(Vec<Vec<Inline>>),
        /// A block of verbatim code with its attributes.
        CodeBlock(Box<Attr>, Text),
        /// Verbatim content in the named format, emitted only by that format's writers.
        RawBlock(Format, Text),
        /// A quoted block sequence.
        BlockQuote(Vec<Block>),
        /// A numbered list: the marker configuration plus one block sequence per item.
        OrderedList(ListAttributes, Vec<Vec<Block>>),
        /// A bulleted list: one block sequence per item.
        BulletList(Vec<Vec<Block>>),
        /// A definition list: each entry pairs a term with its definition bodies.
        DefinitionList(Vec<(Vec<Inline>, Vec<Vec<Block>>)>),
        /// A section heading: level, attributes, and heading text.
        Header(i32, Box<Attr>, Vec<Inline>),
        /// A thematic break.
        HorizontalRule,
        /// A table; see [`Table`].
        Table(Box<Table>),
        /// A figure: attributes, caption, and content.
        Figure(Box<Attr>, Box<Caption>, Vec<Block>),
        /// A generic attributed block container.
        Div(Box<Attr>, Vec<Block>),
    }
    tags: BLOCK_TAGS
}

node_enum! {
    /// An inline node: a piece of text or an intra-paragraph construct.
    #[derive(Debug, Clone, PartialEq)]
    #[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
    #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
    #[cfg_attr(feature = "serde", serde(tag = "t", content = "c"))]
    pub enum Inline {
        /// A run of text.
        Str(Text),
        /// Emphasized content, conventionally italic.
        Emph(Vec<Inline>),
        /// Underlined content.
        Underline(Vec<Inline>),
        /// Strong content, conventionally bold.
        Strong(Vec<Inline>),
        /// Struck-out content.
        Strikeout(Vec<Inline>),
        /// Superscripted content.
        Superscript(Vec<Inline>),
        /// Subscripted content.
        Subscript(Vec<Inline>),
        /// Small-caps content.
        SmallCaps(Vec<Inline>),
        /// Quoted content, with the quotation-mark kind alongside.
        Quoted(QuoteType, Vec<Inline>),
        /// A citation: the references cited plus the source text they were written as.
        Cite(Vec<Citation>, Vec<Inline>),
        /// A span of verbatim code with its attributes.
        Code(Box<Attr>, Text),
        /// An inter-word space.
        Space,
        /// A soft line break: a source newline that renders as breakable space.
        SoftBreak,
        /// A hard line break.
        LineBreak,
        /// A math payload, inline or display.
        Math(MathType, Text),
        /// Verbatim content in the named format, emitted only by that format's writers.
        RawInline(Format, Text),
        /// A link: attributes, link text, and destination.
        Link(Box<Attr>, Vec<Inline>, Box<Target>),
        /// An image: attributes, alt text, and source.
        Image(Box<Attr>, Vec<Inline>, Box<Target>),
        /// A footnote or endnote holding block content.
        Note(Vec<Block>),
        /// A generic attributed inline container.
        Span(Box<Attr>, Vec<Inline>),
    }
    tags: INLINE_TAGS
}

node_enum! {
    /// A metadata value. Documents carry a `String`-keyed map of these.
    #[derive(Debug, Clone, PartialEq)]
    #[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
    #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
    #[cfg_attr(feature = "serde", serde(tag = "t", content = "c"))]
    pub enum MetaValue {
        /// A nested map of metadata values.
        MetaMap(BTreeMap<Text, MetaValue>),
        /// A list of metadata values.
        MetaList(Vec<MetaValue>),
        /// A boolean.
        MetaBool(bool),
        /// A plain string.
        MetaString(Text),
        /// Inline markup content.
        MetaInlines(Vec<Inline>),
        /// Block-level content.
        MetaBlocks(Vec<Block>),
    }
    tags: META_VALUE_TAGS
}

/// The quotation-mark kind of an [`Inline::Quoted`] span.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "t"))]
pub enum QuoteType {
    /// Single quotes.
    SingleQuote,
    /// Double quotes.
    DoubleQuote,
}

/// Whether an [`Inline::Math`] payload renders within the line or as its own display block.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "t"))]
pub enum MathType {
    /// Math rendered within the line of text.
    InlineMath,
    /// Math rendered as its own display block.
    DisplayMath,
}

/// The numeral style of an ordered list's markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "t"))]
pub enum ListNumberStyle {
    /// The target format's default style.
    DefaultStyle,
    /// `(@)` example-list numbering, sequential across the whole document.
    Example,
    /// Decimal numbers.
    Decimal,
    /// Lowercase roman numerals.
    LowerRoman,
    /// Uppercase roman numerals.
    UpperRoman,
    /// Lowercase letters.
    LowerAlpha,
    /// Uppercase letters.
    UpperAlpha,
}

/// The punctuation around an ordered list's markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "t"))]
pub enum ListNumberDelim {
    /// The target format's default delimiter.
    DefaultDelim,
    /// A trailing period: `1.`.
    Period,
    /// A trailing parenthesis: `1)`.
    OneParen,
    /// Enclosing parentheses: `(1)`.
    TwoParens,
}

/// The horizontal alignment of a table column or cell.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "t"))]
pub enum Alignment {
    /// Left-aligned.
    AlignLeft,
    /// Right-aligned.
    AlignRight,
    /// Centered.
    AlignCenter,
    /// The target format's default alignment.
    AlignDefault,
}

/// A table column's width: an explicit fraction of the available width, or the renderer's default.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "t", content = "c"))]
pub enum ColWidth {
    /// An explicit fraction of the available width.
    ColWidth(f64),
    /// The renderer's default width.
    ColWidthDefault,
}

/// How a citation is rendered relative to the sentence around it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "t"))]
pub enum CitationMode {
    /// The author's name reads as part of the sentence, with the year set off from it.
    AuthorInText,
    /// The author's name is omitted, leaving only the year and any locator.
    SuppressAuthor,
    /// The whole citation is set off from the sentence, conventionally in parentheses.
    NormalCitation,
}

/// The leading-marker configuration of an ordered list: start number, numeral style, delimiter.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub struct ListAttributes {
    /// The number the first item carries.
    pub start: i32,
    /// The numeral style of the markers.
    pub style: ListNumberStyle,
    /// The punctuation around the markers.
    pub delim: ListNumberDelim,
}

/// One cited reference within an [`Inline::Cite`].
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct Citation {
    /// The citation key identifying the referenced source.
    #[cfg_attr(feature = "serde", serde(rename = "citationId"))]
    pub id: Text,
    /// Inline content preceding the key inside the citation (e.g. `see`).
    #[cfg_attr(feature = "serde", serde(rename = "citationPrefix"))]
    pub prefix: Vec<Inline>,
    /// Inline content following the key inside the citation (e.g. a locator such as `p. 12`).
    #[cfg_attr(feature = "serde", serde(rename = "citationSuffix"))]
    pub suffix: Vec<Inline>,
    /// How the citation is rendered relative to the surrounding sentence.
    #[cfg_attr(feature = "serde", serde(rename = "citationMode"))]
    pub mode: CitationMode,
    /// The sequence number of the note the citation belongs to; `0` before citations are processed.
    #[cfg_attr(feature = "serde", serde(rename = "citationNoteNum"))]
    pub note_num: i32,
    /// An occurrence hash assigned by a citation processor; `0` before citations are processed.
    #[cfg_attr(feature = "serde", serde(rename = "citationHash"))]
    pub hash: i32,
}

/// A table (or figure) caption: an optional short form plus the full block-level caption.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub struct Caption {
    /// The optional short form, for contexts where the full caption does not fit (a list of
    /// tables, say).
    pub short: Option<Vec<Inline>>,
    /// The full caption content.
    pub long: Vec<Block>,
}

/// One table column's specification.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub struct ColSpec {
    /// The column's horizontal alignment.
    pub align: Alignment,
    /// The column's width.
    pub width: ColWidth,
}

/// A table: its attributes, caption, per-column specs, header, bodies, and footer. Boxed inside
/// [`Block::Table`] so the common, table-free blocks stay small. This is the general rule for the
/// node enums: attribute-bearing and otherwise heavy payloads are boxed so the common lightweight
/// variants set each enum's size.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub struct Table {
    /// The table's attributes.
    pub attr: Attr,
    /// The table's caption.
    pub caption: Caption,
    /// Per-column alignment and width, in column order.
    pub col_specs: Vec<ColSpec>,
    /// The header section.
    pub head: TableHead,
    /// The body sections; a table may carry several.
    pub bodies: Vec<TableBody>,
    /// The footer section.
    pub foot: TableFoot,
}

/// A table's header section.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub struct TableHead {
    /// The section's attributes.
    pub attr: Attr,
    /// The header rows.
    pub rows: Vec<Row>,
}

/// One body section of a table.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub struct TableBody {
    /// The section's attributes.
    pub attr: Attr,
    /// How many leading columns of each row act as row headers.
    pub row_head_columns: i32,
    /// Header rows specific to this body section.
    pub head: Vec<Row>,
    /// The section's content rows.
    pub body: Vec<Row>,
}

/// A table's footer section.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub struct TableFoot {
    /// The section's attributes.
    pub attr: Attr,
    /// The footer rows.
    pub rows: Vec<Row>,
}

/// One table row.
#[derive(Debug, Clone, PartialEq, Default)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub struct Row {
    /// The row's attributes.
    pub attr: Attr,
    /// The row's cells, in column order.
    pub cells: Vec<Cell>,
}

/// One table cell.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
pub struct Cell {
    /// The cell's attributes.
    pub attr: Attr,
    /// The cell's horizontal alignment; [`Alignment::AlignDefault`] defers to the column's.
    pub align: Alignment,
    /// How many rows the cell spans.
    pub row_span: i32,
    /// How many columns the cell spans.
    pub col_span: i32,
    /// The cell's block content.
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

/// The inline content of a block sequence used where inline text is required (a document title, a
/// short header field). A lone paragraph or plain block contributes its inlines; any other shape
/// (an empty sequence, several blocks, or a single block that is not a paragraph) has no inline form
/// and yields an empty slice.
#[must_use]
pub fn single_block_inlines(blocks: &[Block]) -> &[Inline] {
    match blocks {
        [Block::Para(inlines) | Block::Plain(inlines)] => inlines,
        _ => &[],
    }
}

/// Whether a character is a Unicode combining mark (general category `Mn`, `Mc`, or `Me`).
fn is_combining_mark(ch: char) -> bool {
    use unicode_general_category::GeneralCategory::{EnclosingMark, NonspacingMark, SpacingMark};
    matches!(
        unicode_general_category::get_general_category(ch),
        NonspacingMark | SpacingMark | EnclosingMark
    )
}

/// Whether a character is a Unicode letter: general category `Lu`, `Ll`, `Lt`, `Lm`, or `Lo`. This
/// is narrower than [`char::is_alphabetic`], which also admits the `Other_Alphabetic` property
/// (letter-like marks and symbols) that does not count toward a heading identifier.
fn is_letter(ch: char) -> bool {
    use unicode_general_category::GeneralCategory::{
        LowercaseLetter, ModifierLetter, OtherLetter, TitlecaseLetter, UppercaseLetter,
    };
    matches!(
        unicode_general_category::get_general_category(ch),
        UppercaseLetter | LowercaseLetter | TitlecaseLetter | ModifierLetter | OtherLetter
    )
}

/// Derive a heading identifier from plain text: a non-breaking space is treated as an ordinary
/// space, the text is lowercased, only Unicode letters and numbers (by general category), whitespace,
/// and `_`, `-`, `.` are kept, whitespace runs collapse to single hyphens, and the leading run up to
/// the first letter is dropped. The result is empty when no letter survives. Lowercasing precedes
/// filtering, so a combining mark produced by case-folding a precomposed letter is removed.
#[must_use]
pub fn slug(text: &str) -> String {
    let mut filtered = String::new();
    for ch in text.chars().flat_map(char::to_lowercase) {
        let ch = if ch == '\u{a0}' { ' ' } else { ch };
        if is_letter(ch) || ch.is_numeric() || ch.is_whitespace() || matches!(ch, '_' | '-' | '.') {
            filtered.push(ch);
        }
    }
    let joined = filtered.split_whitespace().collect::<Vec<_>>().join("-");
    joined.chars().skip_while(|ch| !is_letter(*ch)).collect()
}

/// Derive a heading identifier in the `gfm_auto_identifiers` style: full-Unicode lowercasing, keep
/// alphanumerics, combining marks, `_`, and `-`, turn each whitespace character into a single `-`,
/// and drop everything else (including `.`). Unlike [`slug`], whitespace runs are not collapsed and
/// no leading characters are stripped, so punctuation removed between words leaves its surrounding
/// separators in place, and combining marks (including any introduced by case-folding) are retained.
#[must_use]
pub fn slug_gfm(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .filter_map(|ch| {
            if ch.is_alphanumeric() || is_combining_mark(ch) || matches!(ch, '_' | '-') {
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
                out.push(Inline::Str(Text::from(" ")));
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
    fn node_enums_stay_small() {
        assert!(std::mem::size_of::<Inline>() <= 64);
        assert!(std::mem::size_of::<Block>() <= 64);
    }

    #[test]
    fn text_stays_word_sized() {
        assert_eq!(
            std::mem::size_of::<Text>(),
            std::mem::size_of::<String>(),
            "Text must keep String's footprint so swapping the type never fattens every node"
        );
    }

    #[test]
    fn plain_inlines_unwraps_markup_keeps_quotation_and_drops_passthrough() {
        let inlines = vec![
            Inline::Emph(vec![Inline::Str("emph".into())]),
            Inline::Space,
            Inline::Quoted(
                QuoteType::DoubleQuote,
                vec![Inline::Strong(vec![Inline::Str("loud".into())])],
            ),
            Inline::Code(Box::default(), "code".into()),
            Inline::Math(MathType::InlineMath, "x".into()),
            Inline::RawInline(Format("html".into()), "<br>".into()),
            Inline::Note(vec![Block::Para(vec![Inline::Str("note".into())])]),
        ];
        let plain = to_plain_inlines(&inlines);
        assert_eq!(
            plain,
            vec![
                Inline::Str("emph".into()),
                Inline::Str(" ".into()),
                Inline::Quoted(QuoteType::DoubleQuote, vec![Inline::Str("loud".into())]),
                Inline::Str("code".into()),
                Inline::Str("x".into()),
            ]
        );
    }

    #[test]
    fn single_block_inlines_takes_a_lone_paragraph_and_nothing_else() {
        let para = vec![Block::Para(vec![
            Inline::Str("Multi".into()),
            Inline::SoftBreak,
            Inline::Str("line".into()),
        ])];
        assert_eq!(
            single_block_inlines(&para),
            &[
                Inline::Str("Multi".into()),
                Inline::SoftBreak,
                Inline::Str("line".into()),
            ]
        );

        let plain = vec![Block::Plain(vec![Inline::Str("p".into())])];
        assert_eq!(single_block_inlines(&plain), &[Inline::Str("p".into())]);

        assert!(single_block_inlines(&[]).is_empty());
        assert!(
            single_block_inlines(&[
                Block::Para(vec![Inline::Str("a".into())]),
                Block::Para(vec![Inline::Str("b".into())]),
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
