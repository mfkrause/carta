//! Offline sandbox tests: a document reaches no file and no URL under `--sandbox`.

#![cfg(all(feature = "cli", feature = "full"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::Write as _;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const CANARY: &str = "CANARY-7f3a91c4";

/// A document in `format` that pulls a canary file into its output when written as `target`.
struct Case {
    format: &'static str,
    target: &'static [&'static str],
    source: &'static str,
}

const CASES: &[Case] = &[
    Case {
        format: "rst",
        target: &["-t", "plain"],
        source: ".. include:: canary.txt\n",
    },
    Case {
        format: "rst",
        target: &["-t", "plain"],
        source: ".. include:: canary.txt\n   :literal:\n",
    },
    Case {
        format: "rst",
        target: &["-t", "plain"],
        source: ".. csv-table::\n   :file: canary.txt\n",
    },
    Case {
        format: "rst",
        target: &["-t", "html"],
        source: ".. raw:: html\n   :file: canary.txt\n",
    },
    Case {
        format: "asciidoc",
        target: &["-t", "plain"],
        source: "include::canary.txt[]\n",
    },
    Case {
        format: "asciidoc",
        target: &["-t", "plain"],
        source: "----\ninclude::canary.txt[]\n----\n",
    },
    Case {
        format: "typst",
        target: &["-t", "plain"],
        source: "#include \"canary.txt\"\n",
    },
    Case {
        format: "typst",
        target: &["-t", "plain"],
        source: "#read(\"canary.txt\")\n",
    },
    Case {
        format: "man",
        target: &["-t", "plain"],
        source: ".so canary.txt\n",
    },
    Case {
        format: "markdown",
        target: &["-t", "html", "--embed-resources"],
        source: "![image](canary.txt)\n",
    },
    Case {
        format: "markdown",
        target: &["-t", "html", "--embed-resources"],
        source: "<script src=\"canary.txt\"></script>\n",
    },
    Case {
        format: "markdown",
        target: &["-t", "rtf"],
        source: "![image](canary.png)\n",
    },
];

fn directory(name: &str) -> PathBuf {
    let directory = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    directory
}

fn convert(directory: &Path, arguments: &[&str], source: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_carta"))
        .args(arguments)
        .current_dir(directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // A failed conversion can exit before it reads all of its input.
    let _ = child.stdin.take().unwrap().write_all(source.as_bytes());
    String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
}

/// Whether `output` carries a canary file as text, as base64, or as hexadecimal bytes.
fn leaks(output: &str, image: &[u8]) -> bool {
    let hexadecimal = image.iter().fold(String::new(), |mut hexadecimal, byte| {
        let _ = write!(hexadecimal, "{byte:02x}");
        hexadecimal
    });
    output.contains(CANARY)
        || output.contains(&carta::media::base64_encode(CANARY.as_bytes()))
        || output.contains(&hexadecimal)
}

#[test]
fn a_document_reaches_no_file_in_the_sandbox() {
    let directory = directory("sandbox-files");
    fs::write(directory.join("canary.txt"), CANARY).unwrap();
    let image = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/standalone/rectangle.png"),
    )
    .unwrap();
    fs::write(directory.join("canary.png"), &image).unwrap();
    for case in CASES {
        let mut arguments = vec!["-f", case.format];
        arguments.extend(case.target);
        let label = format!("{} {:?}", case.format, case.source);
        assert!(
            leaks(&convert(&directory, &arguments, case.source), &image),
            "the case reads no file, so it proves nothing: {label}"
        );
        arguments.push("--sandbox");
        assert!(
            !leaks(&convert(&directory, &arguments, case.source), &image),
            "the sandbox let a file through: {label}"
        );
    }
}

/// The cover and stylesheet entries of the EPUB that `arguments` produce; `None` when the
/// conversion fails.
fn epub_assets(directory: &Path, arguments: &[&str], source: &str) -> Option<Vec<String>> {
    let output = directory.join("book.epub");
    let _ = fs::remove_file(&output);
    let mut arguments = arguments.to_vec();
    arguments.extend(["-f", "markdown", "-o", "book.epub"]);
    convert(directory, &arguments, source);
    let entries = carta_core::container::zip::read(&fs::read(output).ok()?).unwrap();
    let mut assets = Vec::new();
    for entry in entries {
        if entry.name.ends_with("cover.xhtml") {
            assets.push("cover".to_owned());
        } else if entry.name.contains("/styles/") {
            assets.push(String::from_utf8(entry.data).unwrap());
        }
    }
    Some(assets)
}

#[test]
fn the_sandbox_reads_what_the_command_line_names_but_not_what_metadata_names() {
    let directory = directory("sandbox-epub");
    let image = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/standalone/rectangle.png");
    fs::copy(image, directory.join("cover.png")).unwrap();
    fs::write(directory.join("style.css"), "p{}").unwrap();
    let named = "---\ntitle: T\ncover-image: cover.png\ncss: style.css\n---\n\nText.\n";
    let plain = "---\ntitle: T\n---\n\nText.\n";
    let flags = ["--epub-cover-image", "cover.png", "--css", "style.css"];
    let packaged = Some(vec!["p{}".to_owned(), "cover".to_owned()]);

    assert_eq!(epub_assets(&directory, &[], named), packaged);
    assert_eq!(epub_assets(&directory, &["--sandbox"], named), None);
    assert_eq!(epub_assets(&directory, &flags, plain), packaged);
    let sandboxed_flags = [&["--sandbox"], &flags[..]].concat();
    assert_eq!(epub_assets(&directory, &sandboxed_flags, plain), packaged);
}

#[test]
fn a_document_reaches_no_url_in_the_sandbox() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/image.png", listener.local_addr().unwrap());
    let directory = directory("sandbox-urls");
    let output = convert(
        &directory,
        &[
            "-f",
            "markdown",
            "-t",
            "html",
            "--embed-resources",
            "--sandbox",
        ],
        &format!("![image]({url})\n"),
    );
    assert!(output.contains(&url), "{output}");
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock,
        "the sandbox opened a connection"
    );
}
