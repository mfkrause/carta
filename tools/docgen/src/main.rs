//! Regenerates carta's committed documentation artifacts from `docs/status.toml`.
//!
//! `--write` refreshes them on disk; `--check` regenerates in memory and fails on the first file
//! that has drifted. Both modes run the same rendering code, so a passing `--check` means the
//! committed artifacts are exactly what `--write` would produce.

mod bench;
mod cli;
mod model;
mod render;
mod site;

use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Context lines shown either side of the first drifted line in `--check` output.
const DIFF_CONTEXT: usize = 3;

const USAGE: &str = "usage: docgen (--write | --check)";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Write,
    Check,
}

struct Artifact {
    path: PathBuf,
    contents: String,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("docgen: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode, Box<dyn Error>> {
    let mode = match std::env::args().nth(1).as_deref() {
        Some("--write") => Mode::Write,
        Some("--check") => Mode::Check,
        _ => {
            eprintln!("{USAGE}");
            return Ok(ExitCode::FAILURE);
        }
    };

    let root = repo_root();
    let artifacts = build(&root)?;

    match mode {
        Mode::Write => {
            for artifact in &artifacts {
                if let Some(parent) = artifact.path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&artifact.path, &artifact.contents)?;
                println!("wrote {}", display(&root, &artifact.path));
            }
            Ok(ExitCode::SUCCESS)
        }
        Mode::Check => {
            for artifact in &artifacts {
                let on_disk = fs::read_to_string(&artifact.path).unwrap_or_default();
                if on_disk != artifact.contents {
                    let relative = display(&root, &artifact.path);
                    eprintln!("docgen: {relative} is out of date");
                    eprint!("{}", diff(&relative, &on_disk, &artifact.contents));
                    eprintln!(
                        "\nrun: cargo run --manifest-path tools/docgen/Cargo.toml -- --write"
                    );
                    return Ok(ExitCode::FAILURE);
                }
            }
            println!("docgen: {} artifacts up to date", artifacts.len());
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn build(root: &Path) -> Result<Vec<Artifact>, Box<dyn Error>> {
    let source = fs::read_to_string(root.join("docs/status.toml"))?;
    let status: model::Status = toml::from_str(&source)?;
    validate(&status)?;

    let measurements = fs::read_to_string(root.join("docs/benchmarks.toml"))?;
    let benchmarks: bench::Benchmarks = toml::from_str(&measurements)?;

    Ok(vec![
        Artifact {
            path: root.join("docs/STATUS.md"),
            contents: render::status_markdown(&status),
        },
        Artifact {
            path: root.join("docs/BENCHMARKS.md"),
            contents: bench::markdown(&benchmarks),
        },
        Artifact {
            path: root.join("website/src/data/generated/benchmarks.json"),
            contents: bench::json(&benchmarks)?,
        },
        Artifact {
            path: root.join("website/src/data/generated/formats.json"),
            contents: site::formats_json(&status)?,
        },
        Artifact {
            path: root.join("website/src/data/generated/extensions.json"),
            contents: site::extensions_json(&status)?,
        },
        Artifact {
            path: root.join("website/src/content/docs/cli/reference.md"),
            contents: cli::reference(root)?,
        },
    ])
}

/// Rejects status data the renderers would silently drop or duplicate.
fn validate(status: &model::Status) -> Result<(), Box<dyn Error>> {
    let mut seen: Vec<&str> = Vec::new();
    for format in &status.formats {
        if model::family_rank(&format.family) == model::FAMILIES.len() {
            return Err(format!(
                "format \"{}\" declares unknown family \"{}\"; add it to FAMILIES in tools/docgen/src/model.rs",
                format.name, format.family
            )
            .into());
        }
        for name in
            std::iter::once(format.name.as_str()).chain(format.aliases.iter().map(String::as_str))
        {
            if seen.contains(&name) {
                return Err(format!("format name \"{name}\" is declared twice").into());
            }
            seen.push(name);
        }
    }
    Ok(())
}

/// A unified-diff-style hunk around the first line that differs.
fn diff(path: &str, on_disk: &str, generated: &str) -> String {
    let old: Vec<&str> = on_disk.lines().collect();
    let new: Vec<&str> = generated.lines().collect();
    let first = (0..old.len().max(new.len()))
        .find(|index| old.get(*index) != new.get(*index))
        .unwrap_or(0);
    let start = first.saturating_sub(DIFF_CONTEXT);
    let end = (first + DIFF_CONTEXT + 1).min(old.len().max(new.len()));

    let mut hunk = format!(
        "--- {path} (on disk)\n+++ {path} (generated)\n@@ line {} @@\n",
        first + 1
    );
    for index in start..end {
        match (old.get(index), new.get(index)) {
            (a, b) if a == b => {
                if let Some(line) = a {
                    let _ = writeln!(hunk, " {line}");
                }
            }
            (a, b) => {
                if let Some(line) = a {
                    let _ = writeln!(hunk, "-{line}");
                }
                if let Some(line) = b {
                    let _ = writeln!(hunk, "+{line}");
                }
            }
        }
    }
    hunk
}

/// The repo root, resolved from this crate's location so the cwd does not matter.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}
