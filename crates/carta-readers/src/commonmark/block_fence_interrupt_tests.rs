use super::{IrBlock, parse};
use carta_core::{Extension, Extensions};

fn md(input: &str, exts: &[Extension]) -> Vec<IrBlock> {
    parse(input, Extensions::from_list(exts), true).0
}

#[test]
fn a_tilde_fence_does_not_interrupt_a_paragraph() {
    let out = md("text\n~~~\ncode\n~~~\n", &[Extension::FencedCodeBlocks]);
    assert!(
        matches!(out.as_slice(), [IrBlock::Para(_)]),
        "a tilde fence folds into the open paragraph: {out:?}"
    );
}

#[test]
fn a_tilde_fence_opens_a_block_at_the_top_level() {
    let out = md("~~~\ncode\n~~~\n", &[Extension::FencedCodeBlocks]);
    assert!(
        matches!(out.as_slice(), [IrBlock::CodeBlock(..)]),
        "a top-level tilde fence still opens a code block: {out:?}"
    );
}

#[test]
fn a_backtick_fence_still_interrupts_a_paragraph() {
    let out = md("text\n```\ncode\n```\n", &[Extension::BacktickCodeBlocks]);
    assert!(
        matches!(out.as_slice(), [IrBlock::Para(_), IrBlock::CodeBlock(..)]),
        "a backtick fence interrupts the paragraph: {out:?}"
    );
}

#[test]
fn fenced_attributes_validate_before_decoding_escapes() {
    let extensions = Extensions::from_list(&[
        Extension::BacktickCodeBlocks,
        Extension::FencedCodeAttributes,
        Extension::Attributes,
    ]);
    let source = "```{#a\\_b}\nword\n```\n";
    let markdown = parse(source, extensions, true).0;
    assert!(matches!(markdown.as_slice(), [IrBlock::Para(_)]));
    let commonmark = parse(source, extensions, false).0;
    assert!(
        matches!(commonmark.as_slice(), [IrBlock::CodeBlock(attr, text)]
        if attr.id == "a_b" && text == "word")
    );
    for markdown in [false, true] {
        let source = "```{key=\"a\\\\b\"}\nword\n```\n";
        let out = parse(source, extensions, markdown).0;
        assert!(matches!(out.as_slice(), [IrBlock::CodeBlock(attr, _)]
            if attr.attributes == [("key".into(), "a\\b".into())]));
    }
    for (markdown, language) in [(true, "py\\+x"), (false, "py+x")] {
        let out = parse("```py\\+x\nword\n```\n", extensions, markdown).0;
        assert!(matches!(out.as_slice(), [IrBlock::CodeBlock(attr, _)]
            if attr.classes == [language]));
    }
    for (markdown, classes) in [(true, ["python", "valid"]), (false, ["valid", "python"])] {
        let out = parse("```python {.valid}\nword\n```\n", extensions, markdown).0;
        assert!(matches!(out.as_slice(), [IrBlock::CodeBlock(attr, _)]
            if attr.classes == classes));
    }
    for info in ["python {#a\\b}", "\\{#name\\}", "python extra"] {
        let source = format!("```{info}\nword\n```\n");
        let out = parse(&source, extensions, true).0;
        assert!(matches!(out.as_slice(), [IrBlock::Para(_)]), "{info}");
    }
}

#[test]
fn an_opener_after_a_non_interrupting_tilde_still_fires() {
    let out = md(
        "text\n~~~\n# h\n~~~\nmore\n",
        &[Extension::FencedCodeBlocks],
    );
    assert!(
        matches!(out.get(1), Some(IrBlock::Heading(1, _))),
        "a heading after a non-interrupting tilde fence still opens: {out:?}"
    );
}
