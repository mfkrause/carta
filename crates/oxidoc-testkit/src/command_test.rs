//! Reuse of pandoc's own command tests (`test/command/*.md`) as differential cases.
//!
//! Command tests are declarative — a pandoc invocation plus input and expected output — and shell
//! out to the `pandoc` executable. We substitute the `oxidoc` binary and diff.

use std::io;
use std::path::PathBuf;

use crate::command_tests_dir;

/// One command test extracted from a `test/command/*.md` file.
#[derive(Debug, Clone)]
pub struct CommandTest {
    /// CLI arguments to pass, with the leading `pandoc` program word removed.
    pub args: Vec<String>,
    pub input: String,
    pub expected: String,
}

/// Recursively collect the `*.md` command-test files in the fetched corpus, sorted for
/// deterministic test ordering.
///
/// Returns an empty vec (not an error) when the corpus has not been fetched, so callers treat
/// "no corpus" as "no cases to run" rather than failing.
pub fn discover_files() -> io::Result<Vec<PathBuf>> {
    crate::collect_files_with_extension(&command_tests_dir(), "md")
}

/// Parse the command tests contained in one corpus file.
///
/// The exact grammar (block delimiters, the `^D` input/output separator) is specified by the
/// corpus's own `test/command/README`; implement it against that file once the corpus is fetched —
/// reading that README is allowed (test docs), reading pandoc source is not.
#[must_use]
pub fn parse(_source: &str) -> Vec<CommandTest> {
    todo!("derive the command-test grammar from the corpus test/command/README, then parse it")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_is_infallible_without_corpus() {
        // Whether or not the corpus is present locally, discovery must not error.
        let files = discover_files().expect("discovery should not error");
        assert!(
            files
                .iter()
                .all(|p| p.extension().is_some_and(|e| e == "md"))
        );
    }
}
