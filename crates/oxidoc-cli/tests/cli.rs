//! End-to-end tests for the `oxidoc` binary: format dispatch, aliases, file vs stdin/stdout I/O,
//! and the error paths. The binary is invoked as a subprocess (`CARGO_BIN_EXE_oxidoc`); outputs are
//! the writer's own deterministic text, so no oracle is needed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

struct Output {
    success: bool,
    stdout: String,
    stderr: String,
}

fn run(args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_oxidoc"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn oxidoc");
    let write_result = child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(stdin.as_bytes());
    // A rejected invocation (e.g. an unsupported format) can exit before reading stdin, closing the
    // pipe; a broken-pipe write is then expected and must not fail the test.
    if let Err(error) = write_result {
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::BrokenPipe,
            "write stdin: {error}"
        );
    }
    let output = child.wait_with_output().expect("wait for oxidoc");
    Output {
        success: output.status.success(),
        stdout: String::from_utf8(output.stdout).expect("utf-8 stdout"),
        stderr: String::from_utf8(output.stderr).expect("utf-8 stderr"),
    }
}

const SAMPLE_JSON: &str = r#"{"pandoc-api-version":[1,23,1,2],"meta":{},"blocks":[{"t":"Para","c":[{"t":"Str","c":"hi"}]}]}"#;

#[test]
fn commonmark_to_html_over_stdin() {
    let result = run(&["-f", "commonmark", "-t", "html"], "# Hi\n");
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(result.stdout, "<h1>Hi</h1>\n");
}

#[test]
fn json_round_trips_canonically() {
    let result = run(&["-f", "json", "-t", "json"], SAMPLE_JSON);
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(result.stdout, format!("{SAMPLE_JSON}\n"));
}

#[test]
fn json_to_html() {
    let result = run(&["-f", "json", "-t", "html"], SAMPLE_JSON);
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(result.stdout, "<p>hi</p>\n");
}

#[test]
fn format_aliases_are_accepted() {
    let result = run(&["-f", "markdown", "-t", "html5"], "*x*\n");
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(result.stdout, "<p><em>x</em></p>\n");
}

#[test]
fn reads_input_file_and_writes_output_file() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let input = dir.join("in.md");
    let output = dir.join("out.html");
    fs::write(&input, "# Hi\n").expect("write input file");

    let result = run(
        &[
            "-f",
            "commonmark",
            "-t",
            "html",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
        ],
        "",
    );
    assert!(result.success, "stderr: {}", result.stderr);
    assert!(result.stdout.is_empty(), "stdout: {}", result.stdout);
    assert_eq!(fs::read_to_string(&output).unwrap(), "<h1>Hi</h1>\n");
}

#[test]
fn unsupported_input_format_fails() {
    let result = run(&["-f", "docx", "-t", "html"], "x");
    assert!(!result.success);
    assert!(
        result.stderr.contains("unsupported format: docx"),
        "stderr: {}",
        result.stderr
    );
}

#[test]
fn unsupported_output_format_fails() {
    let result = run(&["-f", "commonmark", "-t", "pdf"], "x");
    assert!(!result.success);
    assert!(
        result.stderr.contains("unsupported format: pdf"),
        "stderr: {}",
        result.stderr
    );
}

#[test]
fn missing_from_flag_fails() {
    let result = run(&["-t", "html"], "x");
    assert!(!result.success);
    assert!(
        result.stderr.contains("--from") && result.stderr.contains("required"),
        "stderr: {}",
        result.stderr
    );
}

#[test]
fn missing_to_flag_fails() {
    let result = run(&["-f", "commonmark"], "x");
    assert!(!result.success);
    assert!(
        result.stderr.contains("--to") && result.stderr.contains("required"),
        "stderr: {}",
        result.stderr
    );
}

#[test]
fn invalid_json_input_fails() {
    let result = run(&["-f", "json", "-t", "html"], "not json");
    assert!(!result.success);
    assert!(
        result.stderr.contains("JSON error"),
        "stderr: {}",
        result.stderr
    );
}
