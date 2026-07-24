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

// `#0` is not a parameter reference (only `#1`…`#9` are) and is emitted verbatim
#[test]
fn substitute_macro_preserves_non_parameter_hash_zero() {
    let expanded = substitute_macro("a#0b", &["Y".to_owned()]);
    assert_eq!(expanded, "a#0b");
}

// a numbered display environment keeps its full `\begin`…`\end` source so numbering markup survives
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

// a cross-reference becomes a link tagged with the reference kind; a preceding `~` tie is a
// non-breaking space
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

// `\cref` and `\autoref` request a lowercase label, `\Cref` an uppercase one
#[test]
fn cref_variants_carry_their_reference_kind() {
    let kinds = |input: &str| match parse(input).into_iter().next() {
        Some(Block::Para(inlines)) => inlines
            .into_iter()
            .filter_map(|inline| match inline {
                Inline::Link(attr, _, _) => attr
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

// a `\textwidth`-fraction width becomes a percentage attribute; default alt text is `image`
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

// with expansion off, a definition passes through verbatim as a raw block
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

// nesting depth is released after each invocation, so a long sequence does not hit the cap
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

// A self-recursive macro is stopped by the nesting-depth guard and returns without panicking.
#[test]
fn self_recursive_macro_terminates() {
    let _ = parse("\\newcommand{\\x}{\\x}\n\n\\x\n");
}

// an expansion ending mid-construct reads its argument across the frame boundary, matching
// the flattened source
#[test]
fn expansion_completed_by_following_source_matches_flattened() {
    assert_eq!(
        parse("\\newcommand{\\bo}{\\textbf}\n\n\\bo{word}\n"),
        parse("\\textbf{word}\n"),
    );
}

// a frame emptying right before `\end{...}` pops cleanly at the environment boundary
#[test]
fn expansion_ending_at_environment_boundary_matches_flattened() {
    assert_eq!(
        parse("\\newcommand{\\c}{content}\n\n\\begin{quote}\\c\\end{quote}\n"),
        parse("\\begin{quote}content\\end{quote}\n"),
    );
}
