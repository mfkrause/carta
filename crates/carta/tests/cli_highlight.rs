//! End-to-end tests for the `carta` binary's syntax-highlighting command surface: the language and
//! style catalogs, the theme printer, the presentation modes (`default`/`none`/`idiomatic`), the
//! style override, and loading an extra language definition. The binary is invoked as a subprocess;
//! outputs are deterministic, so these run fully offline.

#![cfg(all(feature = "cli", feature = "highlight", feature = "write-html"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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
    if let Err(error) = write_result {
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe, "{error}");
    }
    let output = child.wait_with_output().expect("wait for carta");
    Output {
        success: output.status.success(),
        stdout: String::from_utf8(output.stdout).expect("utf-8 stdout"),
        stderr: String::from_utf8(output.stderr).expect("utf-8 stderr"),
    }
}

fn toy_grammar() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/highlight/toy.xml")
}

const PY_BLOCK: &str = "```python\nx = 1\n```\n";

#[test]
fn list_highlight_styles_names_the_builtins() {
    let result = run(&["--list-highlight-styles"], "");
    assert!(result.success, "stderr: {}", result.stderr);
    let styles: Vec<&str> = result.stdout.lines().collect();
    assert!(styles.contains(&"pygments"), "got: {styles:?}");
    assert!(styles.contains(&"breezedark"), "got: {styles:?}");
    // Each style is a bare name on its own line.
    assert!(
        styles
            .iter()
            .all(|line| !line.contains(char::is_whitespace))
    );
}

#[test]
fn list_highlight_languages_is_sorted_and_populated() {
    let result = run(&["--list-highlight-languages"], "");
    assert!(result.success, "stderr: {}", result.stderr);
    let langs: Vec<&str> = result.stdout.lines().collect();
    assert!(langs.contains(&"python"), "python listed");
    assert!(langs.len() > 100, "a full catalog, got {}", langs.len());
    let mut sorted = langs.clone();
    sorted.sort_unstable();
    assert_eq!(langs, sorted, "languages are emitted in sorted order");
}

#[test]
fn print_highlight_style_emits_a_json_theme() {
    let result = run(&["--print-highlight-style", "pygments"], "");
    assert!(result.success, "stderr: {}", result.stderr);
    let json = result.stdout.trim();
    assert!(
        json.starts_with('{') && json.ends_with('}'),
        "a JSON object"
    );
    assert!(json.contains("\"text-styles\""), "carries token styles");
    assert!(json.contains("\"background-color\""));
    // A print-only invocation converts nothing, so it emits the theme and only the theme.
    assert!(!json.contains("sourceCode"));
}

#[test]
fn print_highlight_style_round_trips_through_a_file() {
    let printed = run(&["--print-highlight-style", "espresso"], "");
    assert!(printed.success, "stderr: {}", printed.stderr);
    let dir = std::env::temp_dir().join("carta_cli_highlight_roundtrip");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("espresso.theme");
    std::fs::write(&path, &printed.stdout).expect("write theme");

    // A theme file supplied to --highlight-style colors code exactly as the built-in name does.
    let from_file = run(
        &[
            "-f",
            "markdown",
            "-t",
            "html",
            "--highlight-style",
            path.to_str().unwrap(),
        ],
        PY_BLOCK,
    );
    let from_name = run(
        &[
            "-f",
            "markdown",
            "-t",
            "html",
            "--highlight-style",
            "espresso",
        ],
        PY_BLOCK,
    );
    assert!(from_file.success, "stderr: {}", from_file.stderr);
    assert_eq!(from_file.stdout, from_name.stdout);
}

#[test]
fn default_mode_colorizes_code() {
    let result = run(&["-f", "markdown", "-t", "html"], PY_BLOCK);
    assert!(result.success, "stderr: {}", result.stderr);
    assert!(result.stdout.contains("class=\"sourceCode python\""));
    assert!(result.stdout.contains("<span class=\"dv\">1</span>"));
}

#[test]
fn no_highlight_leaves_code_plain() {
    let result = run(
        &["-f", "markdown", "-t", "html", "--no-highlight"],
        PY_BLOCK,
    );
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(
        result.stdout,
        "<pre class=\"python\"><code>x = 1</code></pre>\n"
    );
}

#[test]
fn syntax_highlighting_none_matches_no_highlight() {
    let none = run(
        &[
            "-f",
            "markdown",
            "-t",
            "html",
            "--syntax-highlighting",
            "none",
        ],
        PY_BLOCK,
    );
    let no_flag = run(
        &["-f", "markdown", "-t", "html", "--no-highlight"],
        PY_BLOCK,
    );
    assert!(none.success, "stderr: {}", none.stderr);
    assert_eq!(none.stdout, no_flag.stdout);
}

#[test]
fn idiomatic_mode_uses_the_targets_own_listing() {
    // The HTML family has no distinct listing construct, so idiomatic code stays a plain, classed
    // block with no color spans, while LaTeX switches to its listing environment.
    let html = run(
        &[
            "-f",
            "markdown",
            "-t",
            "html",
            "--syntax-highlighting",
            "idiomatic",
        ],
        PY_BLOCK,
    );
    assert!(html.success, "stderr: {}", html.stderr);
    assert!(!html.stdout.contains("class=\"sourceCode"));

    let latex = run(
        &[
            "-f",
            "markdown",
            "-t",
            "latex",
            "--syntax-highlighting",
            "idiomatic",
        ],
        PY_BLOCK,
    );
    assert!(latex.success, "stderr: {}", latex.stderr);
    assert!(latex.stdout.contains("\\begin{lstlisting}"));
}

#[test]
fn syntax_definition_adds_a_language() {
    let path = toy_grammar();
    let source = "```toy\nif 1\n```\n";
    let plain = run(&["-f", "markdown", "-t", "html"], source);
    assert!(
        plain.stdout.contains("<pre class=\"toy\">"),
        "unknown language stays plain: {}",
        plain.stdout
    );

    let defined = run(
        &[
            "-f",
            "markdown",
            "-t",
            "html",
            "--syntax-definition",
            path.to_str().unwrap(),
        ],
        source,
    );
    assert!(defined.success, "stderr: {}", defined.stderr);
    assert!(
        defined.stdout.contains("<span class=\"kw\">if</span>"),
        "keyword tokenized: {}",
        defined.stdout
    );
    assert!(defined.stdout.contains("<span class=\"dv\">1</span>"));
}

#[test]
fn unknown_highlight_style_is_reported() {
    let result = run(
        &[
            "-f",
            "markdown",
            "-t",
            "html",
            "--highlight-style",
            "no-such-style-xyz",
        ],
        PY_BLOCK,
    );
    assert!(!result.success);
    assert!(!result.stderr.is_empty());
}
