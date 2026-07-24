use super::*;
use carta_ast::{Attr, Cell, Format, ListNumberDelim, ListNumberStyle, TableHead};

fn render(blocks: Vec<Block>) -> String {
    let document = Document {
        blocks,
        ..Document::default()
    };
    RtfWriter
        .write(&document, &WriterOptions::default())
        .unwrap()
}

fn s(text: &str) -> Inline {
    Inline::Str(text.into())
}

fn para(items: Vec<Inline>) -> Block {
    Block::Para(items)
}

#[test]
fn empty_document_is_empty() {
    assert_eq!(render(vec![]), "");
}

#[test]
fn paragraph_and_plain_spacing() {
    assert_eq!(
        render(vec![para(vec![s("hi")])]),
        "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 hi\\par}"
    );
    assert_eq!(
        render(vec![Block::Plain(vec![s("hi")])]),
        "{\\pard \\ql \\f0 \\sa0 \\li0 \\fi0 hi\\par}"
    );
}

#[test]
fn header_outline_and_size() {
    assert_eq!(
        render(vec![Block::Header(2, Box::default(), vec![s("H")])]),
        "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 \\outlinelevel1 \\b \\fs32 H\\par}"
    );
}

#[test]
fn horizontal_rule_is_centered_em_dashes() {
    assert_eq!(
        render(vec![Block::HorizontalRule]),
        "{\\pard \\qc \\f0 \\sa180 \\li0 \\fi0 \\emdash\\emdash\\emdash\\emdash\\emdash\\par}"
    );
}

#[test]
fn code_block_preserves_lines() {
    assert_eq!(
        render(vec![Block::CodeBlock(Box::default(), "a\nb\n".into())]),
        "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 \\f1 a\\line\nb\\par}"
    );
}

#[test]
fn block_quote_indents() {
    assert_eq!(
        render(vec![Block::BlockQuote(vec![para(vec![s("q")])])]),
        "{\\pard \\ql \\f0 \\sa180 \\li720 \\fi0 q\\par}"
    );
}

#[test]
fn line_block_joins_with_breaks() {
    assert_eq!(
        render(vec![Block::LineBlock(
            vec![vec![s("one")], vec![s("two")],]
        )]),
        "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 one\\line two\\par}"
    );
}

#[test]
fn bullet_list_marker_and_spacing() {
    assert_eq!(
        render(vec![Block::BulletList(vec![
            vec![Block::Plain(vec![s("a")])],
            vec![Block::Plain(vec![s("b")])],
        ])]),
        "{\\pard \\ql \\f0 \\sa0 \\li360 \\fi-360 \\bullet \\tx360\\tab a\\par}\n\
             {\\pard \\ql \\f0 \\sa0 \\li360 \\fi-360 \\bullet \\tx360\\tab b\\sa180\\par}"
    );
}

#[test]
fn nested_bullets_alternate_and_accumulate_spacing() {
    let inner = Block::BulletList(vec![vec![Block::Plain(vec![s("b")])]]);
    let outer = Block::BulletList(vec![vec![Block::Plain(vec![s("a")]), inner]]);
    assert_eq!(
        render(vec![outer]),
        "{\\pard \\ql \\f0 \\sa0 \\li360 \\fi-360 \\bullet \\tx360\\tab a\\par}\n\
             {\\pard \\ql \\f0 \\sa0 \\li720 \\fi-360 \\endash \\tx360\\tab b\\sa180\\sa180\\par}"
    );
}

#[test]
fn ordered_list_numbers() {
    let attrs = ListAttributes {
        start: 1,
        style: ListNumberStyle::Decimal,
        delim: ListNumberDelim::Period,
    };
    assert_eq!(
        render(vec![Block::OrderedList(
            attrs,
            vec![vec![Block::Plain(vec![s("x")])]]
        )]),
        "{\\pard \\ql \\f0 \\sa0 \\li360 \\fi-360 1.\\tx360\\tab x\\sa180\\par}"
    );
}

#[test]
fn definition_list_term_and_definition() {
    assert_eq!(
        render(vec![Block::DefinitionList(vec![(
            vec![s("T")],
            vec![vec![Block::Plain(vec![s("d")])]],
        )])]),
        "{\\pard \\ql \\f0 \\sa0 \\li0 \\fi0 T\\par}\n\
             {\\pard \\ql \\f0 \\sa0 \\li360 \\fi0 d\\sa180\\par}"
    );
}

#[test]
fn inline_styles_and_nesting() {
    assert_eq!(
        render(vec![para(vec![Inline::Strong(vec![Inline::Emph(vec![
            s("x")
        ])])])]),
        "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 {\\b {\\i x}}\\par}"
    );
    assert_eq!(
        render(vec![para(vec![Inline::Code(Box::default(), "c".into())])]),
        "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 {\\f1 c}\\par}"
    );
}

#[test]
fn quoted_uses_escaped_curly_quotes() {
    assert_eq!(
        render(vec![para(vec![Inline::Quoted(
            QuoteType::DoubleQuote,
            vec![s("q")]
        )])]),
        "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 \\u8220\"q\\u8221\"\\par}"
    );
}

#[test]
fn line_break_is_forced() {
    assert_eq!(
        render(vec![para(vec![s("a"), Inline::LineBreak, s("b")])]),
        "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 a\\line b\\par}"
    );
}

#[test]
fn escaping_controls_and_unicode() {
    assert_eq!(
        render(vec![para(vec![s("a{b}c\\d")])]),
        "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 a\\{b\\}c\\\\d\\par}"
    );
    assert_eq!(
        render(vec![para(vec![s("é…")])]),
        "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 \\u233 ?\\u8230 ?\\par}"
    );
    // Astral characters split into a UTF-16 surrogate pair.
    assert_eq!(
        render(vec![para(vec![s("\u{1F600}")])]),
        "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 \\u55357 ?\\u56832 ?\\par}"
    );
}

#[test]
fn link_becomes_hyperlink_field() {
    let target = Box::new(Target {
        url: "http://e.com/a b".into(),
        title: "t".into(),
    });
    assert_eq!(
        render(vec![para(vec![Inline::Link(
            Box::default(),
            vec![s("text")],
            target
        )])]),
        "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 \
             {\\field{\\*\\fldinst{HYPERLINK \"http://e.com/a%20b\"}}\
             {\\fldrslt{\\ul\ntext\n}}}\n\\par}"
    );
}

#[test]
fn image_shows_source() {
    let target = Box::new(Target {
        url: "img.png".into(),
        title: "".into(),
    });
    assert_eq!(
        render(vec![para(vec![Inline::Image(
            Box::default(),
            vec![s("alt")],
            target
        )])]),
        "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 {\\cf1 [image: img.png]\\cf0}\\par}"
    );
}

/// A 4x3-pixel PNG embedded as a `data:` URI, so the writer resolves real bytes and emits a
/// `\pict` group carrying the picture goal.
const EMBEDDED_PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAQAAAADCAIAAAA7ljmRAAAAEElEQVR4nGP4z8AARww4OQD1MQv1NXv7ggAAAABJRU5ErkJggg==";

fn embedded_image(attributes: Vec<(&str, &str)>) -> Block {
    let attr = Attr {
        attributes: attributes
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect(),
        ..Attr::default()
    };
    para(vec![Inline::Image(
        Box::new(attr),
        vec![s("alt")],
        Box::new(Target {
            url: EMBEDDED_PNG.into(),
            title: "".into(),
        }),
    )])
}

#[test]
fn image_without_dimensions_uses_intrinsic_goal() {
    // 4x3 px at 72 dpi: 4*1440/72 = 80 twips wide, 3*1440/72 = 60 twips tall.
    assert!(
        render(vec![embedded_image(vec![])]).contains("\\picw4\\pich3\\picwgoal80\\pichgoal60"),
    );
}

#[test]
fn image_dimensions_set_picture_goal() {
    // width=100px, height=50px at 96 dpi: 100*1440/96 = 1500, 50*1440/96 = 750.
    assert!(
        render(vec![embedded_image(vec![
            ("width", "100px"),
            ("height", "50px")
        ])])
        .contains("\\picw4\\pich3\\picwgoal1500\\pichgoal750"),
    );
}

#[test]
fn image_single_dimension_scales_other_axis() {
    // Only width=1in (1440 twips); the height follows the intrinsic 4:3 ratio: 1440*3/4 = 1080.
    assert!(
        render(vec![embedded_image(vec![("width", "1in")])])
            .contains("\\picwgoal1440\\pichgoal1080"),
    );
    // Only height=1cm (0.3937*1440 = 566 twips floored); width follows: 566.928*4/3 → 755.
    assert!(
        render(vec![embedded_image(vec![("height", "1cm")])])
            .contains("\\picwgoal755\\pichgoal566"),
    );
}

#[test]
fn image_percent_dimension_falls_back_to_intrinsic() {
    // A percentage carries no absolute size, so each axis keeps its intrinsic goal.
    assert!(
        render(vec![embedded_image(vec![("width", "50%")])]).contains("\\picwgoal80\\pichgoal60"),
    );
}

/// A 10x10-pixel JPEG whose JFIF header records 300 dots per inch on both axes.
const EMBEDDED_JPEG_300DPI: &str = "data:image/jpeg;base64,/9j/4AAQSkZJRgABAQEBLAEsAAD/2wBDAAMCAgICAgMCAgIDAwMDBAYEBAQEBAgGBgUGCQgKCgkICQkKDA8MCgsOCwkJDRENDg8QEBEQCgwSExIQEw8QEBD/2wBDAQMDAwQDBAgEBAgQCwkLEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBD/wAARCAAKAAoDAREAAhEBAxEB/8QAFQABAQAAAAAAAAAAAAAAAAAAAAT/xAAUEAEAAAAAAAAAAAAAAAAAAAAA/8QAFgEBAQEAAAAAAAAAAAAAAAAAAAcI/8QAFBEBAAAAAAAAAAAAAAAAAAAAAP/aAAwDAQACEQMRAD8AkQ9pAAAB/9k=";

#[test]
fn jpeg_image_goal_uses_the_jfif_density() {
    // 10 px at the header's 300 dpi: 10*1440/300 = 48 twips per axis, not 72 dpi's 200.
    let block = para(vec![Inline::Image(
        Box::default(),
        vec![s("alt")],
        Box::new(Target {
            url: EMBEDDED_JPEG_300DPI.into(),
            title: "".into(),
        }),
    )]);
    assert!(render(vec![block]).contains("\\picw10\\pich10\\picwgoal48\\pichgoal48"),);
}

#[test]
fn footnote_is_inline_group() {
    assert_eq!(
        render(vec![para(vec![
            s("x"),
            Inline::Note(vec![para(vec![s("n")])])
        ])]),
        "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 x\
             {\\super\\chftn}{\\*\\footnote\\chftn\\~\\plain\\pard \
             {\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 n\\par}\n}\\par}"
    );
}

#[test]
fn raw_block_rtf_passes_through_others_dropped() {
    assert_eq!(
        render(vec![Block::RawBlock(
            Format("rtf".into()),
            "{\\x}\n".into()
        )]),
        "{\\x}"
    );
    assert_eq!(
        render(vec![
            Block::RawBlock(Format("html".into()), "<div>".into()),
            para(vec![s("y")]),
        ]),
        "{\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 y\\par}"
    );
}

#[test]
fn table_rows_and_caption() {
    let cell = |text: &str, align: Alignment| Cell {
        attr: Attr::default(),
        align,
        row_span: 1,
        col_span: 1,
        content: vec![Block::Plain(vec![s(text)])],
    };
    let spec = |align: Alignment| ColSpec {
        align,
        width: ColWidth::ColWidthDefault,
    };
    let table = Table {
        col_specs: vec![spec(Alignment::AlignLeft), spec(Alignment::AlignRight)],
        head: TableHead {
            attr: Attr::default(),
            rows: vec![Row {
                attr: Attr::default(),
                cells: vec![
                    cell("A", Alignment::AlignDefault),
                    cell("B", Alignment::AlignDefault),
                ],
            }],
        },
        ..Table::default()
    };
    assert_eq!(
        render(vec![Block::Table(Box::new(table))]),
        "{\n\\trowd \\trgaph120\n\
             \\clbrdrb\\brdrs\\cellx4320\\clbrdrb\\brdrs\\cellx8640\n\
             \\trkeep\\intbl\n{\n\
             {{\\pard\\intbl \\ql \\f0 \\sa0 \\li0 \\fi0 A\\par}\n\\cell}\n\
             {{\\pard\\intbl \\qr \\f0 \\sa0 \\li0 \\fi0 B\\par}\n\\cell}\n\
             }\n\\intbl\\row}\n\
             {\\pard \\ql \\f0 \\sa180 \\li0 \\fi0 \\par}"
    );
}

#[test]
fn column_widths_honor_explicit_fractions() {
    let spec = |width| ColSpec {
        align: Alignment::AlignDefault,
        width,
    };
    let cw = ColWidth::ColWidth;
    let def = || ColWidth::ColWidthDefault;
    // With no declared widths, the full width divides evenly.
    assert_eq!(column_widths(&[spec(def()), spec(def())]), vec![4320, 8640]);
    // An undeclared column takes no width, so an explicit fraction keeps its proportion.
    assert_eq!(
        column_widths(&[spec(cw(0.8)), spec(def())]),
        vec![6912, 6912]
    );
    assert_eq!(
        column_widths(&[spec(def()), spec(cw(0.5)), spec(def())]),
        vec![0, 4320, 4320]
    );
    // All columns explicit: plain cumulative edges.
    assert_eq!(
        column_widths(&[spec(cw(0.3)), spec(cw(0.3))]),
        vec![2592, 5184]
    );
}

#[test]
fn multi_row_head_borders_only_first_row() {
    // The head's bottom border falls under its first row only.
    let cell = |text: &str| Cell {
        attr: Attr::default(),
        align: Alignment::AlignDefault,
        row_span: 1,
        col_span: 1,
        content: vec![Block::Plain(vec![s(text)])],
    };
    let spec = || ColSpec {
        align: Alignment::AlignDefault,
        width: ColWidth::ColWidthDefault,
    };
    let row = |a: &str, b: &str| Row {
        attr: Attr::default(),
        cells: vec![cell(a), cell(b)],
    };
    let table = Table {
        col_specs: vec![spec(), spec()],
        head: TableHead {
            attr: Attr::default(),
            rows: vec![row("G1", "G2"), row("A", "B")],
        },
        bodies: vec![carta_ast::TableBody {
            attr: Attr::default(),
            row_head_columns: 0,
            head: Vec::new(),
            body: vec![row("1", "2")],
        }],
        ..Table::default()
    };
    let out = render(vec![Block::Table(Box::new(table))]);
    // The first head row is bordered; the second head row and the body row are not.
    assert_eq!(
        out.matches("\\clbrdrb\\brdrs\\cellx4320\\clbrdrb\\brdrs\\cellx8640")
            .count(),
        1
    );
    assert_eq!(out.matches("\n\\cellx4320\\cellx8640\n").count(), 2);
}

#[test]
fn meta_inlines_have_no_paragraph_chrome() {
    let rendered = RtfWriter
        .render_meta_inlines(&[Inline::Emph(vec![s("Title")])], &WriterOptions::default())
        .unwrap();
    assert_eq!(rendered, "{\\i Title}");
}
