//! End-to-end tests for JSON filters and the data directory: the `carta` binary is invoked as a
//! subprocess, transforming the document through external programs between reading and writing.
//!
//! Filters are written as `sh` scripts (identity via `cat`, transforms via `sed` on the compact JSON)
//! so the tests need no language interpreter and run fully offline. Unix-only: the scripts rely on a
//! shebang and the executable bit.
//!
//! Gated on `cli`: without that feature the binary is not built, so `CARGO_BIN_EXE_carta` is unset.

#![cfg(all(feature = "cli", unix))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

struct Output {
    code: Option<i32>,
    success: bool,
    stdout: String,
    stderr: String,
}

fn run(args: &[&str], stdin: &str) -> Output {
    run_in_dir(None, args, stdin)
}

/// Like `run`, but with the child's working directory set to `dir`, so working-directory filter
/// resolution can be exercised deterministically.
fn run_in(dir: &Path, args: &[&str], stdin: &str) -> Output {
    run_in_dir(Some(dir), args, stdin)
}

fn run_in_dir(dir: Option<&Path>, args: &[&str], stdin: &str) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_carta"));
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    let mut child = command
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
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::BrokenPipe,
            "write stdin: {error}"
        );
    }
    let output = child.wait_with_output().expect("wait for carta");
    Output {
        code: output.status.code(),
        success: output.status.success(),
        stdout: String::from_utf8(output.stdout).expect("utf-8 stdout"),
        stderr: String::from_utf8(output.stderr).expect("utf-8 stderr"),
    }
}

/// A fresh, empty directory unique to `name`, so tests running in parallel do not collide.
fn work_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create work dir");
    dir
}

/// Write `contents` to `path` and mark it executable.
fn write_exec(path: &Path, contents: &str) {
    fs::write(path, contents).expect("write script");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod script");
}

#[test]
fn an_identity_filter_leaves_the_output_unchanged() {
    let dir = work_dir("filter-identity");
    let filter = dir.join("identity");
    write_exec(&filter, "#!/bin/sh\ncat\n");

    let unfiltered = run(&["-f", "commonmark", "-t", "html"], "# Hi *there*\n");
    let filtered = run(
        &[
            "-f",
            "commonmark",
            "-t",
            "html",
            "-F",
            filter.to_str().unwrap(),
        ],
        "# Hi *there*\n",
    );
    assert!(filtered.success, "stderr: {}", filtered.stderr);
    assert_eq!(filtered.stdout, unfiltered.stdout);
    assert_eq!(filtered.stdout, "<h1>Hi <em>there</em></h1>\n");
}

#[test]
fn a_filter_receives_the_output_format_name() {
    let dir = work_dir("filter-format-arg");
    let filter = dir.join("echo-format");
    // The format name arrives as the first argument; report it on stderr and pass the document on.
    write_exec(&filter, "#!/bin/sh\necho \"format=$1\" >&2\ncat\n");

    let result = run(
        &[
            "-f",
            "commonmark",
            "-t",
            "latex",
            "-F",
            filter.to_str().unwrap(),
        ],
        "hi\n",
    );
    assert!(result.success, "stderr: {}", result.stderr);
    assert!(
        result.stderr.contains("format=latex"),
        "stderr: {}",
        result.stderr
    );
}

#[test]
fn a_filter_transforms_the_document() {
    let dir = work_dir("filter-transform");
    let filter = dir.join("rename");
    write_exec(&filter, "#!/bin/sh\nsed 's/PLACEHOLDER/REPLACED/g'\n");

    let result = run(
        &[
            "-f",
            "commonmark",
            "-t",
            "html",
            "-F",
            filter.to_str().unwrap(),
        ],
        "a PLACEHOLDER word\n",
    );
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(result.stdout, "<p>a REPLACED word</p>\n");
}

#[test]
fn filters_apply_in_order() {
    let dir = work_dir("filter-chain");
    let first = dir.join("first");
    let second = dir.join("second");
    write_exec(&first, "#!/bin/sh\nsed 's/ALPHA/BETA/g'\n");
    write_exec(&second, "#!/bin/sh\nsed 's/BETA/GAMMA/g'\n");

    let result = run(
        &[
            "-f",
            "commonmark",
            "-t",
            "html",
            "-F",
            first.to_str().unwrap(),
            "-F",
            second.to_str().unwrap(),
        ],
        "an ALPHA token\n",
    );
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(result.stdout, "<p>an GAMMA token</p>\n");
}

#[test]
fn a_data_directory_filter_resolves_by_bare_name() {
    let dir = work_dir("filter-datadir");
    let filters = dir.join("filters");
    fs::create_dir_all(&filters).unwrap();
    write_exec(&filters.join("rename"), "#!/bin/sh\nsed 's/MARK/DONE/g'\n");

    let result = run(
        &[
            "--data-dir",
            dir.to_str().unwrap(),
            "-f",
            "commonmark",
            "-t",
            "html",
            "-F",
            "rename",
        ],
        "a MARK here\n",
    );
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(result.stdout, "<p>a DONE here</p>\n");
}

#[test]
fn a_working_directory_filter_takes_precedence_over_the_data_directory() {
    let dir = work_dir("filter-cwd-precedence");
    let data = dir.join("data");
    let filters = data.join("filters");
    fs::create_dir_all(&filters).unwrap();
    // The same bare name lives in both the data directory and the working directory, each tagging the
    // document differently so the winner is unambiguous. The working-directory copy takes precedence.
    write_exec(&filters.join("dup"), "#!/bin/sh\nsed 's/TOKEN/DATADIR/g'\n");
    let cwd = dir.join("cwd");
    fs::create_dir_all(&cwd).unwrap();
    write_exec(&cwd.join("dup"), "#!/bin/sh\nsed 's/TOKEN/WORKDIR/g'\n");

    let result = run_in(
        &cwd,
        &[
            "--data-dir",
            data.to_str().unwrap(),
            "-f",
            "commonmark",
            "-t",
            "html",
            "-F",
            "dup",
        ],
        "a TOKEN here\n",
    );
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(result.stdout, "<p>a WORKDIR here</p>\n");
}

#[test]
fn a_failing_filter_reports_status_83() {
    let dir = work_dir("filter-fail");
    let filter = dir.join("boom");
    write_exec(&filter, "#!/bin/sh\nexit 7\n");

    let result = run(
        &[
            "-f",
            "commonmark",
            "-t",
            "html",
            "-F",
            filter.to_str().unwrap(),
        ],
        "x\n",
    );
    assert!(!result.success);
    assert_eq!(result.code, Some(83), "stderr: {}", result.stderr);
}

#[test]
fn a_filter_emitting_invalid_output_reports_status_83() {
    let dir = work_dir("filter-badjson");
    let filter = dir.join("garbage");
    write_exec(&filter, "#!/bin/sh\nprintf 'not json'\n");

    let result = run(
        &[
            "-f",
            "commonmark",
            "-t",
            "html",
            "-F",
            filter.to_str().unwrap(),
        ],
        "x\n",
    );
    assert!(!result.success);
    assert_eq!(result.code, Some(83), "stderr: {}", result.stderr);
}

#[test]
fn a_missing_filter_reports_status_83() {
    let result = run(
        &[
            "-f",
            "commonmark",
            "-t",
            "html",
            "-F",
            "./no-such-filter-program",
        ],
        "x\n",
    );
    assert!(!result.success);
    assert_eq!(result.code, Some(83), "stderr: {}", result.stderr);
}

#[test]
fn a_filter_sees_merged_command_line_metadata() {
    let dir = work_dir("filter-meta-seen");
    let template = dir.join("marker.html");
    fs::write(&template, "[$marker$]\n").unwrap();
    // The metadata is folded into the document before the filter runs, so the filter observes the
    // `-M` value and can rewrite it.
    let filter = dir.join("rewrite");
    write_exec(&filter, "#!/bin/sh\nsed 's/PRESENT/SEEN/g'\n");

    let result = run(
        &[
            "-f",
            "commonmark",
            "-t",
            "html",
            "-M",
            "marker:PRESENT",
            "--template",
            template.to_str().unwrap(),
            "-F",
            filter.to_str().unwrap(),
        ],
        "body\n",
    );
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(result.stdout, "[SEEN]\n");
}

#[test]
fn a_filters_metadata_edit_survives_rendering() {
    let dir = work_dir("filter-meta-edit");
    let template = dir.join("author.html");
    fs::write(&template, "[$author$]\n").unwrap();
    let filter = dir.join("rewrite");
    write_exec(&filter, "#!/bin/sh\nsed 's/BEFORE/AFTER/g'\n");

    // The `-M` layer is cleared once merged, so rendering does not re-apply the original value on top
    // of the filter's edit: the output reflects the filter, not the command line.
    let result = run(
        &[
            "-f",
            "commonmark",
            "-t",
            "html",
            "-M",
            "author:BEFORE",
            "--template",
            template.to_str().unwrap(),
            "-F",
            filter.to_str().unwrap(),
        ],
        "body\n",
    );
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(result.stdout, "[AFTER]\n");
}

#[test]
fn a_data_directory_default_template_overrides_the_built_in() {
    let dir = work_dir("template-datadir-default");
    let templates = dir.join("templates");
    fs::create_dir_all(&templates).unwrap();
    // `-t html` writes through the html5 writer, whose default template file is `default.html5`.
    fs::write(templates.join("default.html5"), "OVERRIDE|$body$|END\n").unwrap();

    let result = run(
        &[
            "--data-dir",
            dir.to_str().unwrap(),
            "-f",
            "commonmark",
            "-t",
            "html",
            "-s",
        ],
        "hi\n",
    );
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(result.stdout, "OVERRIDE|<p>hi</p>|END\n");
}

#[test]
fn a_named_template_resolves_from_the_data_directory() {
    let dir = work_dir("template-datadir-named");
    let templates = dir.join("templates");
    fs::create_dir_all(&templates).unwrap();
    fs::write(templates.join("fancy.html"), "NAMED<$body$>\n").unwrap();

    let result = run(
        &[
            "--data-dir",
            dir.to_str().unwrap(),
            "-f",
            "commonmark",
            "-t",
            "html",
            "--template",
            "fancy.html",
        ],
        "hi\n",
    );
    assert!(result.success, "stderr: {}", result.stderr);
    assert_eq!(result.stdout, "NAMED<<p>hi</p>>\n");
}
