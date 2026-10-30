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
