use super::escaping::{escape, escape_url};
use super::inline::Dimension;
use super::*;
use carta_ast::Format;
use carta_ast::{MathType, Target};

fn render(blocks: Vec<Block>) -> String {
    LatexWriter
        .write(
            &Document {
                blocks,
                ..Document::default()
            },
            &WriterOptions::default(),
        )
        .unwrap()
}

fn str_inlines(text: &str) -> Vec<Inline> {
    vec![Inline::Str(text.to_owned().into())]
}

fn render_columns(blocks: Vec<Block>, columns: usize) -> String {
    let document = Document {
        blocks,
        ..Document::default()
    };
    let mut options = WriterOptions::default();
    options.columns = Some(columns);
    LatexWriter.write(&document, &options).unwrap()
}

fn long_paragraph() -> Vec<Block> {
    let words: Vec<Inline> =
        "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi"
            .split(' ')
            .flat_map(|word| [Inline::Str(word.to_owned().into()), Inline::Space])
            .collect();
    vec![Block::Para(words)]
}

#[test]
fn custom_columns_change_paragraph_wrapping() {
    let narrow = render_columns(long_paragraph(), 20);
    let wide = render_columns(long_paragraph(), 70);
    assert!(narrow.lines().count() > wide.lines().count());
    assert!(narrow.lines().all(|line| line.chars().count() <= 20));
}

#[test]
fn omitted_columns_uses_the_default_fill_width() {
    assert_eq!(
        render(long_paragraph()),
        render_columns(long_paragraph(), 72)
    );
}

#[test]
fn dimension_parses_units() {
    assert!(matches!(Dimension::parse("96px"), Some(Dimension::Length(s)) if s == "1in"));
    assert!(matches!(Dimension::parse("96"), Some(Dimension::Length(s)) if s == "1in"));
    assert!(
        matches!(Dimension::parse("50%"), Some(Dimension::Percent(p)) if (p - 50.0).abs() < 1e-9)
    );
    assert!(matches!(Dimension::parse("2in"), Some(Dimension::Length(s)) if s == "2in"));
    assert!(matches!(Dimension::parse("3cm"), Some(Dimension::Length(s)) if s == "3cm"));
    // The unit's match key is case-folded, but the original spelling is preserved verbatim.
    assert!(matches!(Dimension::parse("3CM"), Some(Dimension::Length(s)) if s == "3CM"));
    for unit in ["mm", "pt", "pc", "em"] {
        assert!(matches!(
            Dimension::parse(&format!("4{unit}")),
            Some(Dimension::Length(_))
        ));
    }
    assert!(Dimension::parse("5xyz").is_none());
    assert!(Dimension::parse("notanumber").is_none());
}

#[test]
fn dimension_renders_against_reference() {
    assert_eq!(Dimension::Length("2in".into()).render("\\linewidth"), "2in");
    assert_eq!(
        Dimension::Percent(50.0).render("\\linewidth"),
        "0.5\\linewidth"
    );
}

#[test]
fn trim_number_drops_trailing_zeros() {
    assert_eq!(trim_number(1.0), "1");
    assert_eq!(trim_number(0.5), "0.5");
    assert_eq!(trim_number(1.230_00), "1.23");
}

#[test]
fn escape_text_metacharacters_and_glyphs() {
    assert_eq!(
        escape("a&b%c#d_e$f{g}", EscapeMode::Text),
        "a\\&b\\%c\\#d\\_e\\$f\\{g\\}"
    );
    assert_eq!(escape("a^b", EscapeMode::Text), "a\\^{}b");
    assert_eq!(escape("[x]", EscapeMode::Text), "{[}x{]}");
    assert_eq!(escape("--", EscapeMode::Text), "-\\/-");
    assert_eq!(escape("\u{a0}", EscapeMode::Text), "~");
    assert_eq!(escape("\u{2026}", EscapeMode::Text), "\\ldots{}");
    assert_eq!(escape("\u{2013}\u{2014}", EscapeMode::Text), "-----");
    // A hyphen before a smart dash is guarded against ligature merging; after a dash no guard is needed.
    assert_eq!(escape("-\u{2013}", EscapeMode::Text), "-\\/--");
    assert_eq!(escape("-\u{2014}", EscapeMode::Text), "-\\/---");
    assert_eq!(escape("\u{2013}-", EscapeMode::Text), "---");
    assert_eq!(escape("\u{2018}x\u{2019}", EscapeMode::Text), "`x'");
    assert_eq!(escape("\u{201C}x\u{201D}", EscapeMode::Text), "``x''");
}

#[test]
fn escape_copies_clean_runs_verbatim() {
    assert_eq!(escape("hello world", EscapeMode::Text), "hello world");
    assert_eq!(
        escape("caf\u{e9} au lait", EscapeMode::Text),
        "caf\u{e9} au lait"
    );
    assert_eq!(escape("a & b", EscapeMode::Text), "a \\& b");
    assert_eq!(escape("100%", EscapeMode::Text), "100\\%");
}

#[test]
fn escape_handles_triggers_at_run_edges() {
    assert_eq!(escape("&x", EscapeMode::Text), "\\&x");
    assert_eq!(escape("x&", EscapeMode::Text), "x\\&");
    assert_eq!(escape("&&", EscapeMode::Text), "\\&\\&");
    assert_eq!(escape("caf\u{e9}&", EscapeMode::Text), "caf\u{e9}\\&");
    assert_eq!(escape("&\u{e9}x", EscapeMode::Text), "\\&\u{e9}x");
}

#[test]
fn escape_lookahead_sees_past_a_verbatim_prefix() {
    // Both guards peek at the next character; a verbatim-copied prefix must not hide that lookahead.
    assert_eq!(escape("abc--def", EscapeMode::Text), "abc-\\/-def");
    assert_eq!(escape("x~y", EscapeMode::Text), "x\\textasciitilde y");
    assert_eq!(escape("abc-\u{2013}", EscapeMode::Text), "abc-\\/--");
}

#[test]
fn escape_control_words_pick_separator() {
    assert_eq!(escape("~x", EscapeMode::Text), "\\textasciitilde x");
    assert_eq!(escape("~ ", EscapeMode::Text), "\\textasciitilde{} ");
    assert_eq!(escape("~!", EscapeMode::Text), "\\textasciitilde!");
    assert_eq!(escape("~", EscapeMode::Text), "\\textasciitilde{}");
    assert_eq!(
        escape("<>|\\", EscapeMode::Text),
        "\\textless\\textgreater\\textbar\\textbackslash{}"
    );
}

#[test]
fn escape_code_mode_handles_space_and_backtick() {
    assert_eq!(escape("a b", EscapeMode::Code), "a\\ b");
    assert_eq!(escape("`", EscapeMode::Code), "\\textasciigrave{}");
    assert_eq!(escape("~", EscapeMode::Code), "\\textasciitilde{}");
}

#[test]
fn escape_url_encodes_specials() {
    assert_eq!(escape_url("a\\b"), "a/b");
    assert_eq!(escape_url("a#b%c"), "a\\#b\\%c");
    assert_eq!(escape_url("a b"), "a\\%20b");
    assert_eq!(escape_url("café"), "caf\\%C3\\%A9");
}

#[test]
fn header_levels_and_anchors() {
    assert_eq!(
        render(vec![Block::Header(1, Box::default(), str_inlines("T"))]),
        "\\section{T}"
    );
    assert_eq!(
        render(vec![Block::Header(4, Box::default(), str_inlines("T"))]),
        "\\paragraph{T}"
    );
    assert_eq!(
        render(vec![Block::Header(5, Box::default(), str_inlines("T"))]),
        "\\subparagraph{T}"
    );
    assert_eq!(
        render(vec![Block::Header(7, Box::default(), str_inlines("T"))]),
        "T"
    );
}

#[test]
fn header_unnumbered_adds_toc_line() {
    let attr = Attr {
        id: "sec".into(),
        classes: vec!["unnumbered".into()],
        ..Attr::default()
    };
    let out = render(vec![Block::Header(1, Box::new(attr), str_inlines("Title"))]);
    assert!(out.contains("\\section*{Title}\\label{sec}"));
    assert!(out.contains("\\addcontentsline{toc}{section}{Title}"));
}

#[test]
fn header_with_markup_wraps_texorpdfstring() {
    let inlines = vec![Inline::Emph(str_inlines("x"))];
    let out = render(vec![Block::Header(1, Box::default(), inlines)]);
    assert!(out.contains("\\texorpdfstring{\\emph{x}}{x}"));
}

#[test]
fn raw_block_kept_only_for_latex() {
    assert_eq!(
        render(vec![Block::RawBlock(
            Format("latex".into()),
            "\\foo\n".into()
        )]),
        "\\foo"
    );
    assert_eq!(
        render(vec![Block::RawBlock(Format("html".into()), "<b>".into())]),
        ""
    );
}

#[test]
fn empty_item_and_term_render_bare() {
    let out = render(vec![Block::BulletList(vec![vec![]])]);
    assert!(out.contains("\\item"));
    let def = render(vec![Block::DefinitionList(vec![(
        str_inlines("term"),
        vec![],
    )])]);
    assert!(def.contains("\\item[term]"));
}

#[test]
fn ordered_list_styles_set_label_and_counter() {
    let attrs = ListAttributes {
        start: 3,
        style: ListNumberStyle::UpperRoman,
        delim: ListNumberDelim::OneParen,
    };
    let out = render(vec![Block::OrderedList(
        attrs,
        vec![vec![Block::Plain(str_inlines("a"))]],
    )]);
    assert!(out.contains("\\def\\labelenumi{\\Roman{enumi})}"));
    assert!(out.contains("\\setcounter{enumi}{2}"));
}

#[test]
fn nested_ordered_lists_use_deeper_counters() {
    let inner = Block::OrderedList(
        ListAttributes {
            start: 1,
            style: ListNumberStyle::LowerAlpha,
            delim: ListNumberDelim::Period,
        },
        vec![vec![Block::Plain(str_inlines("x"))]],
    );
    let out = render(vec![Block::OrderedList(
        ListAttributes {
            start: 1,
            style: ListNumberStyle::LowerAlpha,
            delim: ListNumberDelim::Period,
        },
        vec![vec![inner]],
    )]);
    assert!(out.contains("\\alph{enumi}"));
    assert!(out.contains("\\alph{enumii}"));
}

#[test]
fn figure_with_id_and_caption() {
    let caption = Caption {
        short: None,
        long: vec![Block::Plain(str_inlines("Cap"))],
    };
    let attr = Attr {
        id: "fig".into(),
        ..Attr::default()
    };
    let out = render(vec![Block::Figure(
        Box::new(attr),
        Box::new(caption),
        vec![Block::Plain(str_inlines("body"))],
    )]);
    assert!(out.contains("\\caption{Cap}\\label{fig}"));
}

#[test]
fn figure_with_id_no_caption_emits_empty_caption() {
    let attr = Attr {
        id: "fig".into(),
        ..Attr::default()
    };
    let out = render(vec![Block::Figure(
        Box::new(attr),
        Box::default(),
        vec![Block::Plain(str_inlines("body"))],
    )]);
    assert!(out.contains("\\caption{}\\label{fig}"));
}

#[test]
fn span_with_id_emits_phantom_label() {
    let span = Inline::Span(
        Box::new(Attr {
            id: "s".into(),
            ..Attr::default()
        }),
        str_inlines("x"),
    );
    let out = render(vec![Block::Para(vec![span])]);
    assert!(out.contains("\\protect\\phantomsection\\label{s}{x}"));
}

#[test]
fn image_with_dimensions_renders_options() {
    let attr = Attr {
        attributes: vec![
            ("width".into(), "50%".into()),
            ("height".into(), "2in".into()),
        ],
        ..Attr::default()
    };
    let image = Inline::Image(
        Box::new(attr),
        str_inlines("alt"),
        Box::new(Target {
            url: "img.png".into(),
            title: String::new().into(),
        }),
    );
    let out = render(vec![Block::Para(vec![image])]);
    assert!(out.contains("width=0.5\\linewidth"));
    assert!(out.contains("height=2in"));
    assert!(out.contains("alt={alt}"));
}

#[test]
fn link_inside_underline_or_strikeout_is_boxed() {
    let link = || {
        Inline::Link(
            Box::default(),
            str_inlines("txt"),
            Box::new(Target {
                url: "http://x.com".into(),
                title: String::new().into(),
            }),
        )
    };
    // A hyperlink inside a soul command is boxed so the command cannot split it apart.
    assert!(
        render(vec![Block::Para(vec![Inline::Underline(vec![link()])])])
            .contains("\\ul{\\mbox{\\href{http://x.com}{txt}}}")
    );
    assert!(
        render(vec![Block::Para(vec![Inline::Strikeout(vec![link()])])])
            .contains("\\st{\\mbox{\\href{http://x.com}{txt}}}")
    );
    // A link that merely follows a soul span is not boxed: the context does not leak to siblings.
    let sibling = render(vec![Block::Para(vec![
        Inline::Underline(str_inlines("u")),
        Inline::Space,
        link(),
    ])]);
    assert!(sibling.contains("\\ul{u} \\href{http://x.com}{txt}"));
    assert!(!sibling.contains("\\mbox"));
    let xref = render(vec![Block::Para(vec![Inline::Underline(vec![
        Inline::Link(
            Box::default(),
            str_inlines("txt"),
            Box::new(Target {
                url: "#sec".into(),
                title: String::new().into(),
            }),
        ),
    ])])]);
    assert!(xref.contains("\\ul{\\hyperref[sec]{txt}}"));
    assert!(!xref.contains("\\mbox"));
}

#[test]
fn math_inside_soul_uses_dollar_delimiters() {
    let math = |kind| Inline::Math(kind, "x^2".into());
    let inline = render(vec![Block::Para(vec![Inline::Underline(vec![math(
        MathType::InlineMath,
    )])])]);
    assert!(inline.contains("\\ul{$x^2$}"));
    let display = render(vec![Block::Para(vec![Inline::Strikeout(vec![math(
        MathType::DisplayMath,
    )])])]);
    assert!(display.contains("\\st{$$x^2$$}"));
    assert!(render(vec![Block::Para(vec![math(MathType::InlineMath)])]).contains("\\(x^2\\)"));
}

#[test]
fn footnote_with_code_block_closes_on_own_line() {
    let note = Inline::Note(vec![Block::CodeBlock(Box::default(), "x\n".into())]);
    let out = render(vec![Block::Para(vec![Inline::Str("a".into()), note])]);
    assert!(out.contains("\\begin{Verbatim}"));
    assert!(out.contains("\n}"));
}

#[cfg(feature = "highlight")]
mod listings {
    use super::super::code::{listings_language, listings_options};
    use super::*;

    fn attr(id: &str, classes: &[&str], attributes: &[(&str, &str)]) -> Attr {
        Attr {
            id: id.into(),
            classes: classes.iter().map(|class| (*class).into()).collect(),
            attributes: attributes
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into()))
                .collect(),
        }
    }

    fn render_idiomatic(blocks: Vec<Block>) -> String {
        let mut options = WriterOptions::default();
        options.highlight.idiomatic = true;
        LatexWriter
            .write(
                &Document {
                    blocks,
                    ..Document::default()
                },
                &options,
            )
            .unwrap()
    }

    #[test]
    fn language_lookup_is_case_insensitive_and_braces_specials() {
        assert_eq!(listings_language("python"), Some("Python"));
        assert_eq!(listings_language("Python"), Some("Python"));
        assert_eq!(listings_language("C++"), Some("{C++}"));
        assert_eq!(listings_language("cpp"), Some("{C++}"));
        assert_eq!(listings_language("objective-c"), Some("C"));
        assert_eq!(listings_language("rust"), None);
    }

    #[test]
    fn options_order_language_numbers_first_attrs_label() {
        let attr = attr(
            "foo",
            &["python", "numberLines"],
            &[("startFrom", "3"), ("key", "value")],
        );
        assert_eq!(
            listings_options(&attr),
            "language=Python, numbers=left, firstnumber=3, key=value, label=foo"
        );
    }

    #[test]
    fn options_are_empty_for_a_bare_unknown_block() {
        assert_eq!(listings_options(&attr("", &["rust"], &[])), "");
    }

    #[test]
    fn a_non_alphanumeric_attribute_value_is_braced_and_escaped() {
        assert_eq!(
            listings_options(&attr("", &["python"], &[("k", "a_b")])),
            "language=Python, k={a\\_b}"
        );
    }

    #[test]
    fn the_first_mappable_class_names_the_language() {
        let out = render_idiomatic(vec![Block::CodeBlock(
            Box::new(attr("", &["rust", "python"], &[])),
            "a = 1\n".into(),
        )]);
        assert!(out.contains("\\begin{lstlisting}[language=Python]"));
    }

    #[test]
    fn an_identifier_becomes_a_label_option_not_an_anchor() {
        let out = render_idiomatic(vec![Block::CodeBlock(
            Box::new(attr("snippet", &["python"], &[])),
            "a = 1\n".into(),
        )]);
        assert!(out.contains("\\begin{lstlisting}[language=Python, label=snippet]"));
        assert!(!out.contains("phantomsection"));
    }
}
