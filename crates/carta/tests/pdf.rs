//! Offline PDF routing and compiler boundary tests.

#![cfg(all(
    feature = "cli",
    feature = "write-pdf",
    feature = "read-commonmark",
    unix
))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output, Stdio};

struct Compiler {
    directory: tempfile::TempDir,
}

impl Compiler {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("carta PDF test ")
            .tempdir()
            .unwrap();
        let executable = directory.path().join("typst");
        fs::write(
            &executable,
            r#"#!/bin/sh
/bin/cat > "$PDF_CAPTURE/source.typ"
printf '%s\n' "$@" > "$PDF_CAPTURE/arguments"
printf '%s' "$SOURCE_DATE_EPOCH" > "$PDF_CAPTURE/timestamp"
for output do :; done
case "$PDF_MODE" in
  fail) printf 'compiler failure' >&2; exit 1 ;;
  invalid) printf 'not PDF' > "$output" ;;
  absent) exit 0 ;;
  *) printf '%%PDF-1.7\nfixture\n' > "$output" ;;
esac
"#,
        )
        .unwrap();
        fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
        Self { directory }
    }

    fn run(&self, arguments: &[&str], input: &str, mode: &str) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_carta"))
            .args(if arguments.contains(&"-f") {
                vec![]
            } else {
                vec!["-f", "markdown"]
            })
            .args(arguments)
            .env("PATH", self.directory.path())
            .env("PDF_CAPTURE", self.directory.path())
            .env("PDF_MODE", mode)
            .env_remove("SOURCE_DATE_EPOCH")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let _ = child.stdin.take().unwrap().write_all(input.as_bytes());
        child.wait_with_output().unwrap()
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.directory.path().join(name)).unwrap()
    }
}

#[test]
fn pdf_uses_standalone_typst_and_preserves_engine_arguments() {
    let compiler = Compiler::new();
    let output = compiler.run(
        &[
            "-t",
            "pdf",
            "--pdf-engine",
            "typst",
            "--pdf-engine-opt=--font-path",
            "--pdf-engine-opt=fonts with spaces",
            "-V",
            "mainfont=My Font",
            "-V",
            "codefont=Code Font",
            "-V",
            "mathfont=Math Font",
            "-V",
            "fontsize=14pt",
        ],
        "# Heading\n\nBody text.",
        "",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.starts_with(b"%PDF-"));
    let source = compiler.read("source.typ");
    assert!(source.contains("#set page("));
    assert!(source.contains("Body text."));
    assert!(source.contains("font: (\"My Font\",)"));
    assert!(source.contains("#show raw: set text(font: (\"Code Font\",))"));
    assert!(source.contains("#show math.equation: set text(font: (\"Math Font\",))"));
    assert!(source.contains("size: 14pt"));
    assert!(
        compiler
            .read("arguments")
            .contains("--font-path\nfonts with spaces\n")
    );
    assert_eq!(compiler.read("timestamp"), "0");
}

#[test]
fn pdf_filename_selects_compilation_and_failures_preserve_existing_output() {
    let compiler = Compiler::new();
    let destination = compiler.directory.path().join("output.pdf");
    for format in [None, Some("pdf"), Some("typst")] {
        let mut arguments = vec!["-o", destination.to_str().unwrap()];
        if let Some(format) = format {
            arguments.extend(["-t", format]);
        }
        let output = compiler.run(&arguments, "Text", "");
        assert!(output.status.success(), "{output:?}");
        assert!(fs::read(&destination).unwrap().starts_with(b"%PDF-"));
    }
    for mode in ["fail", "invalid", "absent"] {
        fs::write(&destination, "existing document").unwrap();
        let output = compiler.run(&["-o", destination.to_str().unwrap()], "Text", mode);
        assert_eq!(output.status.code(), Some(43), "{output:?}");
        assert_eq!(
            fs::read_to_string(&destination).unwrap(),
            "existing document"
        );
    }
}

#[test]
fn pdf_rejects_other_engines_and_incompatible_writers() {
    let compiler = Compiler::new();
    for arguments in [
        vec!["-t", "pdf", "--pdf-engine=latex"],
        vec!["-t", "html", "-o", "out.pdf"],
    ] {
        let output = compiler.run(&arguments, "Text", "");
        assert!(!output.status.success());
        assert!(!compiler.directory.path().join("source.typ").exists());
    }
}

#[test]
fn pdf_reports_missing_compiler() {
    let compiler = Compiler::new();
    fs::remove_file(compiler.directory.path().join("typst")).unwrap();
    let output = compiler.run(&["-t", "pdf"], "Text", "");
    assert_eq!(output.status.code(), Some(47));
    assert!(String::from_utf8_lossy(&output.stderr).contains("was not found"));
}

#[test]
fn pdf_resolves_resource_paths_data_uris_and_metadata_images() {
    let compiler = Compiler::new();
    fs::write(
        compiler.directory.path().join("image with spaces.png"),
        b"image bytes",
    )
    .unwrap();
    let output = compiler.run(&["-t", "pdf", "--resource-path", compiler.directory.path().to_str().unwrap()],
        "---\nabstract: '![](<image with spaces.png>)'\n---\n\n![](<image with spaces.png>)\n\n![](data:image/png;base64,aW1hZ2U=)", "");
    assert!(output.status.success(), "{output:?}");
    let source = compiler.read("source.typ");
    assert_eq!(source.matches("image-0").count(), 2);
    assert_eq!(source.matches("image-1").count(), 1);
    assert!(!source.contains("data:image"));
}

#[test]
fn pdf_honors_custom_and_data_directory_templates() {
    let compiler = Compiler::new();
    let templates = compiler.directory.path().join("templates");
    fs::create_dir(&templates).unwrap();
    let template = templates.join("default.typst");
    fs::write(&template, "Custom header\n$body$").unwrap();
    for arguments in [
        vec![
            "-t",
            "pdf",
            "--data-dir",
            compiler.directory.path().to_str().unwrap(),
        ],
        vec!["-t", "pdf", "--template", template.to_str().unwrap()],
    ] {
        let output = compiler.run(&arguments, "Text", "");
        assert!(output.status.success(), "{output:?}");
        assert!(compiler.read("source.typ").starts_with("Custom header\n"));
    }
}

#[test]
fn text_api_rejects_pdf_before_running_the_engine() {
    let result = carta::convert_text(
        "markdown",
        "pdf",
        "Text",
        &carta::ReaderOptions::default(),
        &carta::WriterOptions::default(),
    );
    assert!(matches!(result, Err(carta::Error::BinaryFormat(format)) if format == "pdf"));
}

#[test]
fn missing_images_use_alt_text_and_warn() {
    let compiler = Compiler::new();
    let output = compiler.run(&["-t", "pdf", "--sandbox"], "Before ![Missing image](no-image.png) and ![Remote image](https://example.com/image.png) after.", "");
    assert!(output.status.success(), "{output:?}");
    assert!(
        compiler
            .read("source.typ")
            .contains("Before Missing image and Remote image after.")
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("using its description"));
}

#[test]
fn pdf_filters_receive_typst_as_the_target() {
    let compiler = Compiler::new();
    let filter = compiler.directory.path().join("filter");
    fs::write(
        &filter,
        "#!/bin/sh\nprintf '%s' \"$1\" > \"$PDF_CAPTURE/filter-format\"\n/bin/cat\n",
    )
    .unwrap();
    fs::set_permissions(&filter, fs::Permissions::from_mode(0o755)).unwrap();
    let output = compiler.run(
        &["-t", "pdf", "--filter", filter.to_str().unwrap()],
        "Text",
        "",
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(compiler.read("filter-format"), "typst");
}

#[test]
fn pdf_preserves_explicit_typst_extension_settings() {
    let compiler = Compiler::new();
    let destination = compiler.directory.path().join("output.pdf");
    let output = compiler.run(
        &[
            "-f",
            "markdown-smart",
            "-t",
            "typst-smart",
            "-o",
            destination.to_str().unwrap(),
        ],
        "Curly “quotes”.",
        "",
    );
    assert!(output.status.success(), "{output:?}");
    assert!(compiler.read("source.typ").contains("Curly “quotes”."));
}
