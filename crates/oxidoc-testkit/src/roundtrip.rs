//! Differential round-trip harness: parse pandoc-minted JSON through the oxidoc document model and
//! re-emit it, then assert the result is semantically identical to the input.
//!
//! The corpus's `*.native` files are themselves ASTs, so minting their JSON with
//! `pandoc -f native -t json` exercises the JSON codec alone — no reader is involved. Minted JSON
//! is cached under `.oracle/cache/native-json/` keyed by the source bytes and the pinned pandoc
//! version, so reruns don't re-spawn pandoc.
//!
//! The gate is `serde_json::Value` equality, not byte equality: it ignores key order and float
//! formatting (pandoc and ryu pick different shortest representations of the same `f64`) while still
//! catching dropped or altered fields.

use std::collections::BTreeSet;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::{oracle_dir, pandoc_bin, pandoc_tests_dir};

/// Recursively collect the `*.native` files in the fetched corpus, sorted for deterministic
/// ordering. Returns an empty vec when the corpus is absent; callers decide whether that is fatal.
pub fn native_files() -> io::Result<Vec<PathBuf>> {
    crate::collect_files_with_extension(&pandoc_tests_dir(), "native")
}

/// The pinned pandoc version string (contents of `.oracle/PANDOC_VERSION`), trimmed. Part of the
/// cache key so minted JSON is invalidated when the oracle is re-pinned.
fn pandoc_version() -> io::Result<String> {
    Ok(fs::read_to_string(oracle_dir().join("PANDOC_VERSION"))?
        .trim()
        .to_owned())
}

fn cache_dir() -> PathBuf {
    oracle_dir().join("cache/native-json")
}

fn cache_key(version: &str, source: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    version.hash(&mut hasher);
    source.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Outcome of minting one `.native` file. A file the binary itself rejects (a deliberately
/// malformed corpus entry) is `Rejected`, not an error.
#[derive(Debug)]
pub enum Mint {
    Ok(Vec<u8>),
    Rejected { stderr: String },
}

/// Convert one `.native` file to JSON with the pinned pandoc binary, caching the result.
pub fn mint_golden(native: &Path) -> io::Result<Mint> {
    let source = fs::read(native)?;
    let version = pandoc_version()?;
    let cache_path = cache_dir().join(format!("{}.json", cache_key(&version, &source)));

    if let Ok(cached) = fs::read(&cache_path) {
        return Ok(Mint::Ok(cached));
    }

    let output = Command::new(pandoc_bin())
        .args(["-f", "native", "-t", "json"])
        .arg(native)
        .output()?;

    if !output.status.success() {
        return Ok(Mint::Rejected {
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    fs::create_dir_all(cache_dir())?;
    fs::write(&cache_path, &output.stdout)?;
    Ok(Mint::Ok(output.stdout))
}

/// Result of round-tripping one golden JSON document through the model.
#[derive(Debug)]
pub enum Roundtrip {
    Match,
    /// The re-emitted JSON differs; `pointer` locates the first divergence (JSON-pointer style).
    Mismatch {
        pointer: String,
    },
}

/// Parse `golden` into the document model, re-serialize it, and compare by `serde_json::Value`.
///
/// A deserialize error is returned (not swallowed): it means the corpus uses a tag or shape the
/// model does not cover, which is a real gap rather than a value mismatch.
pub fn check(golden: &[u8]) -> serde_json::Result<Roundtrip> {
    let document = oxidoc_ast::from_json(golden)?;
    let reemitted = oxidoc_ast::to_json(&document)?;

    let before: Value = serde_json::from_slice(golden)?;
    let after: Value = serde_json::from_slice(reemitted.as_bytes())?;

    Ok(
        match first_difference(&before, &after, &mut String::new()) {
            Some(pointer) => Roundtrip::Mismatch { pointer },
            None => Roundtrip::Match,
        },
    )
}

/// The JSON pointer of the first place `before` and `after` diverge, or `None` if they are equal.
/// Recurses structurally so a mismatch deep in a table is reported as e.g. `/blocks/0/c/2`.
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

/// Collect every `"t"` tag string present in a JSON document. This over-collects (it also picks up
/// small-enum tags like `AlignDefault`), which is harmless: the coverage test only checks that the
/// known node tags are a subset of what was seen.
pub fn collect_tags(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(tag)) = map.get("t") {
                out.insert(tag.clone());
            }
            for nested in map.values() {
                collect_tags(nested, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_tags(item, out);
            }
        }
        _ => {}
    }
}
