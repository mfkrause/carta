//! Extract the worked examples from the vendored `CommonMark` specification.
//!
//! Each example in `spec.txt` is delimited by a long backtick fence tagged ` example`, holds the
//! markdown input, a single `.` separator line, then the reference HTML, and a closing fence. The
//! spec uses `→` (U+2192) to stand in for a literal tab. We extract only the markdown *input*: the
//! reference HTML is `CommonMark`'s, whereas oxidoc targets the pinned binary's output, so parity is
//! measured differentially against that binary, not against the spec's HTML.

const EXAMPLE_TAB: char = '\u{2192}';

/// One worked example from the specification.
#[derive(Debug, Clone)]
pub struct SpecExample {
    /// 1-based position in the document, matching the spec's own example numbering.
    pub number: usize,
    /// The markdown input, with tab placeholders restored to real tabs.
    pub markdown: String,
}

/// The vendored specification text, embedded at build time so extraction needs no corpus fetch.
const SPEC: &str = include_str!("../vendor/commonmark/spec.txt");

/// Extract every worked example from the vendored spec, in document order.
#[must_use]
pub fn examples() -> Vec<SpecExample> {
    parse_examples(SPEC)
}

fn parse_examples(spec: &str) -> Vec<SpecExample> {
    let mut out = Vec::new();
    let mut lines = spec.lines();
    let mut number = 0;
    while let Some(line) = lines.next() {
        if !is_example_open(line) {
            continue;
        }
        let mut markdown = String::new();
        for content in lines.by_ref() {
            if content == "." {
                break;
            }
            markdown.push_str(&content.replace(EXAMPLE_TAB, "\t"));
            markdown.push('\n');
        }
        // Consume the reference HTML up to the closing fence; it is not used for parity.
        for content in lines.by_ref() {
            if is_fence(content) {
                break;
            }
        }
        number += 1;
        out.push(SpecExample { number, markdown });
    }
    out
}

fn is_example_open(line: &str) -> bool {
    line.strip_suffix(" example").is_some_and(is_fence)
}

fn is_fence(line: &str) -> bool {
    line.len() >= 3 && line.bytes().all(|byte| byte == b'`')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_examples_in_order() {
        let examples = examples();
        assert!(
            examples.len() > 600,
            "expected the full spec corpus, got {}",
            examples.len()
        );
        assert_eq!(examples.first().map(|e| e.number), Some(1));
        for pair in examples.windows(2) {
            if let [a, b] = pair {
                assert_eq!(b.number, a.number + 1);
            }
        }
    }

    #[test]
    fn restores_tab_placeholders() {
        let first = examples().into_iter().next().expect("at least one example");
        assert!(first.markdown.contains('\t'));
        assert!(!first.markdown.contains(EXAMPLE_TAB));
    }
}
