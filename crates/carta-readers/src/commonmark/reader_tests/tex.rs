use super::*;
use carta_ast::Format;

fn raw(source: &str) -> Block {
    Block::RawBlock(Format("tex".into()), source.into())
}

#[test]
fn macro_definition_blocks_preserve_complete_replacement_text() {
    for source in [
        r"\newcommand{\word}{Hello}",
        r"\newcommand*\word[2][{a]b}]{#1 #2}",
        r"\def\word#1,#2;{#2 #1}",
        r"\newcommand{\word}{a\}b}",
        "\\newcommand{\\word}{\nHello\n\nworld}",
        "\\newcommand{\\word}{a% }\nb}",
    ] {
        assert_eq!(
            read_markdown(source, &[Extension::RawTex]).blocks,
            vec![raw(source)],
            "{source}"
        );
    }
}

#[test]
fn consecutive_definitions_share_a_block_until_a_blank_line() {
    for separator in ["", " ", "\n", " \n  "] {
        let source = format!(r"\newcommand{{\a}}{{a}}{separator}\def\b{{b}}");
        assert_eq!(
            read_markdown(&source, &[Extension::RawTex]).blocks,
            vec![raw(&source)]
        );
    }
    let source = "\\newcommand{\\a}{a}\n\n\\def\\b{b}";
    assert_eq!(
        read_markdown(source, &[Extension::RawTex]).blocks,
        vec![raw(r"\newcommand{\a}{a}"), raw(r"\def\b{b}")]
    );
}

#[test]
fn blank_lines_drop_list_indentation_and_keep_quoted_content_spaces() {
    let definition = "\\newcommand{\\word}{\nHello\n\nworld}";
    assert_eq!(
        read_markdown(
            "- \\newcommand{\\word}{\n  Hello\n    \n  world}",
            &[Extension::RawTex]
        )
        .blocks,
        vec![Block::BulletList(vec![vec![raw(definition)]])]
    );
    let quoted = "\\newcommand{\\word}{\nHello\n  \nworld}";
    assert_eq!(
        read_markdown(
            "- > \\newcommand{\\word}{\n  > Hello\n  >   \n  > world}",
            &[Extension::RawTex]
        )
        .blocks,
        vec![Block::BulletList(vec![vec![Block::BlockQuote(vec![raw(
            quoted
        )])]])]
    );
}

#[test]
fn text_after_a_definition_stays_in_its_container() {
    let definition = r"\newcommand{\word}{Hello}";
    let after = vec![Inline::Str("after".into())];
    assert_eq!(
        read_markdown(&format!("> {definition} after"), &[Extension::RawTex]).blocks,
        vec![Block::BlockQuote(vec![
            raw(definition),
            Block::Para(after.clone())
        ])]
    );
    assert_eq!(
        read_markdown(&format!("- {definition} after"), &[Extension::RawTex]).blocks,
        vec![Block::BulletList(vec![vec![
            raw(definition),
            Block::Plain(after)
        ]])]
    );
    assert_eq!(
        read_markdown(&format!("{definition} # Heading"), &[Extension::RawTex]).blocks,
        vec![
            raw(definition),
            Block::Header(1, Box::default(), vec![Inline::Str("Heading".into())])
        ]
    );
    assert!(matches!(
        read_markdown(
            &format!("Before\n{definition}\nAfter"),
            &[Extension::RawTex]
        )
        .blocks
        .as_slice(),
        [Block::Para(_)]
    ));
}

#[test]
fn a_code_fence_after_a_definition_keeps_its_following_lines() {
    let definition = r"\newcommand{\word}{Hello}";
    let code = Block::CodeBlock(
        Box::new(Attr {
            classes: vec!["rust".into()],
            ..Attr::default()
        }),
        "code".into(),
    );
    let extensions = &[Extension::RawTex, Extension::BacktickCodeBlocks];
    assert_eq!(
        read_markdown(&format!("{definition} ```rust\ncode\n```"), extensions).blocks,
        vec![raw(definition), code.clone()]
    );
    assert_eq!(
        read_markdown(
            &format!("> {definition} ```rust\n> code\n> ```"),
            extensions
        )
        .blocks,
        vec![Block::BlockQuote(vec![raw(definition), code])]
    );
}

#[test]
fn an_unclosed_definition_does_not_change_the_valid_prefix() {
    let first = r"\newcommand{\first}{First}";
    let second = r"\newcommand{\second}{unclosed";
    let standalone = read_markdown(second, &[Extension::RawTex]).blocks;
    let mut expected = vec![raw(first)];
    expected.extend(standalone);
    assert_eq!(
        read_markdown(&format!("{first}\n{second}"), &[Extension::RawTex]).blocks,
        expected
    );
}

fn read_macros(source: &str) -> Document {
    let mut options = ReaderOptions::default();
    options.extensions = carta_core::extensions::presets::MARKDOWN;
    options.greedy_paragraphs = true;
    options.extensions.insert(Extension::LatexMacros);
    CommonmarkReader.read(source, &options).unwrap()
}

fn math_text(document: &mut Document) -> Vec<String> {
    let mut text = Vec::new();
    carta_core::walk::for_each_document_inline(document, &mut |inline| {
        if let Inline::Math(_, source) = inline {
            text.push(source.to_string());
        }
    });
    text
}

#[test]
fn definitions_expand_later_math_and_keep_their_raw_source() {
    let mut document = read_macros(
        "$\\word$\n\n\\newcommand{\\word}{first}\n\n$\\word$\n\n\\renewcommand{\\word}{second}\n\n$\\word$",
    );
    assert_eq!(math_text(&mut document), [r"\word", "first", "second"]);
    assert_eq!(
        document.blocks.get(1),
        Some(&raw(r"\newcommand{\word}{first}"))
    );
    let mut document = read_macros(r"Before \def\word#1{Hi #1} $\word{reader}$ after.");
    assert_eq!(math_text(&mut document), ["Hi reader"]);
}

#[test]
fn named_notes_use_definitions_at_their_source_position() {
    let mut document = read_macros(
        "\\newcommand{\\word}{first}\n\nBefore[^n].\n\n[^n]: $\\word$\n\n\\renewcommand{\\word}{second}\n\nAfter[^n]. $\\word$",
    );
    assert_eq!(math_text(&mut document), ["first", "first", "second"]);
    let mut document = read_macros(
        "\\newcommand{\\word}{outer}\n\n[^n]: \\renewcommand{\\word}{inside}\n\n    $\\word$\n\nText[^n]. $\\word$",
    );
    assert_eq!(math_text(&mut document), ["inside", "outer"]);
}

#[test]
fn inline_notes_and_markdown_containers_keep_definitions_in_order() {
    let mut document = read_macros(
        "Text^[\\newcommand{\\word}{note} $\\word$]. $\\word$\n\n> \\renewcommand{\\word}{quote}\n\n$\\word$",
    );
    assert_eq!(math_text(&mut document), ["note", "note", "quote"]);
}

#[test]
fn heading_references_use_expanded_identifiers() {
    let document = read_macros(
        "\\newcommand{\\word}{Hello}\n\n# $\\word$\n\n[$\\word$]\n\n\\renewcommand{\\word}{Goodbye}\n\n# $\\word$",
    );
    assert_eq!(header_ids(&document.blocks), ["hello", "goodbye"]);
    assert!(matches!(document.blocks.get(2), Some(Block::Para(inlines))
        if matches!(inlines.as_slice(), [Inline::Link(_, _, target)] if target.url == "#hello")));
}

#[test]
fn raw_code_and_verbatim_environments_keep_macro_names() {
    let document = read_macros(
        "\\newcommand{\\word}{Hello}\n\n```{=tex}\n\\word\n```\n\n`\\word`{=tex}\n\n\\begin{verbatim}\\word\\end{verbatim}",
    );
    assert_eq!(document.blocks.get(1), Some(&raw(r"\word")));
    assert!(matches!(document.blocks.get(2), Some(Block::Para(inlines))
        if matches!(inlines.as_slice(), [Inline::RawInline(_, source)] if source == r"\word")));
    assert_eq!(
        document.blocks.get(3),
        Some(&raw(r"\begin{verbatim}\word\end{verbatim}"))
    );
}

#[test]
fn tex_environments_and_groups_keep_their_definitions_local() {
    let mut document = read_macros(
        "\\begin{quote}\n\\newcommand{\\word}{inside}\n\\word\n\\end{quote}\n\n$\\word$\n\n\\textbf{\\newcommand{\\word}{group}\\word} $\\word$",
    );
    assert_eq!(math_text(&mut document), [r"\word", r"\word"]);
    assert_eq!(
        document.blocks.first(),
        Some(&raw(
            "\\begin{quote}\n\\newcommand{\\word}{inside}\ninside\n\\end{quote}"
        ))
    );
    assert!(matches!(document.blocks.get(2), Some(Block::Para(inlines))
        if matches!(inlines.first(), Some(Inline::RawInline(_, source)) if source == r"\textbf{\newcommand{\word}{group}group}")));
}

#[test]
fn tables_and_captions_inherit_macro_definitions() {
    let mut document = read_macros(
        "\\newcommand{\\word}{Hello}\n\n: $\\word$\n\n| Head |\n|---|\n| $\\word$ |\n\n+----------------------+\n| $\\word$              |\n+----------------------+\n",
    );
    assert_eq!(math_text(&mut document), ["Hello", "Hello", "Hello"]);
}

#[test]
fn metadata_definitions_apply_to_later_fields_and_body() {
    let mut document = read_macros(
        "---\nfirst: '\\newcommand{\\word}{Hello} $\\word$'\nsecond: '$\\word$'\n---\n\n$\\word$",
    );
    assert_eq!(math_text(&mut document), ["Hello", "Hello", "Hello"]);
    let mut document = read_macros("% \\newcommand{\\word}{Title} $\\word$\n\n$\\word$");
    assert_eq!(math_text(&mut document), ["Title", "Title"]);
}

#[test]
fn incomplete_macro_arguments_keep_the_invocation() {
    let mut document = read_macros("\\newcommand{\\word}[2]{#1 #2}\n\n$\\word{one}$");
    assert_eq!(math_text(&mut document), [r"\word{one}"]);
}

#[test]
fn standalone_commands_share_a_block_and_aliases_expand() {
    let document = read_macros("\\let\\word=\\textbf\n\\word{Hello}\n\n\\word world");
    assert_eq!(
        document.blocks.first(),
        Some(&raw("\\let\\word=\\textbf\n\\textbf{Hello}"))
    );
    assert!(matches!(document.blocks.get(1), Some(Block::Para(inlines))
        if matches!(inlines.first(), Some(Inline::RawInline(_, source)) if source == "\\textbf ")));
}

#[test]
fn a_later_caption_definition_does_not_change_earlier_cells() {
    let mut document = read_macros(
        "\\newcommand{\\word}{outer}\n\n| $\\word$ |\n|---|\n| $\\word$ |\n\n: \\renewcommand{\\word}{caption} $\\word$\n\n$\\word$",
    );
    assert_eq!(
        math_text(&mut document),
        ["caption", "outer", "outer", "caption"]
    );
}

#[test]
fn nested_tex_environments_and_explicit_groups_restore_definitions() {
    let document = read_macros(
        "\\newcommand{\\word}{outer}\n\n\\begin{quote}\n\\begin{verbatim}\\word\\end{verbatim}\n\\begin{center}\\renewcommand{\\word}{inside}\\word\\end{center}\n\\word\n\\end{quote}",
    );
    assert_eq!(
        document.blocks.get(1),
        Some(&raw(
            "\\begin{quote}\n\\begin{verbatim}\\word\\end{verbatim}\n\\begin{center}\\renewcommand{\\word}{inside}inside\\end{center}\nouter\n\\end{quote}"
        ))
    );
    let document = read_macros(
        "\\newcommand{\\word}{outer}\n\n\\textbf{\\begingroup\\renewcommand{\\word}{inside}\\word\\endgroup\\word}",
    );
    assert!(matches!(document.blocks.get(1), Some(Block::Para(inlines))
        if matches!(inlines.as_slice(), [Inline::RawInline(_, source)]
            if source == r"\textbf{\begingroup\renewcommand{\word}{inside}inside\endgroup outer}")));
}

#[test]
fn providing_a_command_keeps_its_existing_definition() {
    let mut document = read_macros(
        "\\providecommand{\\word}{first}\n\\providecommand{\\word}{second}\n\n$\\word$",
    );
    assert_eq!(math_text(&mut document), ["first"]);
}

#[test]
fn disabling_expansion_keeps_macro_calls_in_math() {
    let mut document = read_markdown(
        "\\newcommand{\\word}{Hello}\n\n$\\word$",
        &[Extension::RawTex, Extension::TexMathDollars],
    );
    assert_eq!(math_text(&mut document), [r"\word"]);
    assert_eq!(
        document.blocks.first(),
        Some(&raw(r"\newcommand{\word}{Hello}"))
    );
}

#[test]
fn link_arguments_expand_macros_and_keep_percent_bytes() {
    let document = read_macros(
        "\\newcommand{\\word}[1]{#1}\n\n\\url{https://host/%20\\word{a%b}}\n\n\\href{https://host/\\word{a%b}}{\\word{label}}\n\n\\nolinkurl{https://host/\\word{literal}}",
    );
    for (index, expected) in [
        (1, r"\url{https://host/%20a%b}"),
        (2, r"\href{https://host/a%b}{label}"),
        (3, r"\nolinkurl{https://host/\word{literal}}"),
    ] {
        assert!(
            matches!(document.blocks.get(index), Some(Block::Para(inlines))
            if matches!(inlines.as_slice(), [Inline::RawInline(_, source)] if source == expected)),
            "{expected}"
        );
    }
}

#[test]
fn embedded_code_environments_follow_the_literate_extension() {
    let source = "\\newcommand{\\word}{outer}\n\n\\begin{quote}\\begin{code}\\word\\end{code}\\word\\end{quote}";
    let document = read_macros(source);
    assert_eq!(
        document.blocks.get(1),
        Some(&raw(
            r"\begin{quote}\begin{code}outer\end{code}outer\end{quote}"
        ))
    );
    let mut options = ReaderOptions::default();
    options.extensions = carta_core::extensions::presets::MARKDOWN;
    options.extensions.insert(Extension::LatexMacros);
    options.extensions.insert(Extension::LiterateHaskell);
    options.greedy_paragraphs = true;
    let document = CommonmarkReader.read(source, &options).unwrap();
    assert_eq!(
        document.blocks.get(1),
        Some(&raw(
            r"\begin{quote}\begin{code}\word\end{code}outer\end{quote}"
        ))
    );
}
