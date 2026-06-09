//! Shared helpers for the golden snapshot tests: corpus discovery and exclusions parsing.
//!
//! The corpus lives at the repo root and is shared with the shell conformance suite; these helpers
//! resolve it relative to the crate manifest so the golden tests run fully offline.

// A shared test toolbox: each integration-test binary that includes this module uses a subset of it.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

/// Repo-root `corpus/` directory.
pub(crate) fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus")
}

/// One discovered corpus case.
#[derive(Debug)]
pub(crate) struct Case {
    /// The subdirectory name: a reader format (`text/`) or a writer feature (`ast/`).
    pub group: String,
    /// The file stem.
    pub label: String,
    pub input: String,
}

/// Every file under `corpus/<kind>/<group>/`, ordered by (group, label) for stable snapshot names.
pub(crate) fn corpus_cases(kind: &str) -> Vec<Case> {
    let root = corpus_dir().join(kind);
    let mut groups: Vec<PathBuf> = read_dir_sorted(&root)
        .into_iter()
        .filter(|path| path.is_dir())
        .collect();
    groups.sort();

    let mut cases = Vec::new();
    for group_dir in groups {
        let group = file_name(&group_dir);
        for file in read_dir_sorted(&group_dir) {
            if !file.is_file() {
                continue;
            }
            let label = file
                .file_stem()
                .unwrap_or_else(|| panic!("no stem: {}", file.display()))
                .to_string_lossy()
                .into_owned();
            let input = fs::read_to_string(&file)
                .unwrap_or_else(|error| panic!("read {}: {error}", file.display()));
            cases.push(Case {
                group: group.clone(),
                label,
                input,
            });
        }
    }
    cases
}

/// The `(target, feature)` pairs the writers cannot yet render, from `corpus/exclusions.tsv`.
pub(crate) fn exclusions() -> Vec<(String, String)> {
    let path = corpus_dir().join("exclusions.tsv");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once('\t'))
        .map(|(target, feature)| (target.to_owned(), feature.to_owned()))
        .collect()
}

pub(crate) fn is_excluded(exclusions: &[(String, String)], target: &str, feature: &str) -> bool {
    exclusions.iter().any(|(t, f)| t == target && f == feature)
}

fn read_dir_sorted(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("read dir {}: {error}", dir.display()))
        .map(|entry| entry.expect("read dir entry").path())
        .collect();
    entries.sort();
    entries
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_else(|| panic!("no file name: {}", path.display()))
        .to_string_lossy()
        .into_owned()
}
