use super::*;
use carta_ast::{Format, QuoteType};

fn render(blocks: Vec<Block>) -> String {
    CommonmarkWriter
        .write(
            &Document {
                blocks,
                ..Document::default()
            },
            &WriterOptions::default(),
        )
        .unwrap()
}

fn para(inlines: Vec<Inline>) -> Block {
    Block::Para(inlines)
}

fn str_inlines(text: &str) -> Vec<Inline> {
    vec![Inline::Str(text.to_owned().into())]
}

fn plain_item(text: &str) -> Vec<Block> {
    vec![Block::Plain(str_inlines(text))]
}

#[test]
fn ordered_list_collapses_to_decimal_and_one_paren() {
    let attrs = ListAttributes {
        start: 5,
        style: ListNumberStyle::UpperRoman,
        delim: ListNumberDelim::TwoParens,
    };
    let out = render(vec![Block::OrderedList(
        attrs,
        vec![plain_item("a"), plain_item("b")],
    )]);
    assert!(out.starts_with("5)  a"));
    assert!(out.contains("6)  b"));
}

#[test]
fn ordered_list_period_delimiter_preserved() {
    let attrs = ListAttributes {
        start: 1,
        style: ListNumberStyle::LowerAlpha,
        delim: ListNumberDelim::Period,
    };
    let out = render(vec![Block::OrderedList(attrs, vec![plain_item("x")])]);
    assert!(out.starts_with("1.  x"));
}

#[test]
fn uri_and_scheme_recognition() {
    assert!(crate::common::is_uri("http://example.com"));
    assert!(!crate::common::is_uri("noscheme"));
    assert!(!crate::common::is_uri("bogusscheme:rest"));
    assert!(crate::common::is_known_scheme("HTTP"));
    assert!(crate::common::is_known_scheme("mailto"));
    assert!(!crate::common::is_known_scheme("nope"));
}

#[test]
fn autolink_class_detection() {
    let uri_class = Attr {
        classes: vec!["uri".into()],
        ..Attr::default()
    };
    let email_class = Attr {
        classes: vec!["email".into()],
        ..Attr::default()
    };
    let other = Attr {
        classes: vec!["other".into()],
        ..Attr::default()
    };
    let with_id = Attr {
        id: "x".into(),
        classes: vec!["uri".into()],
        ..Attr::default()
    };
    assert!(is_autolink_class(&uri_class));
    assert!(is_autolink_class(&email_class));
    assert!(!is_autolink_class(&other));
    assert!(!is_autolink_class(&with_id));
}

#[test]
fn link_with_autolink_class_renders_angle_form() {
    let link = Inline::Link(
        Box::new(Attr {
            classes: vec!["uri".into()],
            ..Attr::default()
        }),
        str_inlines("http://example.com"),
        Box::new(Target {
            url: "http://example.com".into(),
            title: String::new().into(),
        }),
    );
    assert_eq!(render(vec![para(vec![link])]), "<http://example.com>");
}

#[test]
fn attributed_link_falls_back_to_html() {
    let link = Inline::Link(
        Box::new(Attr {
            id: "l".into(),
            ..Attr::default()
        }),
        str_inlines("text"),
        Box::new(Target {
            url: "/p".into(),
            title: "T".into(),
        }),
    );
    let out = render(vec![para(vec![link])]);
    assert!(out.contains("<a href=\"/p\" id=\"l\" title=\"T\">text</a>"));
}

#[test]
fn plain_link_uses_inline_destination() {
    let link = Inline::Link(
        Box::default(),
        str_inlines("text"),
        Box::new(Target {
            url: "/p".into(),
            title: "T".into(),
        }),
    );
    assert_eq!(render(vec![para(vec![link])]), "[text](/p \"T\")");
}

#[test]
fn consecutive_lists_get_comment_separator() {
    let out = render(vec![
        Block::BulletList(vec![plain_item("a")]),
        Block::BulletList(vec![plain_item("b")]),
    ]);
    assert!(out.contains("<!-- -->"));
}

#[test]
fn plain_followed_by_block_uses_single_newline() {
    let out = render(vec![
        Block::Plain(str_inlines("a")),
        Block::Plain(str_inlines("b")),
    ]);
    assert_eq!(out, "a\nb");
}

#[test]
fn empty_header_keeps_marker() {
    assert_eq!(
        render(vec![Block::Header(2, Box::default(), vec![])]),
        "## "
    );
}

#[test]
fn raw_html_block_collapses_blank_lines() {
    let out = render(vec![Block::RawBlock(
        Format("html".into()),
        "<p>\n\nx\n".into(),
    )]);
    assert_eq!(out, "<p>\n&#10;x");
}

#[test]
fn empty_blockquote_renders_bare_marker() {
    assert_eq!(quote_block(""), "> ");
    let out = render(vec![Block::BlockQuote(vec![])]);
    assert_eq!(out, "> ");
}

#[test]
fn smallcaps_and_double_emph() {
    assert_eq!(
        render(vec![para(vec![Inline::SmallCaps(str_inlines("x"))])]),
        "<span class=\"smallcaps\">x</span>"
    );
    let double = Inline::Emph(vec![Inline::Emph(str_inlines("x"))]);
    assert_eq!(render(vec![para(vec![double])]), "x");
}

#[test]
fn quoted_inline_uses_glyphs() {
    let quoted = Inline::Quoted(QuoteType::DoubleQuote, str_inlines("x"));
    assert_eq!(render(vec![para(vec![quoted])]), "\u{201c}x\u{201d}");
}

#[test]
fn span_with_attrs_wraps_in_tag() {
    let span = Inline::Span(
        Box::new(Attr {
            id: "s".into(),
            ..Attr::default()
        }),
        str_inlines("x"),
    );
    assert_eq!(render(vec![para(vec![span])]), "<span id=\"s\">x</span>");
}

#[test]
fn image_with_attrs_falls_back_to_html() {
    let image = Inline::Image(
        Box::new(Attr {
            classes: vec!["c".into()],
            ..Attr::default()
        }),
        str_inlines("alt"),
        Box::new(Target {
            url: "i.png".into(),
            title: "T".into(),
        }),
    );
    let out = render(vec![para(vec![image])]);
    assert!(out.contains("<img src=\"i.png\" title=\"T\" class=\"c\" alt=\"alt\" />"));
}

#[test]
fn long_div_opening_tag_wraps_at_fill_column() {
    let a = "a".repeat(28);
    let b = "b".repeat(28);
    let div = Block::Div(
        Box::new(Attr {
            attributes: vec![
                ("data-one".into(), a.clone().into()),
                ("data-two".into(), b.clone().into()),
            ],
            ..Attr::default()
        }),
        vec![para(str_inlines("body"))],
    );
    assert_eq!(
        render(vec![div]),
        format!("<div data-one=\"{a}\"\ndata-two=\"{b}\">\n\nbody\n\n</div>")
    );
}

#[test]
fn long_image_tag_wraps_at_fill_column() {
    let url = format!("{}.png", "x".repeat(46));
    let image = Inline::Image(
        Box::new(Attr {
            attributes: vec![
                ("width".into(), "320".into()),
                ("height".into(), "240".into()),
            ],
            ..Attr::default()
        }),
        vec![],
        Box::new(Target {
            url: url.clone().into(),
            title: String::new().into(),
        }),
    );
    assert_eq!(
        render(vec![para(vec![image])]),
        format!("<img src=\"{url}\"\nwidth=\"320\" height=\"240\" />")
    );
}

#[test]
fn attr_image_wraps_before_the_whole_tag() {
    // An `<img>` tag that does not fit moves whole to the next line, never folding mid-tag.
    let url = format!("{}.png", "a".repeat(40));
    let image = Inline::Image(
        Box::new(Attr {
            attributes: vec![("width".into(), "1in".into())],
            ..Attr::default()
        }),
        str_inlines("image"),
        Box::new(Target {
            url: url.into(),
            title: String::new().into(),
        }),
    );
    let mut inlines = Vec::new();
    for word in ["Photo", "goes", "right", "about", "here", "now"] {
        inlines.push(Inline::Str(word.into()));
        inlines.push(Inline::Space);
    }
    inlines.push(image);
    inlines.push(Inline::Space);
    inlines.push(Inline::Str("end".into()));
    let out = render(vec![para(inlines)]);
    // A line begins with the opening tag, and the tag is never split immediately after `<img`.
    assert!(out.lines().any(|line| line.starts_with("<img src=")));
    assert!(!out.contains("<img\n"));
}

#[test]
fn div_holding_only_a_dropped_raw_block_emits_no_surplus_blank_lines() {
    let div = Block::Div(
        Box::new(Attr {
            id: "embed".into(),
            ..Attr::default()
        }),
        vec![Block::RawBlock(Format("latex".into()), "\\x".into())],
    );
    assert_eq!(render(vec![div]), "<div id=\"embed\">\n\n</div>");
}

#[test]
fn fenced_code_block_with_class() {
    let attr = Attr {
        classes: vec!["rust".into()],
        ..Attr::default()
    };
    assert_eq!(
        render(vec![Block::CodeBlock(
            Box::new(attr.clone()),
            "fn x(){}\n".into()
        )]),
        "``` rust\nfn x(){}\n```"
    );
    assert_eq!(
        render(vec![Block::CodeBlock(Box::new(attr), String::new().into())]),
        "``` rust\n```"
    );
}

#[test]
fn indented_code_block_without_attrs() {
    assert_eq!(
        render(vec![Block::CodeBlock(Box::default(), "a\n\nb\n".into())]),
        "    a\n\n    b"
    );
}

#[test]
fn character_and_named_reference_detection() {
    assert!(begins_character_reference("&#65;"));
    assert!(begins_character_reference("&#x41;"));
    assert!(!begins_character_reference("&#;"));
    assert!(!begins_character_reference("&65;"));
    assert!(begins_named_entity("&amp;"));
    assert!(!begins_named_entity("&notareal;"));
    assert!(!begins_named_entity("&amp"));
}

#[test]
fn escape_str_escapes_markup_and_references() {
    assert_eq!(
        escape_str("a*b`c[d]e<f>", false),
        "a\\*b\\`c\\[d\\]e\\<f\\>"
    );
    assert_eq!(escape_str("&amp;", false), "\\&amp;");
    assert_eq!(escape_str("&#65;", false), "\\&#65;");
    assert_eq!(escape_str("a_b", false), "a_b");
    assert_eq!(escape_str("a _ b", false), "a \\_ b");
    assert_eq!(escape_str("#lead", true), "\\#lead");
}

#[test]
fn leading_escape_finds_block_starters() {
    assert_eq!(leading_escape("#x"), Some(0));
    assert_eq!(leading_escape("- x"), Some(0));
    assert_eq!(leading_escape("+ x"), Some(0));
    assert_eq!(leading_escape("-x"), None);
    assert_eq!(leading_escape("12. x"), Some(2));
    assert_eq!(leading_escape("12) x"), Some(2));
    assert_eq!(leading_escape("12.x"), None);
    assert_eq!(leading_escape("1234567890. x"), None);
    assert_eq!(leading_escape("abc"), None);
}

#[test]
fn paren_marker_close_finds_decimal_paren_markers() {
    assert_eq!(paren_marker_close("(1) x"), Some(2));
    assert_eq!(paren_marker_close("(12) x"), Some(3));
    assert_eq!(paren_marker_close("(1)"), Some(2));
    assert_eq!(paren_marker_close("(1)x"), None);
    assert_eq!(paren_marker_close("(a) x"), None);
    assert_eq!(paren_marker_close("1) x"), None);
    assert_eq!(paren_marker_close("(1234567890) x"), None);
}

#[test]
fn escape_str_escapes_decimal_paren_markers_only_at_line_start() {
    assert_eq!(escape_str("(1) item", true), "\\(1\\) item");
    assert_eq!(escape_str("(1) item", false), "(1) item");
    assert_eq!(escape_str("(a) item", true), "(a) item");
}

#[test]
fn word_boundary_for_underscore() {
    assert!(!is_word_boundary(Some('a'), Some('b')));
    assert!(is_word_boundary(Some('a'), Some(' ')));
    assert!(is_word_boundary(None, Some('a')));
}

#[test]
fn code_block_indented_then_list_separates() {
    assert!(needs_separator(
        &Block::BulletList(vec![plain_item("a")]),
        &Block::CodeBlock(Box::default(), "x".into())
    ));
    assert!(!needs_separator(&Block::Para(vec![]), &Block::Para(vec![])));
}

#[test]
fn title_attr_helper() {
    assert_eq!(title_attr(&carta_ast::Text::default()), "");
    assert_eq!(title_attr(&"T".into()), " title=\"T\"");
}

fn inline_math(tex: &str) -> Inline {
    Inline::Math(MathType::InlineMath, tex.to_owned().into())
}

fn display_math(tex: &str) -> Inline {
    Inline::Math(MathType::DisplayMath, tex.to_owned().into())
}

#[test]
fn convertible_math_uses_inline_markup() {
    // Math spacing: `U+2005` around `+`, `U+2004` around `=`.
    assert_eq!(
        render(vec![para(vec![inline_math("a^2 + b^2 = c^2")])]),
        "*a*<sup>2</sup>\u{2005}+\u{2005}*b*<sup>2</sup>\u{2004}=\u{2004}*c*<sup>2</sup>"
    );
}

#[test]
fn display_math_shares_inline_conversion() {
    // `\,` is a thin space (`U+2006`) in the converted tree.
    assert_eq!(
        render(vec![para(vec![display_math("\\int_0^1 x \\, dx")])]),
        "\u{222b}<sub>0</sub><sup>1</sup>*x*\u{2006}*d**x*"
    );
}

#[test]
fn unconvertible_inline_math_falls_back_to_single_dollars() {
    // The running-text path escapes a word-boundary `_`; the `$` delimiters stay literal.
    assert_eq!(
        render(vec![para(vec![inline_math("\\sum_{i=1}^n a_i")])]),
        "$\\sum\\_{i=1}^n a_i$"
    );
}

#[test]
fn unconvertible_display_math_falls_back_to_double_dollars() {
    assert_eq!(
        render(vec![para(vec![display_math("\\sqrt{x}")])]),
        "$$\\sqrt{x}$$"
    );
}

#[test]
fn inline_math_fallback_trims_edge_whitespace() {
    // Edge whitespace is stripped before wrapping in `$…$`; interior whitespace stays.
    assert_eq!(
        render(vec![para(vec![inline_math("\\sqrt{x} ")])]),
        "$\\sqrt{x}$"
    );
    assert_eq!(
        render(vec![para(vec![inline_math(" \\sqrt{x}")])]),
        "$\\sqrt{x}$"
    );
    assert_eq!(
        render(vec![para(vec![inline_math("  \\sqrt{x}  ")])]),
        "$\\sqrt{x}$"
    );
    assert_eq!(
        render(vec![para(vec![inline_math("\\sqrt{x}   y")])]),
        "$\\sqrt{x}   y$"
    );
}

#[test]
fn display_math_fallback_keeps_edge_whitespace() {
    // Display fallback wraps the source as written; only inline math trims its edges.
    assert_eq!(
        render(vec![para(vec![display_math("\\sqrt{x} ")])]),
        "$$\\sqrt{x} $$"
    );
    assert_eq!(
        render(vec![para(vec![display_math(" \\sqrt{x}")])]),
        "$$ \\sqrt{x}$$"
    );
}

#[test]
fn inline_math_fallback_of_lone_backslash_escapes() {
    // `\` wraps to `$\$`, escaped to `$\\`; a bailed `\ ` trims to the same body.
    assert_eq!(render(vec![para(vec![inline_math("\\")])]), "$\\\\");
}

#[test]
fn empty_math_emits_nothing() {
    // Empty or whitespace-only math contributes no output; the flanking spaces collapse.
    let out = render(vec![para(vec![
        Inline::Str("a".into()),
        Inline::Space,
        inline_math("  "),
        Inline::Space,
        Inline::Str("b".into()),
    ])]);
    assert_eq!(out, "a b");
    assert_eq!(render(vec![para(vec![display_math("")])]), "");
}

#[test]
fn figure_renders_as_html_fallback() {
    let caption = carta_ast::Caption {
        short: None,
        long: vec![Block::Plain(str_inlines("a caption"))],
    };
    let image = Inline::Image(
        Box::default(),
        str_inlines("a caption"),
        Box::new(Target {
            url: "pic.png".into(),
            title: "fig title".into(),
        }),
    );
    let figure = Block::Figure(
        Box::default(),
        Box::new(caption),
        vec![Block::Plain(vec![image])],
    );
    assert_eq!(
        render(vec![figure]),
        "<figure>\n<img src=\"pic.png\" title=\"fig title\" alt=\"a caption\" />\n\
             <figcaption aria-hidden=\"true\">a caption</figcaption>\n</figure>"
    );
}

#[test]
fn dimensioned_image_falls_back_to_html_img() {
    let image = Inline::Image(
        Box::new(Attr {
            attributes: vec![("width".into(), "200".into())],
            ..Attr::default()
        }),
        str_inlines("alt"),
        Box::new(Target {
            url: "pic.png".into(),
            title: String::new().into(),
        }),
    );
    assert_eq!(
        render(vec![para(vec![image])]),
        "<img src=\"pic.png\" width=\"200\" alt=\"alt\" />"
    );
}

#[test]
fn attrless_image_stays_markdown() {
    let image = Inline::Image(
        Box::default(),
        str_inlines("alt"),
        Box::new(Target {
            url: "pic.png".into(),
            title: String::new().into(),
        }),
    );
    assert_eq!(render(vec![para(vec![image])]), "![alt](pic.png)");
}

fn dimensioned_image(attributes: Vec<(String, String)>) -> Inline {
    Inline::Image(
        Box::new(Attr {
            attributes: attributes
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
            ..Attr::default()
        }),
        str_inlines("alt"),
        Box::new(Target {
            url: "pic.png".into(),
            title: String::new().into(),
        }),
    )
}

#[test]
fn pixel_dimensions_strip_px_and_render_as_attributes() {
    let image = dimensioned_image(vec![("width".into(), "200px".into())]);
    assert_eq!(
        render(vec![para(vec![image])]),
        "<img src=\"pic.png\" width=\"200\" alt=\"alt\" />"
    );
    let both = dimensioned_image(vec![
        ("width".into(), "200".into()),
        ("height".into(), "100".into()),
    ]);
    assert_eq!(
        render(vec![para(vec![both])]),
        "<img src=\"pic.png\" width=\"200\" height=\"100\" alt=\"alt\" />"
    );
}

#[test]
fn percent_and_length_dimensions_become_style() {
    let percent = dimensioned_image(vec![("width".into(), "50%".into())]);
    assert_eq!(
        render(vec![para(vec![percent])]),
        "<img src=\"pic.png\" style=\"width:50.0%\" alt=\"alt\" />"
    );
    let length = dimensioned_image(vec![("width".into(), "5cm".into())]);
    assert_eq!(
        render(vec![para(vec![length])]),
        "<img src=\"pic.png\" style=\"width:5cm\" alt=\"alt\" />"
    );
}

#[test]
fn mixed_pixel_and_style_dimensions_separate_correctly() {
    let image = dimensioned_image(vec![
        ("width".into(), "200".into()),
        ("height".into(), "50%".into()),
    ]);
    assert_eq!(
        render(vec![para(vec![image])]),
        "<img src=\"pic.png\" style=\"height:50.0%\" width=\"200\" alt=\"alt\" />"
    );
}

#[test]
fn unrecognized_dimension_is_dropped() {
    let image = dimensioned_image(vec![("width".into(), "4ex".into())]);
    assert_eq!(
        render(vec![para(vec![image])]),
        "<img src=\"pic.png\" alt=\"alt\" />",
        "the unparsable dimension is dropped but the attributed image keeps its HTML form"
    );
}

// `=` carries `U+2004` math spacing on each side in the converted tree.
const X_EQ_Y: &str = "*x*\u{2004}=\u{2004}*y*";

#[test]
fn display_math_is_set_off_on_its_own_line() {
    let out = render(vec![para(vec![
        Inline::Str("before".into()),
        Inline::Space,
        display_math("x=y"),
        Inline::Space,
        Inline::Str("after".into()),
    ])]);
    assert_eq!(out, format!("before\n{X_EQ_Y}\nafter"));
}

#[test]
fn display_math_breaks_at_paragraph_edges_collapse() {
    // The leading break drops at the paragraph's start; the trailing one trims at its end.
    let at_start = render(vec![para(vec![
        display_math("x=y"),
        Inline::Space,
        Inline::Str("after".into()),
    ])]);
    assert_eq!(at_start, format!("{X_EQ_Y}\nafter"));
    let at_end = render(vec![para(vec![
        Inline::Str("before".into()),
        Inline::Space,
        display_math("x=y"),
    ])]);
    assert_eq!(at_end, format!("before\n{X_EQ_Y}"));
    let alone = render(vec![para(vec![display_math("x=y")])]);
    assert_eq!(alone, X_EQ_Y);
}

#[test]
fn inline_math_stays_on_the_line() {
    let out = render(vec![para(vec![
        Inline::Str("before".into()),
        Inline::Space,
        inline_math("x=y"),
        Inline::Space,
        Inline::Str("after".into()),
    ])]);
    assert_eq!(out, format!("before {X_EQ_Y} after"));
}

#[test]
fn unconvertible_display_math_still_breaks_and_falls_back() {
    let out = render(vec![para(vec![
        Inline::Str("before".into()),
        Inline::Space,
        display_math("\\sqrt{x}"),
        Inline::Space,
        Inline::Str("after".into()),
    ])]);
    assert_eq!(out, "before\n$$\\sqrt{x}$$\nafter");
}

#[test]
fn empty_display_math_still_sets_off_a_break() {
    let out = render(vec![para(vec![
        Inline::Str("before".into()),
        Inline::Space,
        display_math("   "),
        Inline::Space,
        Inline::Str("after".into()),
    ])]);
    assert_eq!(out, "before\nafter");
}

#[test]
fn consecutive_display_math_each_take_a_line() {
    let out = render(vec![para(vec![
        Inline::Str("a".into()),
        Inline::Space,
        display_math("x=y"),
        Inline::Space,
        display_math("p=q"),
        Inline::Space,
        Inline::Str("b".into()),
    ])]);
    assert_eq!(out, format!("a\n{X_EQ_Y}\n*p*\u{2004}=\u{2004}*q*\nb"));
}
