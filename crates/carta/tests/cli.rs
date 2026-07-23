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
fn invalid_utf8_input_fails() {
    let result = run_bytes(&["-f", "commonmark", "-t", "html"], &[0xff, 0xfe]);
    assert!(!result.success);
    assert!(
        result.stderr.contains("input is not valid UTF-8"),
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
