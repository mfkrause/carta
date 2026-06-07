//! Differential surfaces that diff oxidoc against the pinned pandoc binary.
//!
//! Every surface is parameterized by format name and direction and drives oxidoc through the public
//! facade ([`oxidoc::reader_for`]/[`oxidoc::writer_for`]/[`oxidoc::convert`]), so any compiled-in
//! format is verifiable without bespoke wiring:
//! - [`reader_json`] — `from` text → JSON AST, compared as `serde_json::Value`.
//! - [`writer`] — pandoc mints the AST from `input`; our `to`-writer renders it, compared against
//!   `pandoc -f from -t to`.
//! - [`e2e`] — `from` text → `to` through the full pipeline.
//!
//! Output is compared as a `serde_json::Value` for the JSON target and byte-for-byte (modulo the
//! single trailing newline the CLI adds) for every other target. Per-target oracle normalization —
//! e.g. neutralizing syntax highlighting and TeX math for HTML — lives in [`oracle_normalization`].

use std::io::Write;
use std::process::{Command, Stdio};

use oxidoc::{ReaderOptions, WriterOptions};
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

/// Reader surface: `from` text → JSON AST, compared against `pandoc -f from -t json`.
pub fn reader_json(from: &str, input: &str) -> std::io::Result<Diff> {
    let oracle = match run_oracle(&["-f", from, "-t", "json"], input)? {
        Ok(bytes) => bytes,
        Err(stderr) => return Ok(Diff::OracleRejected { stderr }),
    };

    let reader = match oxidoc::reader_for(from) {
        Ok(reader) => reader,
        Err(error) => {
            return Ok(Diff::OxidocError {
                detail: error.to_string(),
            });
        }
    };
    let document = match reader.read(input, &ReaderOptions::default()) {
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

/// Oracle arguments that neutralize target-specific nondeterminism the writer does not reproduce.
/// HTML output suppresses syntax highlighting and renders TeX math as `MathJax`; other targets need
/// no normalization yet. This is the seam to extend as each new writer is verified.
fn oracle_normalization(to: &str) -> &'static [&'static str] {
    match to {
        "html" | "html5" => &["--syntax-highlighting=none", "--mathjax"],
        _ => &[],
    }
}

/// Writer surface: pandoc mints the AST from `input` (parsed as `from`); our `to`-writer renders it,
/// compared against `pandoc -f from -t to`. Both sides start from the same oracle AST, so any
/// divergence is the writer's.
pub fn writer(to: &str, from: &str, input: &str) -> std::io::Result<Diff> {
    let mut expected_args = vec!["-f", from, "-t", to];
    expected_args.extend_from_slice(oracle_normalization(to));
    let oracle = match run_oracle(&expected_args, input)? {
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
    let writer = match oxidoc::writer_for(to) {
        Ok(writer) => writer,
        Err(error) => {
            return Ok(Diff::OxidocError {
                detail: error.to_string(),
            });
        }
    };
    let ours = match writer.write(&document, &WriterOptions::default()) {
        Ok(output) => output,
        Err(error) => {
            return Ok(Diff::OxidocError {
                detail: error.to_string(),
            });
        }
    };
    Ok(compare_output(to, &ours, &oracle))
}

/// End-to-end surface: `from` text → `to` through the facade, compared against `pandoc -f from -t to`.
pub fn e2e(from: &str, to: &str, input: &str) -> std::io::Result<Diff> {
    let mut expected_args = vec!["-f", from, "-t", to];
    expected_args.extend_from_slice(oracle_normalization(to));
    let oracle = match run_oracle(&expected_args, input)? {
        Ok(bytes) => bytes,
        Err(stderr) => return Ok(Diff::OracleRejected { stderr }),
    };
    let ours = match oxidoc::convert(
        from,
        to,
        input,
        &ReaderOptions::default(),
        &WriterOptions::default(),
    ) {
        Ok(output) => output,
        Err(error) => {
            return Ok(Diff::OxidocError {
                detail: error.to_string(),
            });
        }
    };
    Ok(compare_output(to, &ours, &oracle))
}

/// Compare oxidoc's output against the oracle's: structurally for the JSON target, byte-for-byte
/// (modulo the single trailing newline the CLI adds) otherwise.
fn compare_output(to: &str, ours: &str, oracle: &[u8]) -> Diff {
    if to == "json" {
        let expected: Value = match serde_json::from_slice(oracle) {
            Ok(value) => value,
            Err(error) => {
                return Diff::OxidocError {
                    detail: format!("oracle JSON unparsable: {error}"),
                };
            }
        };
        let actual: Value = match serde_json::from_str(ours) {
            Ok(value) => value,
            Err(error) => {
                return Diff::OxidocError {
                    detail: error.to_string(),
                };
            }
        };
        return match json_first_difference(&expected, &actual) {
            Some(detail) => Diff::Mismatch { detail },
            None => Diff::Match,
        };
    }

    // The CLI appends a single trailing newline that writers omit; the oracle's stdout has one.
    let expected = String::from_utf8_lossy(oracle);
    let expected = expected.strip_suffix('\n').unwrap_or(&expected);
    if ours == expected {
        Diff::Match
    } else {
        Diff::Mismatch {
            detail: first_text_difference(expected, ours),
        }
    }
}

/// A short description of the first line where two text outputs diverge.
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
