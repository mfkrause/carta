//! End-to-end tests for the `carta` binary: format dispatch, aliases, file vs stdin/stdout I/O,
//! and the error paths. The binary is invoked as a subprocess (`CARGO_BIN_EXE_carta`); outputs are
//! the writer's own deterministic text, so these run fully offline.
//!
//! Gated on `cli`: without that feature the binary is not built, so `CARGO_BIN_EXE_carta` is unset.

#![cfg(feature = "cli")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

struct Output {
    success: bool,
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run(args: &[&str], stdin: &str) -> Output {
    run_bytes(args, stdin.as_bytes())
}

fn run_bytes(args: &[&str], stdin: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_carta"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn carta");
    let write_result = child.stdin.take().expect("child stdin").write_all(stdin);
    // a rejected invocation can exit before reading stdin; a broken-pipe write is expected then
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
        code: output.status.code(),
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

#[cfg(all(feature = "read-commonmark", feature = "write-html"))]
#[test]
fn mathml_flag_renders_structured_math() {
    let result = run(
        &["-f", "markdown", "-t", "html", "--mathml", "--wrap=none"],
        "$x^2$\n\n$$\\frac{a}{b}$$\n",
    );
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(
        result.stdout,
        concat!(
            "<p><math display=\"inline\" xmlns=\"http://www.w3.org/1998/Math/MathML\">",
            "<semantics><msup><mi>x</mi><mn>2</mn></msup>",
            "<annotation encoding=\"application/x-tex\">x^2</annotation></semantics></math></p>\n",
            "<p><math display=\"block\" xmlns=\"http://www.w3.org/1998/Math/MathML\">",
            "<semantics><mfrac><mi>a</mi><mi>b</mi></mfrac>",
            "<annotation encoding=\"application/x-tex\">\\frac{a}{b}</annotation></semantics></math></p>\n",
        ),
    );
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

#[cfg(all(feature = "read-commonmark", feature = "write-html"))]
#[test]
fn format_specs_are_case_insensitive() {
    let result = run(&["-f", "MarkDown+SMART", "-t", "HTML5"], "\"q\"\n");
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(result.stdout, "<p>\u{201c}q\u{201d}</p>\n");
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
    let result = run(&["-f", "notaformat", "-t", "html"], "x");
    assert!(!result.success);
    // generic failure, distinct from the dedicated unsupported-extension code 23
    assert_eq!(result.code, Some(1));
    assert!(
        result.stderr.contains("unsupported format: notaformat"),
        "stderr: {}",
        result.stderr
    );
}

#[cfg(feature = "write-dokuwiki")]
#[test]
fn unsupported_extension_exits_23() {
    let result = run(&["-f", "commonmark", "-t", "dokuwiki+bogus"], "# H\n");
    assert!(!result.success);
    assert_eq!(result.code, Some(23), "stderr: {}", result.stderr);
    assert!(
        result.stderr.contains("bogus") && result.stderr.contains("dokuwiki"),
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
fn omitted_from_flag_defaults_to_markdown() {
    let result = run(&["-t", "html"], "\"x\"");
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(result.stdout, "<p>\u{201c}x\u{201d}</p>\n");
}

#[test]
fn omitted_to_flag_defaults_to_html() {
    let result = run(&["-f", "commonmark"], "x");
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(result.stdout, "<p>x</p>\n");
}

fn input_directory(name: &str) -> PathBuf {
    let directory = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    fs::create_dir_all(&directory).expect("create input directory");
    directory
}

#[test]
fn no_format_flags_convert_markdown_to_html() {
    let result = run(&[], "*text*");
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(result.stdout, "<p><em>text</em></p>\n");
}

#[test]
fn filenames_select_readers_case_insensitively() {
    let directory = input_directory("inferred-readers");
    for (extension, source, expected) in [
        ("MD", "\"text\"", "<p>\u{201c}text\u{201d}</p>\n"),
        (
            "htm",
            "<p><strong>text</strong></p>",
            "<p><strong>text</strong></p>\n",
        ),
        ("TEX", "\\emph{text}", "<p><em>text</em></p>\n"),
        ("rst", "**text**", "<p><strong>text</strong></p>\n"),
        ("org", "Some *text*", "<p>Some <strong>text</strong></p>\n"),
        ("typ", "*text*", "<p><strong>text</strong></p>\n"),
        ("json", SAMPLE_JSON, "<p>hi</p>\n"),
        ("native", "[Para [Str \"hi\"]]", "<p>hi</p>\n"),
    ] {
        let path = directory.join(format!("input.{extension}"));
        fs::write(&path, source).unwrap();
        let result = run(&[path.to_str().unwrap()], "");
        assert!(result.success, "{extension}: {}", result.stderr);
        assert_eq!(result.stdout, expected, "{extension}");
        assert!(result.stderr.is_empty(), "{extension}: {}", result.stderr);
    }
}

#[test]
fn filenames_select_writers_case_insensitively() {
    let directory = input_directory("inferred-writers");
    for (extension, expected) in [
        ("TEX", "\\emph{text}\n"),
        ("md", "*text*\n"),
        ("txt", "*text*\n"),
        ("typ", "#emph[text]\n"),
        ("htm", "<p><em>text</em></p>\n"),
        ("unknown", "<p><em>text</em></p>\n"),
    ] {
        let path = directory.join(format!("output.{extension}"));
        let result = run(&["-o", path.to_str().unwrap()], "*text*");
        assert!(result.success, "{extension}: {}", result.stderr);
        assert!(result.stdout.is_empty());
        assert_eq!(fs::read_to_string(path).unwrap(), expected, "{extension}");
    }
}

#[test]
fn explicit_formats_override_filenames() {
    let directory = input_directory("explicit-formats");
    let input = directory.join("input.json");
    let output = directory.join("output.tex");
    fs::write(&input, "*text*").unwrap();
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
    assert_eq!(
        fs::read_to_string(output).unwrap(),
        "<p><em>text</em></p>\n"
    );
    assert!(result.stderr.is_empty());
}

#[test]
fn unknown_input_extension_warns_and_uses_markdown() {
    let directory = input_directory("unknown-input-extension");
    for name in ["input.unknown", "no-extension"] {
        let input = directory.join(name);
        fs::write(&input, "*text*").unwrap();
        let result = run(&[input.to_str().unwrap()], "");
        assert!(result.success, "stderr: {}", result.stderr);
        assert_eq!(result.stdout, "<p><em>text</em></p>\n");
        assert!(
            result.stderr.contains("defaulting to markdown"),
            "{}",
            result.stderr
        );
    }
}

#[test]
fn first_known_input_extension_selects_one_reader_for_all_files() {
    let directory = input_directory("first-known-extension");
    let unknown = directory.join("first.unknown");
    let html = directory.join("second.html");
    let markdown = directory.join("third.md");
    for path in [&unknown, &html, &markdown] {
        fs::write(path, "<p><strong>text</strong></p>").unwrap();
    }
    let result = run(
        &[
            unknown.to_str().unwrap(),
            html.to_str().unwrap(),
            markdown.to_str().unwrap(),
        ],
        "",
    );
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(
        result.stdout,
        "<p><strong>text</strong></p>\n<p><strong>text</strong></p>\n<p><strong>text</strong></p>\n"
    );
    assert!(result.stderr.is_empty());
}

#[test]
fn multiple_text_inputs_have_blank_line_boundaries() {
    let directory = input_directory("text-boundaries");
    let first = directory.join("first.md");
    let second = directory.join("second.md");
    fs::write(&first, "one").unwrap();
    fs::write(&second, "two").unwrap();
    let result = run(&[first.to_str().unwrap(), second.to_str().unwrap()], "");
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(result.stdout, "<p>one</p>\n<p>two</p>\n");
}

#[test]
fn reference_definitions_resolve_across_input_files() {
    let directory = input_directory("shared-reference-definitions");
    let first = directory.join("first.md");
    let second = directory.join("second.md");
    fs::write(&first, "[text][target]").unwrap();
    fs::write(&second, "[target]: https://example.com").unwrap();
    let result = run(&[first.to_str().unwrap(), second.to_str().unwrap()], "");
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(
        result.stdout,
        "<p><a href=\"https://example.com\">text</a></p>\n"
    );
}

#[test]
fn text_constructs_can_span_input_files() {
    let directory = input_directory("shared-code-fence");
    let first = directory.join("first.md");
    let second = directory.join("second.md");
    fs::write(&first, "```\none").unwrap();
    fs::write(&second, "two\n```").unwrap();
    let result = run(&[first.to_str().unwrap(), second.to_str().unwrap()], "");
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(result.stdout, "<pre><code>one\n\ntwo</code></pre>\n");
}

#[test]
fn dash_reads_stdin_at_the_given_position_and_writes_stdout() {
    let directory = input_directory("stdin-position");
    let input = directory.join("input.md");
    fs::write(&input, "file").unwrap();
    for (paths, expected) in [
        (
            ["-", input.to_str().unwrap()],
            "<p>stdin</p>\n<p>file</p>\n",
        ),
        (
            [input.to_str().unwrap(), "-"],
            "<p>file</p>\n<p>stdin</p>\n",
        ),
    ] {
        let result = run(&[paths[0], paths[1], "-o", "-"], "stdin");
        assert!(result.success, "stderr: {}", result.stderr);
        assert_eq!(result.stdout, expected);
    }
}

#[test]
fn multiple_inputs_decode_each_file_separately() {
    let directory = input_directory("input-encodings");
    let first = directory.join("first.md");
    let second = directory.join("second.md");
    fs::write(&first, "\u{e9}").unwrap();
    fs::write(&second, [0xff]).unwrap();
    let result = run(&[first.to_str().unwrap(), second.to_str().unwrap()], "");
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(result.stdout, "<p>\u{e9}</p>\n<p>\u{ff}</p>\n");
    assert!(result.stderr.contains(second.to_str().unwrap()));
    assert!(!result.stderr.contains(first.to_str().unwrap()));
}

#[test]
fn multiple_json_inputs_merge_blocks_and_metadata() {
    let directory = input_directory("json-inputs");
    let first = directory.join("first.json");
    let second = directory.join("second.json");
    for (path, title, body) in [(&first, "first", "one"), (&second, "second", "two")] {
        let mut document: serde_json::Value = serde_json::from_str(SAMPLE_JSON).unwrap();
        *document.pointer_mut("/meta").unwrap() =
            serde_json::json!({"title": {"t": "MetaString", "c": title}});
        *document.pointer_mut("/blocks/0/c/0/c").unwrap() = body.into();
        fs::write(path, document.to_string()).unwrap();
    }
    let result = run(
        &[
            "-t",
            "json",
            first.to_str().unwrap(),
            second.to_str().unwrap(),
        ],
        "",
    );
    assert!(result.success, "stderr: {}", result.stderr);
    let document: serde_json::Value = serde_json::from_str(&result.stdout).unwrap();
    assert_eq!(document.pointer("/meta/title/c").unwrap(), "second");
    assert_eq!(document.pointer("/blocks/0/c/0/c").unwrap(), "one");
    assert_eq!(document.pointer("/blocks/1/c/0/c").unwrap(), "two");
}

#[test]
fn input_errors_leave_the_output_file_unchanged() {
    let directory = input_directory("input-errors");
    let input = directory.join("present.md");
    let missing = directory.join("missing.md");
    let output = directory.join("output.html");
    fs::write(&input, "text").unwrap();
    fs::write(&output, "keep").unwrap();
    let result = run(
        &[
            input.to_str().unwrap(),
            missing.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
        ],
        "",
    );
    assert!(!result.success);
    assert!(
        result.stderr.contains(missing.to_str().unwrap()),
        "{}",
        result.stderr
    );
    assert_eq!(fs::read_to_string(output).unwrap(), "keep");
}

#[test]
fn multiple_binary_inputs_fail_before_reading() {
    let result = run(&["first.docx", "second.docx"], "");
    assert!(!result.success);
    assert!(
        result
            .stderr
            .contains("multiple inputs require a text format"),
        "{}",
        result.stderr
    );
}

#[test]
fn repeated_stdin_is_rejected() {
    let result = run(&["-", "-"], "text");
    assert!(!result.success);
    assert!(
        result
            .stderr
            .contains("standard input can only be read once"),
        "{}",
        result.stderr
    );
}

#[test]
fn known_unsupported_output_extension_fails() {
    let directory = input_directory("unsupported-output-extension");
    let output = directory.join("output.pdf");
    let result = run(&["-o", output.to_str().unwrap()], "text");
    assert!(!result.success);
    assert!(
        result.stderr.contains("unsupported format: pdf"),
        "{}",
        result.stderr
    );
    assert!(!output.exists());
}

#[cfg(all(feature = "read-typst", feature = "write-html"))]
#[test]
fn typst_stdin_preserves_relative_image_paths() {
    for arguments in [
        &["-f", "typst", "-t", "json"][..],
        &["-f", "typst", "-t", "json", "-"][..],
    ] {
        let result = run(arguments, "#image(\"image.png\")");
        assert!(result.success, "stderr: {}", result.stderr);
        let document: serde_json::Value = serde_json::from_str(&result.stdout).unwrap();
        assert_eq!(
            document.pointer("/blocks/0/c/0/c/2/0").unwrap(),
            "image.png"
        );
    }
}

#[cfg(all(feature = "read-typst", feature = "write-html"))]
#[test]
fn each_typst_input_resolves_its_own_includes_and_images() {
    let directory = input_directory("typst-source-directories");
    let first = directory.join("first");
    let second = directory.join("second");
    for (path, text, image) in [
        (&first, "one", b"first-image".as_slice()),
        (&second, "two", b"second-image".as_slice()),
    ] {
        fs::create_dir_all(path.join("parts")).unwrap();
        fs::write(path.join("parts/part.typ"), text).unwrap();
        fs::write(path.join("image.png"), image).unwrap();
        fs::write(
            path.join("main.typ"),
            "\u{feff}\u{e9}\r\n\r\n#include \"parts/part.typ\"\r\n\r\n#image(\"image.png\")",
        )
        .unwrap();
    }
    let first = first.join("main.typ");
    let second = second.join("main.typ");
    let result = run(
        &[
            "--embed-resources",
            first.to_str().unwrap(),
            second.to_str().unwrap(),
        ],
        "",
    );
    assert!(result.success, "stderr: {}", result.stderr);
    assert!(result.stdout.contains("<p>one</p>"), "{}", result.stdout);
    assert!(result.stdout.contains("<p>two</p>"), "{}", result.stdout);
    assert!(
        result
            .stdout
            .contains("data:image/png;base64,Zmlyc3QtaW1hZ2U="),
        "{}",
        result.stdout
    );
    assert!(
        result
            .stdout
            .contains("data:image/png;base64,c2Vjb25kLWltYWdl"),
        "{}",
        result.stdout
    );
}

#[test]
fn markdown_images_keep_working_directory_paths() {
    let directory = input_directory("markdown-resource-directories");
    let first = directory.join("first.md");
    let second = directory.join("second.md");
    fs::write(&first, "![one](one.png)").unwrap();
    fs::write(&second, "![two](two.png)").unwrap();
    let result = run(
        &[
            "-f",
            "commonmark",
            first.to_str().unwrap(),
            second.to_str().unwrap(),
        ],
        "",
    );
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(
        result.stdout,
        "<p><img src=\"one.png\" alt=\"one\" /></p>\n<p><img src=\"two.png\" alt=\"two\" /></p>\n"
    );
}

#[test]
fn invalid_utf8_input_falls_back_to_latin1() {
    let result = run_bytes(&["-f", "commonmark", "-t", "html"], &[0xff, 0xfe]);
    assert!(result.success);
    assert!(
        result.stdout.contains("\u{ff}\u{fe}"),
        "stdout: {}",
        result.stdout
    );
    assert!(
        result
            .stderr
            .contains("not UTF-8 encoded: falling back to latin1"),
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

#[test]
fn list_extensions_rejects_a_toggle_outside_the_format_set() {
    // rst admits no pipe-table toggle; the failure names both extension and format
    let result = run(&["--list-extensions=rst+pipe_tables"], "");
    assert!(!result.success);
    assert!(
        result.stderr.contains("pipe_tables") && result.stderr.contains("rst"),
        "stderr: {}",
        result.stderr
    );
}

#[test]
fn list_extensions_lists_exactly_the_rst_accepted_set() {
    let result = run(&["--list-extensions=rst"], "");
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(
        lines(&result.stdout),
        vec![
            "-ascii_identifiers",
            "+auto_identifiers",
            "-east_asian_line_breaks",
            "-gfm_auto_identifiers",
            "-literate_haskell",
            "-smart",
        ]
    );
}

#[test]
fn list_extensions_reports_github_reader_defaults() {
    let result = run(&["--list-extensions=markdown_github"], "");
    assert!(result.success, "stderr: {}", result.stderr);
    let extensions = lines(&result.stdout);
    assert!(
        extensions.contains(&"+shortcut_reference_links"),
        "{extensions:?}"
    );
    assert!(
        extensions.contains(&"+space_in_atx_header"),
        "{extensions:?}"
    );
}

#[cfg(all(feature = "read-ipynb", feature = "write-markdown"))]
#[test]
fn extract_media_writes_files_and_rewrites_references() {
    const NOTEBOOK: &str = r#"{"cells":[{"cell_type":"code","execution_count":1,"metadata":{},"outputs":[{"output_type":"display_data","data":{"image/png":"iVBORw0KGgoAAAANSUhEUg=="},"metadata":{}}],"source":["draw()"]}],"metadata":{"kernelspec":{"display_name":"Python 3","language":"python","name":"python3"}},"nbformat":4,"nbformat_minor":5}"#;

    // absolute path: the subprocess resolves it against its own working directory
    let media_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("extract-media");
    let _ = fs::remove_dir_all(&media_dir);

    let bytes = carta::media::base64_decode("iVBORw0KGgoAAAANSUhEUg==").unwrap();
    let name = carta::media::content_addressed_name("image/png", &bytes);

    let extract_arg = format!("--extract-media={}", media_dir.display());
    let result = run(
        &["-f", "ipynb", "-t", "markdown", extract_arg.as_str()],
        NOTEBOOK,
    );
    assert!(result.success, "stderr: {}", result.stderr);

    // the rewritten reference is URL-style (forward slash on every platform), so build it with the
    // writer's join, not `Path::join`
    let extracted = media_dir.join(&name);
    let extracted_ref = carta::media::extracted_path(&media_dir.to_string_lossy(), &name);
    assert!(
        result.stdout.contains(&extracted_ref),
        "stdout missing extracted path {extracted_ref}:\n{}",
        result.stdout
    );
    let written = fs::read(&extracted).expect("extracted media file");
    assert_eq!(written, bytes);
}

#[cfg(feature = "write-html")]
#[test]
fn embed_resources_inlines_local_images_as_data_uris() {
    // `--resource-path` is honored just as for the container writers
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("embed-resources");
    let assets = dir.join("assets");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&assets).expect("create asset dir");
    fs::write(assets.join("logo.png"), b"PNGDATA-abc").expect("write asset");

    let resource_arg = format!("--resource-path={}", assets.display());
    let result = run(
        &[
            "-f",
            "commonmark",
            "-t",
            "html",
            "--embed-resources",
            resource_arg.as_str(),
            "--wrap=none",
        ],
        "![logo](logo.png)\n",
    );
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(
        result.stdout,
        "<p><img role=\"img\" aria-label=\"logo\" \
         src=\"data:image/png;base64,UE5HREFUQS1hYmM=\" alt=\"logo\" /></p>\n"
    );
}

#[cfg(feature = "write-html")]
#[test]
fn sandbox_leaves_remote_references_external() {
    // `--sandbox` never fetches document-controlled URLs (no SSRF surface); the reference stays external
    let result = run(
        &[
            "-f",
            "commonmark",
            "-t",
            "html",
            "--embed-resources",
            "--sandbox",
            "--wrap=none",
        ],
        "![probe](http://169.254.169.254/latest/meta-data/)\n",
    );
    assert!(result.success, "stderr: {}", result.stderr);
    assert!(
        result
            .stdout
            .contains("http://169.254.169.254/latest/meta-data/"),
        "remote URL should be left external under --sandbox:\n{}",
        result.stdout
    );
    assert!(
        !result.stdout.contains("data:"),
        "no resource should be inlined under --sandbox:\n{}",
        result.stdout
    );
}

#[cfg(feature = "write-markdown")]
#[test]
fn embed_resources_is_ignored_for_non_html_output() {
    let result = run(
        &["-f", "commonmark", "-t", "markdown", "--embed-resources"],
        "![logo](logo.png)\n",
    );
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(result.stdout, "![logo](logo.png)\n");
}

#[cfg(all(feature = "write-html", feature = "fetch"))]
#[test]
fn embed_resources_fetches_a_remote_image_over_http() {
    use std::io::Read;
    use std::net::TcpListener;

    // loopback server stands in for a remote host; exercises the fetch path without leaving the machine
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept connection");
        // Drain the request line and headers up to the blank line; the body of a GET is empty.
        let mut request = Vec::new();
        let mut byte = [0u8; 1];
        while stream.read(&mut byte).unwrap_or(0) == 1 {
            request.push(byte[0]);
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let body = b"PNGDATA-xyz";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .expect("write headers");
        stream.write_all(body).expect("write body");
    });

    let url = format!("http://{addr}/logo.png");
    let markdown = format!("![logo]({url})\n");
    let result = run(
        &[
            "-f",
            "commonmark",
            "-t",
            "html",
            "--embed-resources",
            "--wrap=none",
        ],
        &markdown,
    );
    handle.join().expect("server thread");

    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(
        result.stdout,
        "<p><img role=\"img\" aria-label=\"logo\" \
         src=\"data:image/png;base64,UE5HREFUQS14eXo=\" alt=\"logo\" /></p>\n"
    );
}

#[cfg(feature = "write-html")]
#[test]
fn self_contained_implies_standalone_and_warns() {
    let result = run(
        &["-f", "commonmark", "-t", "html", "--self-contained"],
        "# Title\n",
    );
    assert!(result.success, "stderr: {}", result.stderr);
    assert!(
        result.stderr.contains("--self-contained is deprecated"),
        "stderr: {}",
        result.stderr
    );
    assert!(
        result.stdout.contains("<!DOCTYPE html>"),
        "stdout: {}",
        result.stdout
    );
}

const TWO_HEADINGS: &str = "# One\n\n## Two\n";

#[cfg(feature = "write-html")]
#[test]
fn standalone_wraps_in_template() {
    let fragment = run(&["-f", "commonmark", "-t", "html"], TWO_HEADINGS);
    assert!(fragment.success, "stderr: {}", fragment.stderr);
    assert!(!fragment.stdout.contains("<html"), "{}", fragment.stdout);

    let standalone = run(&["-f", "commonmark", "-t", "html", "-s"], TWO_HEADINGS);
    assert!(standalone.success, "stderr: {}", standalone.stderr);
    assert!(
        standalone.stdout.contains("<!DOCTYPE html>")
            && standalone.stdout.contains("<html")
            && standalone.stdout.contains("<head>"),
        "{}",
        standalone.stdout
    );
}

#[cfg(feature = "write-html")]
#[test]
fn toc_is_included_with_flag() {
    let without = run(&["-f", "commonmark", "-t", "html", "-s"], TWO_HEADINGS);
    assert!(without.success, "stderr: {}", without.stderr);
    assert!(
        !without.stdout.contains("<nav id=\"TOC\""),
        "{}",
        without.stdout
    );

    let with = run(
        &["-f", "commonmark", "-t", "html", "-s", "--toc"],
        TWO_HEADINGS,
    );
    assert!(with.success, "stderr: {}", with.stderr);
    assert!(
        with.stdout.contains("<nav id=\"TOC\" role=\"doc-toc\">"),
        "{}",
        with.stdout
    );
    let nav = toc_nav(&with.stdout);
    assert!(nav.contains("One") && nav.contains("Two"), "{nav}");
}

#[cfg(feature = "write-html")]
#[test]
fn toc_depth_limits_listed_levels() {
    let result = run(
        &[
            "-f",
            "commonmark",
            "-t",
            "html",
            "-s",
            "--toc",
            "--toc-depth=1",
        ],
        TWO_HEADINGS,
    );
    assert!(result.success, "stderr: {}", result.stderr);
    let nav = toc_nav(&result.stdout);
    assert!(nav.contains("One"), "{nav}");
    assert!(!nav.contains("Two"), "{nav}");
}

#[cfg(feature = "write-html")]
fn toc_nav(stdout: &str) -> &str {
    let start = stdout.find("<nav id=\"TOC\"").expect("TOC nav present");
    let end = stdout[start..].find("</nav>").expect("TOC nav closed");
    &stdout[start..start + end]
}

#[cfg(feature = "write-html")]
#[test]
fn number_sections_prefixes_headings() {
    let result = run(&["-f", "commonmark", "-t", "html", "-N"], TWO_HEADINGS);
    assert!(result.success, "stderr: {}", result.stderr);
    assert!(
        result.stdout.contains("data-number=\"1\"")
            && result.stdout.contains("data-number=\"1.1\"")
            && result.stdout.contains("class=\"header-section-number\""),
        "{}",
        result.stdout
    );

    let plain = run(&["-f", "commonmark", "-t", "html"], TWO_HEADINGS);
    assert!(plain.success, "stderr: {}", plain.stderr);
    assert!(!plain.stdout.contains("data-number"), "{}", plain.stdout);
}

#[cfg(feature = "write-docbook")]
#[test]
fn top_level_division_names_the_outermost_sections() {
    for (division, outer, inner) in [
        ("section", "section", "section"),
        ("chapter", "chapter", "section"),
        ("part", "part", "chapter"),
    ] {
        let result = run(
            &[
                "-f",
                "commonmark",
                "-t",
                "docbook",
                &format!("--top-level-division={division}"),
            ],
            TWO_HEADINGS,
        );
        assert!(result.success, "stderr: {}", result.stderr);
        assert!(
            result.stdout.starts_with(&format!("<{outer} xmlns="))
                && result.stdout.contains(&format!("<{inner}>"))
                && result.stdout.trim_end().ends_with(&format!("</{outer}>")),
            "{division}: {}",
            result.stdout
        );
    }
}

#[cfg(feature = "write-native")]
#[test]
fn shift_heading_level_by_relevels_the_document() {
    let deeper = run(
        &[
            "-f",
            "commonmark",
            "-t",
            "native",
            "--shift-heading-level-by=1",
        ],
        TWO_HEADINGS,
    );
    assert!(deeper.success, "stderr: {}", deeper.stderr);
    assert!(
        deeper.stdout.contains("Header 2") && deeper.stdout.contains("Header 3"),
        "{}",
        deeper.stdout
    );
}

#[cfg(all(feature = "write-json", feature = "read-commonmark"))]
#[test]
fn shift_heading_level_by_lifts_the_leading_heading_into_the_title() {
    let result = run(
        &[
            "-f",
            "commonmark",
            "-t",
            "json",
            "--shift-heading-level-by=-1",
        ],
        "# One\n\ntext\n\n## Two\n",
    );
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(
        result.stdout.trim_end(),
        concat!(
            r#"{"pandoc-api-version":[1,23,1,2],"#,
            r#""meta":{"title":{"t":"MetaInlines","c":[{"t":"Str","c":"One"}]}},"#,
            r#""blocks":[{"t":"Para","c":[{"t":"Str","c":"text"}]},"#,
            r#"{"t":"Header","c":[1,["",[],[]],[{"t":"Str","c":"Two"}]]}]}"#,
        )
    );
}

#[cfg(feature = "write-markdown")]
#[test]
fn wrap_none_keeps_single_line() {
    let input = "one two three four five six seven eight nine ten eleven twelve\n";
    let result = run(
        &["-f", "commonmark", "-t", "markdown", "--wrap=none"],
        input,
    );
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(result.stdout, input);
}

#[cfg(feature = "write-markdown")]
#[test]
fn wrap_auto_reflows_at_columns() {
    let input = "one two three four five six seven eight nine ten eleven twelve\n";
    let result = run(
        &[
            "-f",
            "commonmark",
            "-t",
            "markdown",
            "--wrap=auto",
            "--columns=20",
        ],
        input,
    );
    assert!(result.success, "stderr: {}", result.stderr);
    let output_lines = lines(&result.stdout);
    assert!(output_lines.len() > 1, "{}", result.stdout);
    assert!(
        output_lines.iter().all(|line| line.len() <= 20),
        "{}",
        result.stdout
    );
    // Only line breaks change: the words survive reflow untouched.
    assert_eq!(
        result.stdout.replace('\n', " ").trim_end(),
        input.trim_end()
    );
}

#[cfg(feature = "write-html")]
#[test]
fn metadata_flag_sets_title() {
    let result = run(
        &["-f", "commonmark", "-t", "html", "-s", "-M", "title:Hello"],
        "body text\n",
    );
    assert!(result.success, "stderr: {}", result.stderr);
    assert!(
        result.stdout.contains("<title>Hello</title>"),
        "{}",
        result.stdout
    );
}

#[cfg(feature = "write-html")]
#[test]
fn variable_flag_is_applied() {
    let result = run(
        &["-f", "commonmark", "-t", "html", "-s", "-V", "lang:fr"],
        "body text\n",
    );
    assert!(result.success, "stderr: {}", result.stderr);
    assert!(result.stdout.contains("lang=\"fr\""), "{}", result.stdout);
}

#[cfg(feature = "write-html")]
#[test]
fn metadata_file_is_read() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let metadata_file = dir.join("metadata.yaml");
    fs::write(&metadata_file, "title: FromFile\n").expect("write metadata file");

    let result = run(
        &[
            "-f",
            "commonmark",
            "-t",
            "html",
            "-s",
            "--metadata-file",
            metadata_file.to_str().unwrap(),
        ],
        "body text\n",
    );
    assert!(result.success, "stderr: {}", result.stderr);
    assert!(
        result.stdout.contains("<title>FromFile</title>"),
        "{}",
        result.stdout
    );
}

#[cfg(feature = "write-html")]
#[test]
fn template_flag_overrides_default() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let template_file = dir.join("marker-template.html");
    fs::write(&template_file, "MARKER-BEFORE $body$ MARKER-AFTER\n").expect("write template file");

    let result = run(
        &[
            "-f",
            "commonmark",
            "-t",
            "html",
            "--template",
            template_file.to_str().unwrap(),
        ],
        "body text\n",
    );
    assert!(result.success, "stderr: {}", result.stderr);
    // A custom template implies standalone: the body is rendered inside it.
    assert_eq!(
        result.stdout,
        "MARKER-BEFORE <p>body text</p> MARKER-AFTER\n"
    );
}

#[cfg(feature = "write-html")]
#[test]
fn print_default_template_emits_template() {
    let result = run(&["-D", "html"], "");
    assert!(result.success, "stderr: {}", result.stderr);
    assert!(
        result.stdout.contains("$body$") && result.stdout.contains("<!DOCTYPE html>"),
        "{}",
        result.stdout
    );
}

#[test]
fn closed_stdout_pipe_exits_cleanly() {
    use std::io::Read;
    let big = "# H\n\nparagraph text here\n\n".repeat(20_000);
    let mut child = Command::new(env!("CARGO_BIN_EXE_carta"))
        .args(["-f", "commonmark", "-t", "html"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn carta");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(big.as_bytes())
        .ok();
    // Read a little, then drop the handle to close the read end of the pipe.
    let mut stdout = child.stdout.take().expect("stdout");
    let mut buf = [0u8; 64];
    let _ = stdout.read(&mut buf);
    drop(stdout);
    let output = child.wait_with_output().expect("wait");
    assert!(
        output.status.success(),
        "expected clean exit on closed pipe"
    );
    assert!(
        output.stderr.is_empty(),
        "stderr not empty: {:?}",
        output.stderr
    );
}
