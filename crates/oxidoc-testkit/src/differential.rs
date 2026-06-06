//! Differential surfaces that diff oxidoc against the pinned pandoc binary.
//!
//! Three surfaces, matching `docs/plans/slice-1-commonmark-html.md`:
//! - [`reader_json`] — `CommonMark` text → JSON AST, compared by `serde_json::Value`.
//! - [`writer_html`] — a native AST → HTML, compared byte-for-byte.
//! - [`e2e_html`] — `CommonMark` text → HTML through the full pipeline, compared byte-for-byte.
//!
//! HTML surfaces invoke pandoc with syntax highlighting and TeX math neutralized
//! (`--syntax-highlighting=none --mathjax`), the same normalization the writer assumes for slice 1.

use std::io::Write;
use std::process::{Command, Stdio};

use oxidoc_core::{Reader, ReaderOptions, Writer, WriterOptions};
use oxidoc_readers::CommonmarkReader;
use oxidoc_writers::HtmlWriter;
use serde_json::Value;

use crate::pandoc_bin;
use crate::roundtrip::json_first_difference;

/// Whether the oracle binary is present. Surfaces require it and should be skipped (or the suite
/// failed) when absent, per the corpus policy.
#[must_use]
pub fn oracle_available() -> bool {
    pandoc_bin().is_file()
}

/// The outcome of one differential comparison.
#[derive(Debug)]
pub enum Diff {
    Match,
    /// oxidoc and the oracle disagree; `detail` describes the first divergence.
    Mismatch {
        detail: String,
    },
    /// oxidoc returned an error or panicked-as-error where the oracle produced output.
    OxidocError {
        detail: String,
    },
    /// The oracle itself rejected the input; not counted against oxidoc.
    OracleRejected {
        stderr: String,
    },
}

impl Diff {
    #[must_use]
    pub fn is_match(&self) -> bool {
        matches!(self, Diff::Match)
    }
}

/// Run the pinned binary, feeding `input` on stdin, returning its stdout on success.
fn run_oracle(args: &[&str], input: &str) -> std::io::Result<Result<Vec<u8>, String>> {
    let mut child = Command::new(pandoc_bin())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    if output.status.success() {
        Ok(Ok(output.stdout))
    } else {
        Ok(Err(String::from_utf8_lossy(&output.stderr).into_owned()))
    }
}

/// Reader surface: `CommonMark` text → JSON AST.
pub fn reader_json(markdown: &str) -> std::io::Result<Diff> {
    let oracle = match run_oracle(&["-f", "commonmark", "-t", "json"], markdown)? {
        Ok(bytes) => bytes,
        Err(stderr) => return Ok(Diff::OracleRejected { stderr }),
    };

    let document = match CommonmarkReader.read(markdown, &ReaderOptions::default()) {
        Ok(document) => document,
        Err(error) => {
            return Ok(Diff::OxidocError {
                detail: error.to_string(),
            });
        }
    };
    let ours = match oxidoc_ast::to_json(&document) {
        Ok(json) => json,
        Err(error) => {
            return Ok(Diff::OxidocError {
                detail: error.to_string(),
            });
        }
    };

    let expected: Value = serde_json::from_slice(&oracle).map_err(std::io::Error::other)?;
    let actual: Value = serde_json::from_str(&ours).map_err(std::io::Error::other)?;
    Ok(match json_first_difference(&expected, &actual) {
        Some(detail) => Diff::Mismatch { detail },
        None => Diff::Match,
    })
}

/// Writer surface: render `input` (in the oracle source format `from`, e.g. `native`, `markdown`,
/// or `html`) to HTML through our writer, compared byte-for-byte. The oracle mints both the JSON our
/// writer consumes and the expected HTML from the same source, so any divergence is the writer's.
pub fn writer_parity(from: &str, input: &str) -> std::io::Result<Diff> {
    let oracle = match run_oracle(
        &[
            "-f",
            from,
            "-t",
            "html",
            "--syntax-highlighting=none",
            "--mathjax",
        ],
        input,
    )? {
        Ok(bytes) => bytes,
        Err(stderr) => return Ok(Diff::OracleRejected { stderr }),
    };
    let json = match run_oracle(&["-f", from, "-t", "json"], input)? {
        Ok(bytes) => bytes,
        Err(stderr) => return Ok(Diff::OracleRejected { stderr }),
    };
    let document = match oxidoc_ast::from_json(&json) {
        Ok(document) => document,
        Err(error) => {
            return Ok(Diff::OxidocError {
                detail: error.to_string(),
            });
        }
    };
    Ok(compare_html(&document, &oracle))
}

/// Writer surface for a native (pandoc) AST → HTML.
pub fn writer_html(native: &str) -> std::io::Result<Diff> {
    writer_parity("native", native)
}

/// End-to-end surface: `CommonMark` text → HTML through reader + writer.
pub fn e2e_html(markdown: &str) -> std::io::Result<Diff> {
    let oracle = match run_oracle(
        &[
            "-f",
            "commonmark",
            "-t",
            "html",
            "--syntax-highlighting=none",
            "--mathjax",
        ],
        markdown,
    )? {
        Ok(bytes) => bytes,
        Err(stderr) => return Ok(Diff::OracleRejected { stderr }),
    };
    let document = match CommonmarkReader.read(markdown, &ReaderOptions::default()) {
        Ok(document) => document,
        Err(error) => {
            return Ok(Diff::OxidocError {
                detail: error.to_string(),
            });
        }
    };
    Ok(compare_html(&document, &oracle))
}

fn compare_html(document: &oxidoc_ast::Document, oracle: &[u8]) -> Diff {
    let ours = match HtmlWriter.write(document, &WriterOptions::default()) {
        Ok(html) => html,
        Err(error) => {
            return Diff::OxidocError {
                detail: error.to_string(),
            };
        }
    };
    // The CLI appends a single trailing newline that the writer omits; the oracle's stdout has one.
    let expected = String::from_utf8_lossy(oracle);
    let expected = expected.strip_suffix('\n').unwrap_or(&expected);
    if ours == expected {
        Diff::Match
    } else {
        Diff::Mismatch {
            detail: first_text_difference(expected, &ours),
        }
    }
}

/// A short description of the first line where two HTML strings diverge.
fn first_text_difference(expected: &str, actual: &str) -> String {
    for (index, (a, b)) in expected.lines().zip(actual.lines()).enumerate() {
        if a != b {
            return format!("line {}: expected {a:?}, got {b:?}", index + 1);
        }
    }
    format!(
        "line count {} vs {}",
        expected.lines().count(),
        actual.lines().count()
    )
}
