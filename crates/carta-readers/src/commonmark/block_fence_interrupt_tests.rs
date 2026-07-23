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
fn an_opener_after_a_non_interrupting_tilde_still_fires() {
    // The tilde line folds in, but the following heading is read normally rather than absorbed.
    let out = md(
        "text\n~~~\n# h\n~~~\nmore\n",
        &[Extension::FencedCodeBlocks],
    );
    assert!(
        matches!(out.get(1), Some(IrBlock::Heading(1, _))),
        "a heading after a non-interrupting tilde fence still opens: {out:?}"
    );
}
