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
