//! End-to-end tests for the `carta` binary: format dispatch, aliases, file vs stdin/stdout I/O,
//! and the error paths. The binary is invoked as a subprocess (`CARGO_BIN_EXE_carta`); outputs are
//! the writer's own deterministic text, so these run fully offline.

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
    let mut child = Command::new(env!("CARGO_BIN_EXE_carta"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn carta");
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
    let output = child.wait_with_output().expect("wait for carta");
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

fn lines(stdout: &str) -> Vec<&str> {
    stdout.lines().collect()
}

#[test]
fn list_input_formats_needs_no_conversion_flags() {
    let result = run(&["--list-input-formats"], "");
    assert!(result.success, "stderr: {}", result.stderr);
    let formats = lines(&result.stdout);
    // Canonical names and their aliases are both listed, sorted.
    for expected in [
        "commonmark",
        "commonmark_x",
        "gfm",
        "json",
        "markdown",
        "native",
    ] {
        assert!(
            formats.contains(&expected),
            "missing {expected}: {formats:?}"
        );
    }
    let mut sorted = formats.clone();
    sorted.sort_unstable();
    assert_eq!(formats, sorted, "output is not sorted");
}

#[test]
fn list_output_formats_includes_aliases() {
    let result = run(&["--list-output-formats"], "");
    assert!(result.success, "stderr: {}", result.stderr);
    let formats = lines(&result.stdout);
    for expected in ["html", "html4", "html5", "latex", "beamer", "json"] {
        assert!(
            formats.contains(&expected),
            "missing {expected}: {formats:?}"
        );
    }
}

#[test]
fn list_extensions_defaults_to_markdown_dialect() {
    let result = run(&["--list-extensions"], "");
    assert!(result.success, "stderr: {}", result.stderr);
    let extensions = lines(&result.stdout);
    assert!(extensions.contains(&"+smart"), "{extensions:?}");
    assert!(extensions.contains(&"+pipe_tables"), "{extensions:?}");
    assert!(
        extensions.contains(&"-gfm_auto_identifiers"),
        "{extensions:?}"
    );
}

#[test]
fn list_extensions_reflects_the_requested_format() {
    let result = run(&["--list-extensions=commonmark"], "");
    assert!(result.success, "stderr: {}", result.stderr);
    let extensions = lines(&result.stdout);
    // Strict CommonMark enables only raw HTML.
    assert!(extensions.contains(&"+raw_html"), "{extensions:?}");
    assert!(extensions.contains(&"-smart"), "{extensions:?}");
    assert!(extensions.contains(&"-pipe_tables"), "{extensions:?}");
}

#[test]
fn list_extensions_rejects_an_unknown_format() {
    let result = run(&["--list-extensions=bogus"], "");
    assert!(!result.success);
    assert!(
        result.stderr.contains("unsupported format: bogus"),
        "stderr: {}",
        result.stderr
    );
}
