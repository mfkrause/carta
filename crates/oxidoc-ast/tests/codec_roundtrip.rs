//! Offline codec identity over the shared AST corpus: decode each `corpus/ast/**` document into the
//! model, re-encode it, and assert the JSON is semantically unchanged. No oracle is involved — the
//! corpus files are inputs we author — so this gates the JSON codec on every run.
//!
//! Equality is by `serde_json::Value`, not bytes: it ignores object-key order and float formatting
//! (different shortest `f64` representations) while still catching any dropped or altered field.

// Integration-test harness code: panicking on a known corpus file is the idiomatic assertion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

fn corpus_files() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/ast");
    let mut files = Vec::new();
    collect_json(&root, &mut files);
    files.sort();
    files
}

fn collect_json(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        fs::read_dir(dir).unwrap_or_else(|error| panic!("read dir {}: {error}", dir.display()));
    for entry in entries {
        let path = entry.expect("read dir entry").path();
        if path.is_dir() {
            collect_json(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "json") {
            out.push(path);
        }
    }
}

/// The JSON pointer of the first place two documents diverge, or `None` if equal. Recurses
/// structurally so a deep mismatch is reported as e.g. `/blocks/0/c/2`.
fn first_difference(before: &Value, after: &Value, path: &mut String) -> Option<String> {
    match (before, after) {
        (Value::Object(a), Value::Object(b)) => {
            for (key, value) in a {
                let Some(other) = b.get(key) else {
                    return Some(format!("{path}/{key} (missing in re-emitted)"));
                };
                let mark = path.len();
                path.push('/');
                path.push_str(key);
                if let Some(found) = first_difference(value, other, path) {
                    return Some(found);
                }
                path.truncate(mark);
            }
            b.keys()
                .find(|key| !a.contains_key(*key))
                .map(|key| format!("{path}/{key} (unexpected in re-emitted)"))
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                return Some(format!("{path} (length {} vs {})", a.len(), b.len()));
            }
            for (index, (value, other)) in a.iter().zip(b).enumerate() {
                let mark = path.len();
                path.push('/');
                path.push_str(&index.to_string());
                if let Some(found) = first_difference(value, other, path) {
                    return Some(found);
                }
                path.truncate(mark);
            }
            None
        }
        _ if before == after => None,
        _ => Some(format!("{path} ({before} vs {after})")),
    }
}

#[test]
fn corpus_round_trips_through_the_codec() {
    let files = corpus_files();
    assert!(!files.is_empty(), "no corpus AST files found");

    let mut failures = Vec::new();
    for path in &files {
        let golden = fs::read(path).expect("read corpus file");
        let document = match oxidoc_ast::from_json(&golden) {
            Ok(document) => document,
            Err(error) => {
                failures.push(format!("{}: decode failed: {error}", path.display()));
                continue;
            }
        };
        let reemitted = oxidoc_ast::to_json(&document).expect("re-encode");

        let before: Value = serde_json::from_slice(&golden).expect("parse golden");
        let after: Value = serde_json::from_str(&reemitted).expect("parse re-emitted");
        if let Some(pointer) = first_difference(&before, &after, &mut String::new()) {
            failures.push(format!("{}: mismatch at {pointer}", path.display()));
        }
    }

    assert!(
        failures.is_empty(),
        "{} file(s) failed to round-trip:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
