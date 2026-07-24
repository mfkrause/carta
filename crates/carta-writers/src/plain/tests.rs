use super::*;
use carta_ast::Document;

fn render(blocks: Vec<Block>) -> String {
    let document = Document {
        blocks,
        ..Document::default()
    };
    PlainWriter
        .write(&document, &WriterOptions::default())
        .unwrap()
}

fn render_columns(blocks: Vec<Block>, columns: usize) -> String {
    let document = Document {
        blocks,
        ..Document::default()
    };
    let mut options = WriterOptions::default();
    options.columns = Some(columns);
    PlainWriter.write(&document, &options).unwrap()
}

#[test]
fn deeply_nested_tables_render_without_compounding_measurement() {
    // without the nesting cap, column sizing multiplies renders per level: exponential in depth
    use carta_ast::{Alignment, Cell, ColSpec, ColWidth, Row, Table, TableBody};

    fn nested_table(content: Vec<Block>) -> Block {
        let cell = Cell {
            attr: Attr::default(),
            align: Alignment::AlignDefault,
            row_span: 1,
            col_span: 1,
            content,
        };
        let filler = Cell {
            content: vec![Block::Para(vec![Inline::Str("cell".into())])],
            ..cell.clone()
        };
        let spec = ColSpec {
            align: Alignment::AlignDefault,
            width: ColWidth::ColWidthDefault,
        };
        Block::Table(Box::new(Table {
            col_specs: vec![spec.clone(), spec],
            bodies: vec![TableBody {
                body: vec![Row {
                    attr: Attr::default(),
                    cells: vec![cell, filler],
                }],
                ..TableBody::default()
            }],
            ..Table::default()
        }))
    }

    // deep enough that compounding measurement would take minutes; capped stays under a second
    let mut block = Block::Para(vec![Inline::Str("innermost".into())]);
    for _ in 0..9 {
        block = nested_table(vec![block]);
    }
    render(vec![block]);
}

fn long_paragraph() -> Vec<Block> {
    let words: Vec<Inline> = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda"
        .split(' ')
        .flat_map(|word| [Inline::Str(word.to_owned().into()), Inline::Space])
        .collect();
    vec![Block::Para(words)]
}

#[test]
fn narrow_columns_wraps_a_paragraph_sooner() {
    let wide = render_columns(long_paragraph(), 40);
    let narrow = render_columns(long_paragraph(), 20);
    // A narrower fill column forces more line breaks.
    assert!(narrow.lines().count() > wide.lines().count());
    // Every laid-out line stays within the requested width.
    assert!(narrow.lines().all(|line| line.chars().count() <= 20));
    assert!(wide.lines().all(|line| line.chars().count() <= 40));
}

#[test]
fn omitted_columns_uses_the_default_fill_width() {
    // The default-width render is identical to passing the built-in width explicitly.
    assert_eq!(
        render(long_paragraph()),
        render_columns(long_paragraph(), 72)
    );
}

fn math_para(kind: MathType, tex: &str) -> Block {
    Block::Para(vec![Inline::Math(kind, tex.to_owned().into())])
}

fn inline(tex: &str) -> String {
    render(vec![math_para(MathType::InlineMath, tex)])
}

fn display(tex: &str) -> String {
    render(vec![math_para(MathType::DisplayMath, tex)])
}

#[test]
fn variable_with_superscript_uses_unicode_exponent() {
    assert_eq!(inline("a^2"), "a\u{b2}");
}

#[test]
fn polynomial_lays_out_with_operator_and_relation_spacing() {
    // binary operators take U+2005, the relation U+2004; digit exponents map to superscripts
    assert_eq!(
        inline("a^2 + b^2 = c^2"),
        "a\u{b2}\u{2005}+\u{2005}b\u{b2}\u{2004}=\u{2004}c\u{b2}"
    );
}

#[test]
fn subscript_falls_back_to_parenthesized_form_for_letters() {
    // A letter index has no unicode subscript glyph, so it renders parenthesized.
    assert_eq!(inline("a_n"), "a_(n)");
}

#[test]
fn greek_letters_render_as_their_codepoints() {
    assert_eq!(
        inline("\\alpha + \\beta"),
        "\u{3b1}\u{2005}+\u{2005}\u{3b2}"
    );
}

#[test]
fn blackboard_bold_renders_as_letterlike_symbol() {
    assert_eq!(inline("\\mathbb{R}"), "\u{211d}");
}

#[test]
fn accent_renders_as_combining_mark() {
    assert_eq!(inline("\\bar{x}"), "x\u{304}");
}

#[test]
fn integral_uses_unicode_scripts_and_thin_space() {
    // the integral carries its limits as unicode sub/superscripts; `\,` renders as U+2006
    assert_eq!(
        display("\\int_0^1 x \\, dx"),
        "\u{222b}\u{2080}\u{b9}x\u{2006}dx"
    );
}

#[test]
fn inline_fallback_emits_verbatim_single_dollars() {
    // no single-line form: wrapped verbatim in `$…$`, dollars unescaped
    assert_eq!(inline("\\frac{1}{2}"), "$\\frac{1}{2}$");
}

#[test]
fn display_fallback_emits_verbatim_double_dollars() {
    assert_eq!(display("\\sqrt{x}"), "$$\\sqrt{x}$$");
}

#[test]
fn inline_fallback_trims_edge_whitespace() {
    // inline fallback trims edge whitespace before wrapping; interior whitespace preserved
    assert_eq!(inline("\\sqrt{x} "), "$\\sqrt{x}$");
    assert_eq!(inline(" \\sqrt{x}"), "$\\sqrt{x}$");
    assert_eq!(inline("  \\sqrt{x}  "), "$\\sqrt{x}$");
    assert_eq!(inline("\\sqrt{x}   y"), "$\\sqrt{x}   y$");
}

#[test]
fn display_fallback_keeps_edge_whitespace() {
    // Display fallback wraps the source as written; only inline math trims its edges.
    assert_eq!(display("\\sqrt{x} "), "$$\\sqrt{x} $$");
    assert_eq!(display(" \\sqrt{x}"), "$$ \\sqrt{x}$$");
}

#[test]
fn inline_fallback_of_lone_backslash() {
    // a lone backslash wraps to `$\$` unescaped; a bailing `\ ` trims to the same body
    assert_eq!(inline("\\"), "$\\$");
}

#[test]
fn math_flows_inside_surrounding_text() {
    let blocks = vec![Block::Para(vec![
        Inline::Str("value".to_owned().into()),
        Inline::Space,
        Inline::Math(MathType::InlineMath, "E = mc^2".to_owned().into()),
    ])];
    assert_eq!(render(blocks), "value E\u{2004}=\u{2004}mc\u{b2}");
}

/// Render a single subscript/superscript run from a plain string of inner text.
fn sub(text: &str) -> String {
    render(vec![Block::Para(vec![Inline::Subscript(vec![
        Inline::Str(text.to_owned().into()),
    ])])])
}
fn sup(text: &str) -> String {
    render(vec![Block::Para(vec![Inline::Superscript(vec![
        Inline::Str(text.to_owned().into()),
    ])])])
}

#[test]
fn ordinary_subscript_digits_use_subscript_glyphs() {
    // A run that maps under the subscript script stays subscript and is never flipped.
    assert_eq!(sub("12"), "\u{2081}\u{2082}");
    assert_eq!(sub("-1"), "\u{208b}\u{2081}"); // ASCII hyphen-minus has a subscript glyph
    assert_eq!(sub("+1"), "\u{208a}\u{2081}");
    assert_eq!(sub("=1"), "\u{208c}\u{2081}"); // U+208C subscript equals
    assert_eq!(sub("(1)"), "\u{208d}\u{2081}\u{208e}");
}

#[test]
fn math_minus_has_no_subscript_glyph_so_the_run_flips_to_superscript() {
    // U+2212 exists only in the superscript script, so the run maps wholly to superscripts
    assert_eq!(sub("\u{2212}1"), "\u{207b}\u{00b9}");
    assert_eq!(sub("\u{2212}2"), "\u{207b}\u{00b2}");
    assert_eq!(sub("\u{2212}"), "\u{207b}");
    assert_eq!(sub("1\u{2212}2"), "\u{00b9}\u{207b}\u{00b2}");
    // The superscript script maps U+2212 directly.
    assert_eq!(sup("\u{2212}1"), "\u{207b}\u{00b9}");
}

#[test]
fn run_that_maps_under_neither_script_falls_back_to_parentheses() {
    // A letter beside the math minus maps under neither script, so the whole run is parenthesized.
    assert_eq!(sub("\u{2212}a"), "_(\u{2212}a)");
    assert_eq!(sub("1\u{2212}a"), "_(1\u{2212}a)");
}

#[test]
fn math_spaces_pass_through_the_script_mappers_unchanged() {
    // fixed-width math spaces keep a run convertible and render as themselves
    assert_eq!(
        sub("\u{2004}=\u{2004}1"),
        "\u{2004}\u{208c}\u{2004}\u{2081}"
    );
    assert_eq!(sub("1\u{2005}2"), "\u{2081}\u{2005}\u{2082}");
    assert_eq!(sub("1\u{2006}2"), "\u{2081}\u{2006}\u{2082}");
    assert_eq!(sub("1\u{2009}2"), "\u{2081}\u{2009}\u{2082}");
    assert_eq!(sub("1\u{00a0}2"), "\u{2081}\u{00a0}\u{2082}");
    assert_eq!(
        sup("\u{2004}=\u{2004}1"),
        "\u{2004}\u{207c}\u{2004}\u{00b9}"
    );
}

#[test]
fn non_space_separators_and_format_marks_do_not_pass_through() {
    // A line/paragraph separator or zero-width mark is not a space, so it forces the fallback.
    assert!(!is_script_space('\u{2028}')); // line separator (Zl)
    assert!(!is_script_space('\u{2029}')); // paragraph separator (Zp)
    assert!(!is_script_space('\u{200b}')); // zero-width space (Cf)
    assert!(!is_script_space('\u{0085}')); // next line (Cc)
    assert!(!is_script_space('\u{feff}')); // byte-order mark (Cf)
    assert_eq!(sub("1\u{2028}2"), "_(1\u{2028}2)");
    // Every ASCII whitespace control and Unicode space separator does pass through.
    for ch in [
        ' ', '\t', '\n', '\u{000b}', '\u{000c}', '\r', '\u{00a0}', '\u{1680}', '\u{2000}',
        '\u{200a}', '\u{202f}', '\u{205f}', '\u{3000}',
    ] {
        assert!(
            is_script_space(ch),
            "expected {:#x} to be a script space",
            ch as u32
        );
    }
}

#[test]
fn formatted_subscript_content_flips_the_whole_run_to_superscript() {
    // non-text, non-space content has no subscript form, so the run renders with superscripts
    let flipped = render(vec![Block::Para(vec![Inline::Subscript(vec![
        Inline::Emph(vec![Inline::Str("2".to_owned().into())]),
    ])])]);
    assert_eq!(flipped, "\u{00b2}");
    // A formatted but otherwise unmappable run still falls back.
    let fallback = render(vec![Block::Para(vec![Inline::Subscript(vec![
        Inline::Emph(vec![Inline::Str("a".to_owned().into())]),
    ])])]);
    assert_eq!(fallback, "_(a)");
}

#[test]
fn absurd_column_width_stays_bounded() {
    use carta_ast::{Caption, Cell, ColSpec, TableBody, TableFoot, TableHead};
    let cell = Cell {
        attr: Attr::default(),
        align: Alignment::AlignLeft,
        row_span: 1,
        col_span: 1,
        content: vec![Block::Para(vec![Inline::Str("x".to_owned().into())])],
    };
    let table = Table {
        attr: Attr::default(),
        caption: Caption::default(),
        col_specs: vec![ColSpec {
            align: Alignment::AlignLeft,
            width: ColWidth::ColWidth(1.9e53),
        }],
        head: TableHead::default(),
        bodies: vec![TableBody {
            attr: Attr::default(),
            row_head_columns: 0,
            head: Vec::new(),
            body: vec![Row {
                attr: Attr::default(),
                cells: vec![cell],
            }],
        }],
        foot: TableFoot::default(),
    };
    // a fractional spec far past the line must not inflate the rule into a huge allocation
    let output = render(vec![Block::Table(Box::new(table))]);
    assert!(
        output.len() < 1_000,
        "unbounded table output: {} bytes",
        output.len()
    );
}
