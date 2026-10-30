use super::*;

use carta_ast::{Attr, Format, Inline, MathType, Target, to_plain_text};

use super::support::substitute_macro;

/// Reader defaults for LaTeX: smart punctuation, header identifiers from title text, and macro
/// expansion. Mirrors the format's default extension set so unit tests observe the same behavior
/// as an ordinary conversion.
fn latex_defaults() -> Extensions {
    Extensions::from_list(&[
        Extension::Smart,
        Extension::AutoIdentifiers,
        Extension::LatexMacros,
    ])
}

fn parse(input: &str) -> Vec<Block> {
    parse_ext(input, latex_defaults())
}

fn parse_ext(input: &str, extensions: Extensions) -> Vec<Block> {
    let mut options = ReaderOptions::default();
    options.extensions = extensions;
    LatexReader
        .read(input, &options)
        .expect("latex reader does not fail")
        .blocks
}

#[test]
fn ascii_heading_ids_fold_titles_and_keep_explicit_labels() {
    let mut extensions = latex_defaults();
    extensions.insert(Extension::AsciiIdentifiers);
    let blocks = parse_ext(
        r"\section{Café Über}\section{Cafe Uber}\section{Café}\label{Café}\section{ı Å K}",
        extensions,
    );
    let ids: Vec<&str> = blocks
        .iter()
        .filter_map(|block| match block {
            Block::Header(_, attributes, _) => Some(attributes.id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(ids, ["cafe-uber", "cafe-uber-1", "Café", "i-a-k"]);
}

#[test]
fn tab_stops_apply_to_verbatim_regions() {
    for (width, text) in [(0, "a\tb"), (3, "a  b"), (4, "a   b"), (8, "a       b")] {
        let mut options = ReaderOptions::default();
        options.tab_stop = width;
        assert_eq!(
            LatexReader
                .read("\\begin{verbatim}\na\tb\n\\end{verbatim}\n", &options)
                .unwrap()
                .blocks,
            vec![Block::CodeBlock(Box::default(), text.into())]
        );
    }
}

#[test]
fn standalone_raw_commands_keep_arguments_and_end_at_a_new_line() {
    let blocks = parse_ext(
        "\\unknown[option] {argument}\nText.\n",
        Extensions::from_list(&[Extension::RawTex]),
    );
    assert_eq!(
        blocks,
        vec![
            Block::RawBlock(
                Format("latex".into()),
                "\\unknown[option] {argument}".into()
            ),
            Block::Para(vec![Inline::Str("Text.".into())]),
        ]
    );
    assert_eq!(
        parse_ext(
            "\\unknown[option]\t{argument}",
            Extensions::from_list(&[Extension::RawTex])
        ),
        vec![Block::RawBlock(
            Format("latex".into()),
            "\\unknown[option]    {argument}".into()
        )]
    );
}

#[test]
fn inline_raw_commands_keep_consumed_spaces_and_nested_formats() {
    let extensions = Extensions::from_list(&[Extension::RawTex]);
    assert_eq!(
        parse_ext("Before \\unknown[option] after.", extensions),
        vec![Block::Para(vec![
            Inline::Str("Before".into()),
            Inline::Space,
            Inline::RawInline(Format("latex".into()), "\\unknown[option] ".into()),
            Inline::Str("after.".into()),
        ])]
    );
    assert_eq!(
        parse_ext("\\emph{\\unknown{argument}}", extensions),
        vec![Block::Para(vec![Inline::Emph(vec![Inline::RawInline(
            Format("latex".into()),
            "\\unknown{argument}".into()
        ),])])]
    );
}

#[test]
fn ignored_commands_keep_the_next_paragraph_intact() {
    assert_eq!(
        parse("\\unknown[option] {argument}\n\nText.\n"),
        vec![Block::Para(vec![Inline::Str("Text.".into())])]
    );
    let extensions = Extensions::from_list(&[Extension::RawTex]);
    assert_eq!(
        parse_ext("\\begin{quote}\\unknown{argument}\\end{quote}", extensions),
        vec![Block::BlockQuote(vec![Block::Para(vec![
            Inline::RawInline(Format("latex".into()), "\\unknown{argument}".into())
        ])])]
    );
}

fn reference_texts(blocks: Vec<Block>) -> Vec<String> {
    let mut document = Document {
        blocks,
        ..Document::default()
    };
    let mut labels = Vec::new();
    carta_core::walk::for_each_document_inline(&mut document, &mut |inline| {
        if let Inline::Link(_, label, _) = inline {
            labels.push(to_plain_text(label));
        }
    });
    labels
}

#[test]
fn references_resolve_forward_to_explicit_heading_labels() {
    let source = "\\ref{second} \\ref{auto}\n\n\\section{Auto}\n\\section{First}\\label{first}\n\\subsection{Second}\\label{second}\n\\ref{first} \\eqref{second} \\autoref{second} \\Cref{second}";
    assert_eq!(
        reference_texts(parse(source)),
        ["2.1", "[auto]", "2", "[second]", "2.1", "2.1"]
    );
}

#[test]
fn captions_advance_separate_counters_and_chapters_reset_them() {
    let source = "\\begin{figure}\\caption{Anonymous}\\end{figure}\n\\begin{figure}\\caption{Named}\\label{figure}\\end{figure}\n\\begin{figure}\\label{bare}\\end{figure}\n\\begin{table}\\caption{Table}\\label{table}\\begin{tabular}{l}Cell\\\\\\end{tabular}\\end{table}\n\\ref{figure} \\ref{bare} \\ref{table}\n\\chapter{Chapter}\n\\begin{figure}\\caption{Chapter figure}\\label{chapter-figure}\\end{figure}\n\\ref{chapter-figure}";
    assert_eq!(reference_texts(parse(source)), ["2", "[bare]", "1", "1.1"]);
}

#[test]
fn part_references_keep_roman_numbers() {
    let source = "\\part{One}\\label{one}\\part{Two}\\part{Three}\\part{Four}\\label{four}\\ref{one} \\ref{four}";
    assert_eq!(reference_texts(parse(source)), ["I", "IV"]);
}

#[test]
fn reference_commands_keep_raw_source_when_requested() {
    let source = "\\ref{x} \\eqref{x} \\autoref{x} \\cref{x} \\Cref{x} \\pageref{x} \\ref*{x}";
    let blocks = parse_ext(source, Extensions::from_list(&[Extension::RawTex]));
    let mut document = Document {
        blocks,
        ..Document::default()
    };
    let mut commands = Vec::new();
    carta_core::walk::for_each_document_inline(&mut document, &mut |inline| {
        if let Inline::RawInline(format, command) = inline {
            assert_eq!(format.0, "latex");
            commands.push(command.to_string());
        }
    });
    assert_eq!(
        commands,
        [
            "\\ref{x}",
            "\\eqref{x}",
            "\\autoref{x}",
            "\\cref{x}",
            "\\Cref{x}",
            "\\pageref{x}",
            "\\ref*{x}"
        ]
    );
}

#[test]
fn long_tables_keep_captions_labels_and_caption_line_breaks() {
    let source = "\\begin{longtable}{ll}\\caption[Short]{First \\\\ \\emph{second}}\\label{table}\\\\ A & B \\\\\\end{longtable}\n\\ref{table}";
    let blocks = parse(source);
    let Some(Block::Table(table)) = blocks.first() else {
        panic!()
    };
    assert_eq!(table.attr.id, "table");
    assert_eq!(table.caption.short, Some(vec![Inline::Str("Short".into())]));
    assert_eq!(
        table.caption.long,
        vec![Block::Plain(vec![
            Inline::Str("First".into()),
            Inline::LineBreak,
            Inline::Emph(vec![Inline::Str("second".into())])
        ])]
    );
    assert_eq!(table.bodies.first().unwrap().body.len(), 1);
    assert_eq!(reference_texts(blocks), ["1"]);
}

#[test]
fn repeated_long_table_captions_keep_each_caption_text() {
    let source = "\\begin{longtable}{l}\\caption{First}\\\\ A\\\\\\caption{Second}\\\\ B\\\\\\end{longtable}";
    let blocks = parse(source);
    let Some(Block::Table(table)) = blocks.first() else {
        panic!()
    };
    assert_eq!(
        table.caption.long,
        vec![
            Block::Plain(vec![Inline::Str("First".into())]),
            Block::Plain(vec![Inline::Str("Second".into())])
        ]
    );
    assert_eq!(table.bodies.first().unwrap().body.len(), 2);
}

#[test]
fn unnumbered_long_table_captions_keep_their_text() {
    let source = "\\begin{longtable}{l}\\caption*{Caption}\\label{table}\\\\ A\\\\\\end{longtable}\\ref{table}";
    let blocks = parse(source);
    let Some(Block::Table(table)) = blocks.first() else {
        panic!()
    };
    assert_eq!(table.attr.classes, ["unnumbered"]);
    assert_eq!(
        table.caption.long,
        vec![Block::Plain(vec![Inline::Str("Caption".into())])]
    );
    assert_eq!(reference_texts(blocks), ["[table]"]);
}

#[test]
fn table_verbatim_text_keeps_separators_and_environment_markers() {
    for code in ["% & \\\\", "\\end{tabular}", "\\begin{tabular}", "{ } &"] {
        let source = format!("\\begin{{tabular}}{{ll}}\\verb|{code}|&Other\\end{{tabular}}");
        let blocks = parse(&source);
        let Some(Block::Table(table)) = blocks.first() else {
            panic!()
        };
        assert_eq!(blocks.len(), 1);
        let cells = &table.bodies.first().unwrap().body.first().unwrap().cells;
        assert_eq!(cells.len(), 2);
        assert_eq!(
            cells.first().unwrap().content,
            [Block::Plain(vec![Inline::Code(
                Box::default(),
                code.into()
            )])]
        );
    }
}

#[test]
fn nested_tables_keep_inner_rows_and_outer_cells() {
    for (open, close) in [("", ""), ("{", "}"), ("{{", "}}")] {
        let source = format!(
            "\\begin{{tabular}}{{ll}}{open}\\begin{{tabular}}{{c}}Inner\\\\Next\\end{{tabular}}{close}&Outer\\\\Last&Tail\\end{{tabular}}"
        );
        let blocks = parse(&source);
        let Some(Block::Table(outer)) = blocks.first() else {
            panic!()
        };
        assert_eq!(blocks.len(), 1);
        let rows = &outer.bodies.first().unwrap().body;
        assert_eq!(rows.len(), 2);
        assert_eq!(rows.first().unwrap().cells.len(), 2);
        let content = &rows.first().unwrap().cells.first().unwrap().content;
        let Some(Block::Table(inner)) = content.first() else {
            panic!()
        };
        assert_eq!(inner.bodies.first().unwrap().body.len(), 2);
    }
}

#[test]
fn table_comments_and_escaped_separators_keep_cell_content() {
    let blocks = parse(
        "\\begin{tabular}{ll}A\\&B% ignored & \\\\\nC&D\\\\% \\end{tabular}\nE&F\\end{tabular}",
    );
    let Some(Block::Table(table)) = blocks.first() else {
        panic!()
    };
    let rows = &table.bodies.first().unwrap().body;
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.first().unwrap().cells.first().unwrap().content,
        [Block::Plain(vec![
            Inline::Str("A&B".into()),
            Inline::SoftBreak,
            Inline::Str("C".into()),
        ])]
    );
}

#[test]
fn multirow_cells_keep_text_after_the_command() {
    let blocks = parse("\\begin{tabular}{ll}\\multirow{2}{*}{Name} suffix&A\\\\&B\\end{tabular}");
    let Some(Block::Table(table)) = blocks.first() else {
        panic!()
    };
    let cell = table
        .bodies
        .first()
        .unwrap()
        .body
        .first()
        .unwrap()
        .cells
        .first()
        .unwrap();
    assert_eq!(cell.row_span, 2);
    assert_eq!(
        cell.content,
        [Block::Plain(vec![
            Inline::Str("Name".into()),
            Inline::Space,
            Inline::Str("suffix".into()),
        ])]
    );
}

#[test]
fn table_cells_span_rows_without_losing_their_content() {
    for source in [
        "\\begin{tabular}{ll}\\multirow{2}{*}{\\emph{Name}}&One\\\\&Two\\end{tabular}",
        "\\begin{tabular}{ll}\\multirow[t]{2}[1]{2cm}[1ex]{\\emph{Name}}&One\\\\&Two\\end{tabular}",
        "\\begin{tabular}{ll}&One\\\\\\multirow{-2}{*}{\\emph{Name}}&Two\\end{tabular}",
    ] {
        let blocks = parse(source);
        let Some(Block::Table(table)) = blocks.first() else {
            panic!()
        };
        let rows = &table.bodies.first().unwrap().body;
        let first = rows.first().unwrap().cells.first().unwrap();
        assert_eq!(first.row_span, 2);
        assert_eq!(
            first.content,
            [Block::Plain(vec![Inline::Emph(vec![Inline::Str(
                "Name".into()
            )])])]
        );
        assert_eq!(rows.get(1).unwrap().cells.len(), 1);
    }
}

#[test]
fn preamble_content_is_kept_before_the_document_body() {
    let source = "\\title{Title}Before\\begin{document}After\\end{document}Ignored";
    assert_eq!(
        parse(source),
        vec![
            Block::Para(vec![Inline::Str("Before".into())]),
            Block::Para(vec![Inline::Str("After".into())]),
        ]
    );
}

#[test]
fn preamble_raw_commands_keep_their_block_boundaries() {
    for (separator, block) in [
        (
            "",
            Block::Para(vec![Inline::RawInline(
                Format("latex".into()),
                "\\institute{Example}".into(),
            )]),
        ),
        (
            "\n",
            Block::RawBlock(Format("latex".into()), "\\institute{Example}".into()),
        ),
    ] {
        let source =
            format!("\\institute{{Example}}{separator}\\begin{{document}}Body\\end{{document}}");
        assert_eq!(
            parse_ext(&source, Extensions::from_list(&[Extension::RawTex])),
            vec![block, Block::Para(vec![Inline::Str("Body".into())])]
        );
    }
}

#[test]
fn raised_boxes_keep_their_content_with_optional_dimensions() {
    for dimensions in ["", "[2ex]", "[2ex][0pt]", " [2ex] [0pt] "] {
        let source = format!("Before \\raisebox{{1ex}}{dimensions}{{\\emph{{kept}}}} after.");
        assert_eq!(
            parse(&source),
            vec![Block::Para(vec![
                Inline::Str("Before".into()),
                Inline::Space,
                Inline::Emph(vec![Inline::Str("kept".into())]),
                Inline::Space,
                Inline::Str("after.".into()),
            ])]
        );
    }
}

#[test]
fn raised_boxes_keep_images_and_raw_source() {
    let source = "\\raisebox{1ex}[2ex][0pt]{\\includegraphics{image.png}}";
    let blocks = parse(source);
    let Some(Block::Para(inlines)) = blocks.first() else {
        panic!()
    };
    assert!(
        matches!(inlines.first(), Some(Inline::Image(_, _, target)) if target.url == "image.png")
    );
    assert_eq!(
        parse_ext(source, Extensions::from_list(&[Extension::RawTex])),
        vec![Block::RawBlock(Format("latex".into()), source.into())]
    );
}

fn attr(attributes: Vec<(String, String)>) -> Attr {
    Attr {
        id: carta_ast::Text::default(),
        classes: Vec::new(),
        attributes: attributes
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect(),
    }
}

#[test]
fn substitute_macro_preserves_non_parameter_hash_zero() {
    let expanded = substitute_macro("a#0b", &["Y".to_owned()]);
    assert_eq!(expanded, "a#0b");
}

#[test]
fn equation_environment_is_display_math_with_verbatim_body() {
    let blocks = parse("\\begin{equation}\n  f(x) = x + 1\n\\end{equation}\n");
    assert_eq!(
        blocks,
        vec![Block::Para(vec![Inline::Math(
            MathType::DisplayMath,
            "\\begin{equation}\n  f(x) = x + 1\n\\end{equation}"
                .to_owned()
                .into(),
        )])],
    );
}

#[test]
fn ref_becomes_tagged_link_with_bracketed_label() {
    let blocks = parse("See Section~\\ref{sec:intro}.\n");
    assert_eq!(
        blocks,
        vec![Block::Para(vec![
            Inline::Str("See".to_owned().into()),
            Inline::Space,
            Inline::Str("Section\u{a0}".to_owned().into()),
            Inline::Link(
                Box::new(attr(vec![
                    ("reference-type".to_owned(), "ref".to_owned()),
                    ("reference".to_owned(), "sec:intro".to_owned()),
                ])),
                vec![Inline::Str("[sec:intro]".to_owned().into())],
                Box::new(Target {
                    url: "#sec:intro".to_owned().into(),
                    title: carta_ast::Text::default(),
                }),
            ),
            Inline::Str(".".to_owned().into()),
        ])],
    );
}

#[test]
fn cref_variants_carry_their_reference_kind() {
    let kinds = |input: &str| match parse(input).into_iter().next().map(Block::into_parts) {
        Some(carta_ast::BlockParts::Para(inlines)) => inlines
            .into_iter()
            .filter_map(|inline| match inline.into_parts() {
                carta_ast::InlineParts::Link(attr, _, _) => attr
                    .attributes
                    .into_iter()
                    .find(|(k, _)| k == "reference-type")
                    .map(|(_, v)| v),
                _ => None,
            })
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    assert_eq!(kinds("\\cref{a}"), vec!["ref+label".to_owned()]);
    assert_eq!(kinds("\\autoref{a}"), vec!["ref+label".to_owned()]);
    assert_eq!(kinds("\\Cref{a}"), vec!["ref+Label".to_owned()]);
}

#[test]
fn includegraphics_textwidth_fraction_becomes_percent_width() {
    let blocks = parse("x \\includegraphics[width=0.5\\textwidth]{diagram.png} y\n");
    assert_eq!(
        blocks,
        vec![Block::Para(vec![
            Inline::Str("x".to_owned().into()),
            Inline::Space,
            Inline::Image(
                Box::new(attr(vec![("width".to_owned(), "50%".to_owned())])),
                vec![Inline::Str("image".to_owned().into())],
                Box::new(Target {
                    url: "diagram.png".to_owned().into(),
                    title: carta_ast::Text::default(),
                }),
            ),
            Inline::Space,
            Inline::Str("y".to_owned().into()),
        ])],
    );
}

#[test]
fn macro_definition_preserved_verbatim_when_expansion_disabled() {
    let ext = Extensions::from_list(&[Extension::Smart, Extension::AutoIdentifiers]);
    let blocks = parse_ext("\\newcommand{\\foo}{bar}\n", ext);
    assert_eq!(
        blocks,
        vec![Block::RawBlock(
            Format("latex".to_owned().into()),
            "\\newcommand{\\foo}{bar}".to_owned().into(),
        )],
    );
}

/// The concatenated plain text of every paragraph a source parses to.
fn plain_text(input: &str) -> String {
    parse(input)
        .iter()
        .filter_map(|block| match block {
            Block::Para(inlines) => Some(to_plain_text(inlines)),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn simple_macro_expands_to_its_body() {
    assert_eq!(plain_text("\\newcommand{\\x}{Y}\n\n\\x\n"), "Y");
}

#[test]
fn let_copies_macro_arguments_and_current_definition() {
    assert_eq!(
        plain_text(
            r"\newcommand{\word}[2][Hello]{#1 #2}\let\alias\word\renewcommand{\word}{changed}\alias{reader} \alias[Dear]{writer} \word"
        ),
        "Hello reader Dear writer changed"
    );
    assert_eq!(
        plain_text(
            r"\newcommand{\word}{first}\let\alias=\word\renewcommand{\word}{second}\alias\word"
        ),
        "firstsecond"
    );
}

#[test]
fn let_preserves_builtin_and_undefined_token_meanings() {
    assert_eq!(
        parse(r"\let\alias=\textbf\let\copy\alias\renewcommand{\textbf}[1]{changed}\copy{bold}"),
        parse(r"\textbf{bold}")
    );
    assert_eq!(plain_text(r"\let\alias=\LaTeX\alias text"), "LaTeXtext");
    for (source, expanded) in [
        (
            r"Before \let\alias=\textbf\alias{bold} after.",
            r"Before \textbf{bold} after.",
        ),
        (
            r"\textit{\let\alias=\textbf\alias{bold}}",
            r"\textit{\textbf{bold}}",
        ),
        (r"\let\alias=\section\alias{Title}", r"\section{Title}"),
        (
            r"\let\alias=\begin\alias{quote}word\end{quote}",
            r"\begin{quote}word\end{quote}",
        ),
    ] {
        assert_eq!(parse(source), parse(expanded), "{source}");
    }
    assert!(
        parse(r"\newcommand{\alias}{old}\let\alias=\unknown\newcommand{\unknown}{later}\alias")
            .is_empty()
    );
    let mut extensions = latex_defaults();
    extensions.insert(Extension::RawTex);
    assert_eq!(
        parse_ext(
            r"\let\alias=\unknown\newcommand{\unknown}{later}\alias",
            extensions
        ),
        parse_ext(r"\unknown", extensions)
    );
}

#[test]
fn let_accepts_character_tokens_and_control_symbols() {
    assert_eq!(plain_text(r"\let\alias=a\alias\alias"), "aa");
    assert_eq!(plain_text(r"\let\alias=\%\alias\alias"), "%%");
    assert_eq!(plain_text(r"\let\!=a\!\!"), "aa");
    assert_eq!(
        plain_text(r"\newcommand{\show}[1]{[#1]}\let\alias\show\alias\%"),
        "[%]"
    );
    assert_eq!(
        plain_text("\\let\\alias % target\n = % value\n a\\alias\\alias"),
        "aa"
    );
}

#[test]
fn let_definition_keeps_its_source_when_expansion_is_disabled() {
    let source = r"\let\alias=\textbf";
    assert_eq!(
        parse_ext(source, Extensions::empty()),
        vec![Block::RawBlock(Format("latex".into()), source.into())]
    );
}

#[test]
fn math_expands_macros_and_keeps_control_word_boundaries() {
    for (source, expanded) in [
        (r"\newcommand{\word}[1]{x+#1}$\word{y}$", "$x+y$"),
        (
            r"\let\alias=\alpha\newcommand{\alpha}{b}$\alias \alpha$",
            r"$\alpha b$",
        ),
        (r"\newcommand{\word}{\alpha}$\word b$", r"$\alpha b$"),
        (r"\newcommand{\word}{x}$\frac{\word}{y}$", r"$\frac{x}{y}$"),
        (r"\let\alias=\alpha\[\alias\]", r"\[\alpha\]"),
        (
            r"\let\alias=\alpha\begin{equation}\alias\end{equation}",
            r"\begin{equation}\alpha\end{equation}",
        ),
        (
            r"\let\alias=\footnote\newcommand{\word}{note}Text\alias{\word}",
            r"Text\footnote{note}",
        ),
    ] {
        assert_eq!(parse(source), parse(expanded), "{source}");
    }
    assert_eq!(
        parse_ext(r"\let\alias=\alpha $\alias$", Extensions::empty()),
        vec![
            Block::RawBlock(Format("latex".into()), r"\let\alias=\alpha ".into()),
            Block::Para(vec![Inline::Math(MathType::InlineMath, r"\alias".into())]),
        ]
    );
}

#[test]
fn delimited_macros_read_complete_arguments_and_preserve_groups() {
    for (source, expected) in [
        (
            r"\def\pair#1,#2;{#2 and #1}\pair first,second;",
            "second and first",
        ),
        (r"\def\word#1END{[#1]}\word Hello END", "[Hello ]"),
        (r"\def\pair#1,#2;{[#1][#2]}\pair {a,b},c;", "[a,b][c]"),
        (r"\def\pair#1,#2;{[#1][#2]}\pair ,;", "[][]"),
        (r"\def\word#1,#2;{[#1][#2]}\word a\,b,c;", "[a\u{2006}b][c]"),
        (r"\def\word#1;{[#1]}\word {a}{b};", "[ab]"),
        (r"\def\word#1 #2;{#2 #1}\word first second;", "second first"),
        (
            r"\def\word#1,#2;{#1:#2}\let\alias\word\def\word{changed}\alias first,second;",
            "first:second",
        ),
    ] {
        assert_eq!(plain_text(source), expected, "{source}");
    }
    assert_eq!(
        parse(r"\def\word#1END{#1}$\word {a{b}}END$"),
        parse("$a{b}$")
    );
    assert_eq!(
        parse(r"\def\word#1;{[#1]}$\word {a}{b};$"),
        parse("$[{a}{b}]$")
    );
    assert_eq!(
        parse("\\def\\word#1,#2;{#1:#2}$\\word a% comment\nb,c;$"),
        parse("$a% comment\nb:c$")
    );
}

#[test]
fn macro_delimiters_match_tokens_without_expansion() {
    for (source, expanded) in [
        (r"\def\word pre#1;{[#1]}\word prevalue;", "[value]"),
        (r"\def\word#1END{[#1]}\word aENDx", "[a]x"),
        (r"\def\word#1\stop{[#1]}\word hello\stop", "[hello]"),
        (
            r"\def\word#1\stop{#1}$\word \stopping\stop$",
            r"$\stopping$",
        ),
        (
            r"\def\word\start x#1\stop{[#1]}\word\start xvalue\stop",
            "[value]",
        ),
        (
            "\\def\\word#1 #2;{#2 #1}\\word first\nsecond;",
            "second first",
        ),
        (r"\def\word#1#{[#1]}\word hello{world}", "[hello]{world}"),
        (
            r"\def\word#1#{[#1]}$\word hello{world}$",
            "$[hello]{world}$",
        ),
    ] {
        assert_eq!(parse(source), parse(expanded), "{source}");
    }
}

#[test]
fn delimiter_matching_uses_the_expansion_work_allowance() {
    let source = format!(
        r"\def\word#1{}b{{expanded}}\word {}b kept",
        "a".repeat(500),
        "a".repeat(3000)
    );
    let text = plain_text(&source);
    assert!(!text.contains("expanded"));
    assert!(text.ends_with("kept"));
}

#[test]
fn expand_after_expands_the_second_token_once_before_the_first() {
    for (source, expanded) in [
        (
            r"\def\first#1{[#1]}\def\second{abc}\expandafter\first\second",
            "[a]bc",
        ),
        (r"\def\word{x}\expandafter\word\word", "xx"),
        (
            r"\newcommand{\word}{Hello}\expandafter\textbf\expandafter{\word}",
            r"\textbf{Hello}",
        ),
        (
            r"\def\first#1{[#1]}\def\second{\third}\def\third{abc}\expandafter\first\second",
            "[abc]",
        ),
        (
            r"\def\first#1{[#1]}\def\second{\third}\def\third{abc}\expandafter\expandafter\expandafter\first\second",
            "[a]bc",
        ),
        (
            r"\def\word{ab}\expandafter\let\expandafter\copy\word\copy",
            "ba",
        ),
        (r"\def\word{x}\expandafter\%\word", r"\%x"),
        (r"\def\word{abc}\expandafter{\word}", "{abc}"),
        (
            r"\let\later\expandafter\def\expandafter{broken}\def\first#1{[#1]}\def\second{abc}\later\first\second",
            "[a]bc",
        ),
        (
            r"\def\first#1{[#1]}\def\second{abc}$\expandafter\first\second$",
            "$[a]bc$",
        ),
        (r"$\expandafter\alpha\beta$", r"$\alpha\beta$"),
    ] {
        assert_eq!(parse(source), parse(expanded), "{source}");
    }
}

#[test]
fn expand_after_keeps_long_chains_and_recursion_bounded() {
    let source = format!("{} kept", r"\expandafter\relax".repeat(3000));
    assert_eq!(plain_text(&source), "kept");
    assert!(parse(r"\def\word{\expandafter\word\word}\word").is_empty());
}

#[test]
fn macro_optional_argument_defaults_when_absent() {
    assert_eq!(
        plain_text("\\newcommand{\\x}[2][d]{#1#2}\n\n\\x{B}\n"),
        "dB"
    );
    assert_eq!(
        plain_text("\\newcommand{\\x}[2][d]{#1#2}\n\n\\x[A]{B}\n"),
        "AB",
    );
}

#[test]
fn macro_body_invoking_another_macro_expands_fully() {
    assert_eq!(
        plain_text("\\newcommand{\\a}{\\b}\n\\newcommand{\\b}{Z}\n\n\\a\n"),
        "Z",
    );
}

#[test]
fn nested_invocations_do_not_accumulate_depth() {
    let mut source = String::from("\\newcommand{\\a}{\\b}\n\\newcommand{\\b}{Z}\n\n");
    for _ in 0..300 {
        source.push_str("\\a ");
    }
    assert_eq!(plain_text(&source).matches('Z').count(), 300);
}

// More than 200 sequential invocations all expand: expansion is not capped by a total count.
#[test]
fn many_sequential_invocations_all_expand() {
    let mut source = String::from("\\newcommand{\\hi}{Hello}\n\n");
    for _ in 0..300 {
        source.push_str("\\hi ");
    }
    assert_eq!(plain_text(&source).matches("Hello").count(), 300);
}

#[test]
fn self_recursive_macro_terminates() {
    let _ = parse("\\newcommand{\\x}{\\x}\n\n\\x\n");
}

#[test]
fn expansion_completed_by_following_source_matches_flattened() {
    assert_eq!(
        parse("\\newcommand{\\bo}{\\textbf}\n\n\\bo{word}\n"),
        parse("\\textbf{word}\n"),
    );
}

#[test]
fn expansion_ending_at_environment_boundary_matches_flattened() {
    assert_eq!(
        parse("\\newcommand{\\c}{content}\n\n\\begin{quote}\\c\\end{quote}\n"),
        parse("\\begin{quote}content\\end{quote}\n"),
    );
}

#[test]
fn adversarially_nested_input_does_not_overflow_the_stack() {
    for opener in [
        "{",
        "\\emph{",
        "\\textbf{",
        "\\footnote{",
        "``",
        "[",
        "\\begin{quote}",
        "\\begin{itemize}\\item ",
    ] {
        let _ = parse(&opener.repeat(10_000));
    }
    let balanced = format!("{}x{}", "{".repeat(10_000), "}".repeat(10_000));
    let _ = parse(&balanced);
}
