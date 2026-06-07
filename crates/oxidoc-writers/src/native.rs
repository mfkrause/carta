//! Native writer: renders the document model to the textual form of its data structure.
//!
//! The output is the document's block sequence rendered as a parenthesized, comma-led data literal:
//! constructor names applied to their arguments, with tuples, lists, and records laid out by a
//! pretty-printer with a line width of 72 and a ribbon width of 60. Strings are escaped with
//! backslash escapes, using symbolic names for control characters. Document metadata is not
//! rendered; the output is the
//! block list alone. The result carries no trailing newline; the caller appends one. This format
//! has no public specification.

use oxidoc_ast::{
    Alignment, Attr, Block, Caption, Cell, Citation, CitationMode, ColSpec, ColWidth, Document,
    Format, Inline, ListAttributes, ListNumberDelim, ListNumberStyle, MathType, QuoteType, Row,
    Table, TableBody, Target,
};
use oxidoc_core::{Result, Writer, WriterOptions};

/// Renders a document to the textual form of its data structure (no trailing newline).
#[derive(Debug, Default, Clone, Copy)]
pub struct NativeWriter;

impl Writer for NativeWriter {
    fn write(&self, document: &Document, _options: &WriterOptions) -> Result<String> {
        let tree = list(document.blocks.iter().map(block).collect());
        let mut printer = Printer {
            out: String::new(),
            col: 0,
            line_indent: 0,
        };
        printer.render(&tree);
        Ok(printer.out)
    }
}

/// Maximum total column a flat layout may reach before a node breaks across lines.
const LINE_LENGTH: usize = 72;
/// Maximum number of non-indentation characters a flat layout may add to a line.
const RIBBON: usize = 60;
/// Columns each level of nesting adds.
const INDENT: usize = 2;
/// Width of the `" , "` separator written between composite items.
const COMPOSITE_SEPARATOR: usize = 3;
/// Width a composite's brackets and inner padding contribute: `[ ` plus ` ]`.
const COMPOSITE_PADDING: usize = 4;

/// A layout tree node: the shape plus its precomputed flat width in characters.
///
/// A node renders flat when its flat width fits the line and ribbon budgets at the current column;
/// otherwise it breaks, and each of its children is then laid out independently. The width is
/// computed once, bottom-up, by the builders, so layout never re-walks a subtree to measure it.
struct Doc {
    width: usize,
    kind: Kind,
}

enum Kind {
    /// An indivisible run of text (already escaped); never broken.
    Atom(String),
    /// A constructor argument wrapped in parentheses with no inner padding: `(Con …)`.
    Wrap(Box<Doc>),
    /// A bracketed, comma-led sequence: a list `[ … ]`, tuple `( … )`, or record `{ … }`.
    Composite {
        open: char,
        close: char,
        items: Vec<Doc>,
    },
    /// A constructor applied to arguments: flat `Con a b`, or the name with each argument on its
    /// own indented line.
    Cons { name: String, args: Vec<Doc> },
}

/// Tracks the output buffer and the current cursor position while laying out a [`Doc`].
struct Printer {
    out: String,
    col: usize,
    line_indent: usize,
}

impl Printer {
    fn write_str(&mut self, text: &str) {
        self.out.push_str(text);
        self.col += text.chars().count();
    }

    fn write_char(&mut self, ch: char) {
        self.out.push(ch);
        self.col += 1;
    }

    fn newline_to(&mut self, indent: usize) {
        self.out.push('\n');
        for _ in 0..indent {
            self.out.push(' ');
        }
        self.col = indent;
        self.line_indent = indent;
    }

    fn fits(&self, width: usize) -> bool {
        self.col + width <= LINE_LENGTH
            && self.col.saturating_sub(self.line_indent) + width <= RIBBON
    }

    fn render(&mut self, doc: &Doc) {
        match &doc.kind {
            Kind::Atom(text) => self.write_str(text),
            Kind::Wrap(inner) => {
                if self.fits(doc.width) {
                    self.write_flat(doc);
                } else if let Kind::Cons { name, args } = &inner.kind {
                    self.write_char('(');
                    self.render_cons_broken(name, args);
                    self.write_char(')');
                } else {
                    self.write_char('(');
                    self.render(inner);
                    self.write_char(')');
                }
            }
            Kind::Composite { open, close, items } => {
                if items.is_empty() {
                    self.write_char(*open);
                    self.write_char(*close);
                } else if self.fits(doc.width) {
                    self.write_flat(doc);
                } else {
                    self.render_composite_broken(*open, *close, items);
                }
            }
            Kind::Cons { name, args } => {
                if args.is_empty() {
                    self.write_str(name);
                } else if self.fits(doc.width) {
                    self.write_flat(doc);
                } else {
                    self.render_cons_broken(name, args);
                }
            }
        }
    }

    fn write_flat(&mut self, doc: &Doc) {
        match &doc.kind {
            Kind::Atom(text) => self.write_str(text),
            Kind::Wrap(inner) => {
                self.write_char('(');
                self.write_flat(inner);
                self.write_char(')');
            }
            Kind::Composite { open, close, items } => {
                if items.is_empty() {
                    self.write_char(*open);
                    self.write_char(*close);
                } else {
                    self.write_char(*open);
                    self.write_char(' ');
                    for (index, item) in items.iter().enumerate() {
                        if index > 0 {
                            self.write_str(" , ");
                        }
                        self.write_flat(item);
                    }
                    self.write_char(' ');
                    self.write_char(*close);
                }
            }
            Kind::Cons { name, args } => {
                self.write_str(name);
                for arg in args {
                    self.write_char(' ');
                    self.write_flat(arg);
                }
            }
        }
    }

    fn render_composite_broken(&mut self, open: char, close: char, items: &[Doc]) {
        let open_col = self.col;
        self.write_char(open);
        self.write_char(' ');
        if let Some(first) = items.first() {
            self.render(first);
        }
        for item in items.iter().skip(1) {
            self.newline_to(open_col);
            self.write_str(", ");
            self.render(item);
        }
        self.newline_to(open_col);
        self.write_char(close);
    }

    /// Lay out a constructor that does not fit flat: the name, then its arguments on the next line.
    /// The arguments share that one line when they all fit; otherwise each takes its own line.
    fn render_cons_broken(&mut self, name: &str, args: &[Doc]) {
        let name_col = self.col;
        self.write_str(name);
        if args.is_empty() {
            return;
        }
        self.newline_to(name_col + INDENT);
        let joined = args.iter().map(|arg| arg.width).sum::<usize>() + (args.len() - 1);
        if self.fits(joined) {
            for (index, arg) in args.iter().enumerate() {
                if index > 0 {
                    self.write_char(' ');
                }
                self.write_flat(arg);
            }
        } else {
            for (index, arg) in args.iter().enumerate() {
                if index > 0 {
                    self.newline_to(name_col + INDENT);
                }
                self.render(arg);
            }
        }
    }
}

fn atom(text: impl Into<String>) -> Doc {
    let text = text.into();
    Doc {
        width: text.chars().count(),
        kind: Kind::Atom(text),
    }
}

fn composite(open: char, close: char, items: Vec<Doc>) -> Doc {
    let width = if items.is_empty() {
        2
    } else {
        let content: usize = items.iter().map(|item| item.width).sum();
        content + COMPOSITE_SEPARATOR * (items.len() - 1) + COMPOSITE_PADDING
    };
    Doc {
        width,
        kind: Kind::Composite { open, close, items },
    }
}

fn list(items: Vec<Doc>) -> Doc {
    composite('[', ']', items)
}

fn tuple(items: Vec<Doc>) -> Doc {
    composite('(', ')', items)
}

fn record(fields: Vec<Doc>) -> Doc {
    composite('{', '}', fields)
}

fn cons(name: &str, args: Vec<Doc>) -> Doc {
    let width = if args.is_empty() {
        name.chars().count()
    } else {
        name.chars().count() + args.iter().map(|arg| 1 + arg.width).sum::<usize>()
    };
    Doc {
        width,
        kind: Kind::Cons {
            name: name.to_owned(),
            args,
        },
    }
}

fn wrap(inner: Doc) -> Doc {
    Doc {
        width: inner.width + 2,
        kind: Kind::Wrap(Box::new(inner)),
    }
}

/// A record field `name = value`. Modeled as a constructor whose name is `name =` so that a value
/// too wide to follow inline hangs on the next line, indented from the field name.
fn field(name: &str, value: Doc) -> Doc {
    cons(&format!("{name} ="), vec![value])
}

fn text_atom(value: &str) -> Doc {
    atom(show_string(value))
}

/// Where an integer appears: as a constructor argument, where a negative value is parenthesized as
/// `show` renders it at that precedence, or standing alone, where it is not.
#[derive(Clone, Copy)]
enum NumberPos {
    Argument,
    Standalone,
}

/// An integer literal, parenthesized when a negative value sits in argument position.
fn integer(value: i64, position: NumberPos) -> Doc {
    if value < 0 && matches!(position, NumberPos::Argument) {
        atom(format!("({value})"))
    } else {
        atom(value.to_string())
    }
}

fn inlines(items: &[Inline]) -> Doc {
    list(items.iter().map(inline).collect())
}

fn blocks(items: &[Block]) -> Doc {
    list(items.iter().map(block).collect())
}

fn attr(value: &Attr) -> Doc {
    tuple(vec![
        text_atom(&value.id),
        list(value.classes.iter().map(|c| text_atom(c)).collect()),
        list(
            value
                .attributes
                .iter()
                .map(|(key, val)| tuple(vec![text_atom(key), text_atom(val)]))
                .collect(),
        ),
    ])
}

fn target(value: &Target) -> Doc {
    tuple(vec![text_atom(&value.url), text_atom(&value.title)])
}

fn format_argument(value: &Format) -> Doc {
    wrap(cons("Format", vec![text_atom(&value.0)]))
}

fn caption_argument(value: &Caption) -> Doc {
    let short = match &value.short {
        None => atom("Nothing"),
        Some(items) => wrap(cons("Just", vec![inlines(items)])),
    };
    wrap(cons("Caption", vec![short, blocks(&value.long)]))
}

fn list_attributes(value: &ListAttributes) -> Doc {
    tuple(vec![
        integer(i64::from(value.start), NumberPos::Standalone),
        atom(number_style(&value.style)),
        atom(number_delim(&value.delim)),
    ])
}

fn block(value: &Block) -> Doc {
    match value {
        Block::Plain(items) => cons("Plain", vec![inlines(items)]),
        Block::Para(items) => cons("Para", vec![inlines(items)]),
        Block::LineBlock(lines) => cons(
            "LineBlock",
            vec![list(lines.iter().map(|l| inlines(l)).collect())],
        ),
        Block::CodeBlock(a, text) => cons("CodeBlock", vec![attr(a), text_atom(text)]),
        Block::RawBlock(fmt, text) => cons("RawBlock", vec![format_argument(fmt), text_atom(text)]),
        Block::BlockQuote(items) => cons("BlockQuote", vec![blocks(items)]),
        Block::OrderedList(list_attrs, items) => cons(
            "OrderedList",
            vec![
                list_attributes(list_attrs),
                list(items.iter().map(|item| blocks(item)).collect()),
            ],
        ),
        Block::BulletList(items) => cons(
            "BulletList",
            vec![list(items.iter().map(|item| blocks(item)).collect())],
        ),
        Block::DefinitionList(definitions) => cons(
            "DefinitionList",
            vec![list(
                definitions
                    .iter()
                    .map(|(term, defs)| {
                        tuple(vec![
                            inlines(term),
                            list(defs.iter().map(|d| blocks(d)).collect()),
                        ])
                    })
                    .collect(),
            )],
        ),
        Block::Header(level, a, items) => cons(
            "Header",
            vec![
                integer(i64::from(*level), NumberPos::Argument),
                attr(a),
                inlines(items),
            ],
        ),
        Block::HorizontalRule => atom("HorizontalRule"),
        Block::Table(table_box) => table(table_box),
        Block::Figure(a, caption, items) => cons(
            "Figure",
            vec![attr(a), caption_argument(caption), blocks(items)],
        ),
        Block::Div(a, items) => cons("Div", vec![attr(a), blocks(items)]),
    }
}

fn inline(value: &Inline) -> Doc {
    match value {
        Inline::Str(text) => cons("Str", vec![text_atom(text)]),
        Inline::Emph(items) => cons("Emph", vec![inlines(items)]),
        Inline::Underline(items) => cons("Underline", vec![inlines(items)]),
        Inline::Strong(items) => cons("Strong", vec![inlines(items)]),
        Inline::Strikeout(items) => cons("Strikeout", vec![inlines(items)]),
        Inline::Superscript(items) => cons("Superscript", vec![inlines(items)]),
        Inline::Subscript(items) => cons("Subscript", vec![inlines(items)]),
        Inline::SmallCaps(items) => cons("SmallCaps", vec![inlines(items)]),
        Inline::Quoted(quote, items) => {
            cons("Quoted", vec![atom(quote_type(quote)), inlines(items)])
        }
        Inline::Cite(citations, items) => cons(
            "Cite",
            vec![
                list(citations.iter().map(citation).collect()),
                inlines(items),
            ],
        ),
        Inline::Code(a, text) => cons("Code", vec![attr(a), text_atom(text)]),
        Inline::Space => atom("Space"),
        Inline::SoftBreak => atom("SoftBreak"),
        Inline::LineBreak => atom("LineBreak"),
        Inline::Math(kind, text) => cons("Math", vec![atom(math_type(kind)), text_atom(text)]),
        Inline::RawInline(fmt, text) => {
            cons("RawInline", vec![format_argument(fmt), text_atom(text)])
        }
        Inline::Link(a, items, tgt) => cons("Link", vec![attr(a), inlines(items), target(tgt)]),
        Inline::Image(a, items, tgt) => cons("Image", vec![attr(a), inlines(items), target(tgt)]),
        Inline::Note(items) => cons("Note", vec![blocks(items)]),
        Inline::Span(a, items) => cons("Span", vec![attr(a), inlines(items)]),
    }
}

fn citation(value: &Citation) -> Doc {
    cons(
        "Citation",
        vec![record(vec![
            field("citationId", text_atom(&value.id)),
            field("citationPrefix", inlines(&value.prefix)),
            field("citationSuffix", inlines(&value.suffix)),
            field("citationMode", atom(citation_mode(&value.mode))),
            field(
                "citationNoteNum",
                integer(i64::from(value.note_num), NumberPos::Standalone),
            ),
            field(
                "citationHash",
                integer(i64::from(value.hash), NumberPos::Standalone),
            ),
        ])],
    )
}

fn table(value: &Table) -> Doc {
    cons(
        "Table",
        vec![
            attr(&value.attr),
            caption_argument(&value.caption),
            list(value.col_specs.iter().map(col_spec).collect()),
            wrap(cons(
                "TableHead",
                vec![
                    attr(&value.head.attr),
                    list(value.head.rows.iter().map(row).collect()),
                ],
            )),
            list(value.bodies.iter().map(table_body).collect()),
            wrap(cons(
                "TableFoot",
                vec![
                    attr(&value.foot.attr),
                    list(value.foot.rows.iter().map(row).collect()),
                ],
            )),
        ],
    )
}

fn col_spec(value: &ColSpec) -> Doc {
    tuple(vec![atom(alignment(&value.align)), col_width(&value.width)])
}

fn col_width(value: &ColWidth) -> Doc {
    match value {
        ColWidth::ColWidthDefault => atom("ColWidthDefault"),
        ColWidth::ColWidth(fraction) => cons("ColWidth", vec![atom(show_double(*fraction))]),
    }
}

fn row(value: &Row) -> Doc {
    cons(
        "Row",
        vec![
            attr(&value.attr),
            list(value.cells.iter().map(cell).collect()),
        ],
    )
}

fn cell(value: &Cell) -> Doc {
    cons(
        "Cell",
        vec![
            attr(&value.attr),
            atom(alignment(&value.align)),
            wrap(cons(
                "RowSpan",
                vec![integer(i64::from(value.row_span), NumberPos::Argument)],
            )),
            wrap(cons(
                "ColSpan",
                vec![integer(i64::from(value.col_span), NumberPos::Argument)],
            )),
            blocks(&value.content),
        ],
    )
}

fn table_body(value: &TableBody) -> Doc {
    cons(
        "TableBody",
        vec![
            attr(&value.attr),
            wrap(cons(
                "RowHeadColumns",
                vec![integer(
                    i64::from(value.row_head_columns),
                    NumberPos::Argument,
                )],
            )),
            list(value.head.iter().map(row).collect()),
            list(value.body.iter().map(row).collect()),
        ],
    )
}

fn alignment(value: &Alignment) -> &'static str {
    match value {
        Alignment::AlignLeft => "AlignLeft",
        Alignment::AlignRight => "AlignRight",
        Alignment::AlignCenter => "AlignCenter",
        Alignment::AlignDefault => "AlignDefault",
    }
}

fn quote_type(value: &QuoteType) -> &'static str {
    match value {
        QuoteType::SingleQuote => "SingleQuote",
        QuoteType::DoubleQuote => "DoubleQuote",
    }
}

fn math_type(value: &MathType) -> &'static str {
    match value {
        MathType::InlineMath => "InlineMath",
        MathType::DisplayMath => "DisplayMath",
    }
}

fn citation_mode(value: &CitationMode) -> &'static str {
    match value {
        CitationMode::AuthorInText => "AuthorInText",
        CitationMode::SuppressAuthor => "SuppressAuthor",
        CitationMode::NormalCitation => "NormalCitation",
    }
}

fn number_style(value: &ListNumberStyle) -> &'static str {
    match value {
        ListNumberStyle::DefaultStyle => "DefaultStyle",
        ListNumberStyle::Example => "Example",
        ListNumberStyle::Decimal => "Decimal",
        ListNumberStyle::LowerRoman => "LowerRoman",
        ListNumberStyle::UpperRoman => "UpperRoman",
        ListNumberStyle::LowerAlpha => "LowerAlpha",
        ListNumberStyle::UpperAlpha => "UpperAlpha",
    }
}

fn number_delim(value: &ListNumberDelim) -> &'static str {
    match value {
        ListNumberDelim::DefaultDelim => "DefaultDelim",
        ListNumberDelim::Period => "Period",
        ListNumberDelim::OneParen => "OneParen",
        ListNumberDelim::TwoParens => "TwoParens",
    }
}

/// Escape names for control characters 0–31, indexed by code point. The entries `a`, `b`, `t`,
/// `n`, `v`, `f`, `r` (codes 7–13) and `SO` (14) yield the short forms `\a`…`\r` and `\SO`.
const CONTROL_NAMES: [&str; 32] = [
    "NUL", "SOH", "STX", "ETX", "EOT", "ENQ", "ACK", "a", "b", "t", "n", "v", "f", "r", "SO", "SI",
    "DLE", "DC1", "DC2", "DC3", "DC4", "NAK", "SYN", "ETB", "CAN", "EM", "SUB", "ESC", "FS", "GS",
    "RS", "US",
];

/// Render a string as `show` would: surrounded by double quotes, with backslash and quote escaped,
/// control characters named, and every non-ASCII character emitted as a decimal escape. A `\&`
/// separator is inserted where a numeric or `\SO` escape would otherwise merge with the following
/// character.
fn show_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    let mut after_numeric = false;
    let mut after_so = false;
    for ch in value.chars() {
        if (after_numeric && ch.is_ascii_digit()) || (after_so && ch == 'H') {
            out.push_str("\\&");
        }
        after_numeric = false;
        after_so = false;

        let code = ch as u32;
        if ch == '"' {
            out.push_str("\\\"");
        } else if code > 127 {
            out.push('\\');
            out.push_str(&code.to_string());
            after_numeric = true;
        } else if code == 127 {
            out.push_str("\\DEL");
        } else if ch == '\\' {
            out.push_str("\\\\");
        } else if code >= 32 {
            out.push(ch);
        } else {
            out.push('\\');
            out.push_str(CONTROL_NAMES.get(code as usize).copied().unwrap_or(""));
            if code == 14 {
                after_so = true;
            }
        }
    }
    out.push('"');
    out
}

/// Render a float in the native value syntax: fixed-point when its magnitude falls in the range
/// `[10^-1, 10^6]`, scientific notation otherwise, always carrying a fractional part.
fn show_double(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value < 0.0 { "-Infinity" } else { "Infinity" }.to_owned();
    }

    let negative = value.is_sign_negative();
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent_text) = scientific
        .split_once('e')
        .unwrap_or((scientific.as_str(), "0"));
    let exponent: i64 = exponent_text.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();

    let body = if (-1..=6).contains(&exponent) {
        fixed_point(&digits, exponent)
    } else {
        scientific_form(&digits, exponent)
    };

    if negative { format!("-{body}") } else { body }
}

fn fixed_point(digits: &str, exponent: i64) -> String {
    let point = exponent + 1;
    if point <= 0 {
        let zeros = usize::try_from(-point).unwrap_or(0);
        format!("0.{}{}", "0".repeat(zeros), digits)
    } else {
        let point = usize::try_from(point).unwrap_or(usize::MAX);
        let len = digits.len();
        if point >= len {
            format!("{}{}.0", digits, "0".repeat(point - len))
        } else {
            let (whole, fraction) = digits.split_at(point);
            format!("{whole}.{fraction}")
        }
    }
}

fn scientific_form(digits: &str, exponent: i64) -> String {
    let (first, rest) = digits.split_at(1.min(digits.len()));
    let fraction = if rest.is_empty() { "0" } else { rest };
    format!("{first}.{fraction}e{exponent}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxidoc_ast::{Attr, Document, Format};

    fn render(blocks: Vec<Block>) -> String {
        let document = Document {
            blocks,
            ..Document::default()
        };
        NativeWriter
            .write(&document, &WriterOptions::default())
            .unwrap()
    }

    #[test]
    fn empty_document() {
        assert_eq!(render(vec![]), "[]");
    }

    #[test]
    fn short_paragraph_stays_flat() {
        let para = Block::Para(vec![
            Inline::Str("hi".into()),
            Inline::Space,
            Inline::Str("there".into()),
        ]);
        assert_eq!(
            render(vec![para]),
            r#"[ Para [ Str "hi" , Space , Str "there" ] ]"#
        );
    }

    #[test]
    fn header_with_attr() {
        let header = Block::Header(
            1,
            Attr {
                id: "hi".into(),
                ..Attr::default()
            },
            vec![Inline::Str("Hi".into())],
        );
        assert_eq!(
            render(vec![header]),
            r#"[ Header 1 ( "hi" , [] , [] ) [ Str "Hi" ] ]"#
        );
    }

    #[test]
    fn raw_block_wraps_format() {
        let raw = Block::RawBlock(Format("html".into()), "<b>".into());
        assert_eq!(render(vec![raw]), r#"[ RawBlock (Format "html") "<b>" ]"#);
    }

    #[test]
    fn paragraph_breaks_when_wide() {
        let para = Block::Para(
            (0..8)
                .flat_map(|_| [Inline::Str("aaaa".into()), Inline::Space])
                .take(15)
                .collect(),
        );
        let rendered = render(vec![para]);
        assert!(rendered.starts_with("[ Para\n    [ Str \"aaaa\"\n    , Space\n"));
        assert!(rendered.ends_with("\n    ]\n]"));
    }

    #[test]
    fn string_escaping_matches_show() {
        assert_eq!(show_string("café"), r#""caf\233""#);
        assert_eq!(show_string("a\u{1}b"), r#""a\SOHb""#);
        assert_eq!(show_string("a\"b\\c"), r#""a\"b\\c""#);
        assert_eq!(show_string("\u{e9}1"), r#""\233\&1""#);
        assert_eq!(show_string("\u{e}H"), r#""\SO\&H""#);
    }

    #[test]
    fn double_formatting_matches_show() {
        assert_eq!(show_double(0.5), "0.5");
        assert_eq!(show_double(0.05), "5.0e-2");
        assert_eq!(show_double(0.1), "0.1");
        assert_eq!(show_double(1.0), "1.0");
        assert_eq!(show_double(1_234_567.0), "1234567.0");
        assert_eq!(show_double(12_345_678.0), "1.2345678e7");
        assert_eq!(show_double(100.25), "100.25");
        assert_eq!(show_double(0.333_333_333_333_333_3), "0.3333333333333333");
    }
}
