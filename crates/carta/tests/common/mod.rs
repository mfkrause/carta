//! Golden-test corpus discovery relative to the crate manifest.

// A shared test toolbox: each integration-test binary that includes this module uses a subset of it.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

/// Repo-root `corpus/` directory.
pub(crate) fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus")
}

/// One discovered corpus case, decoded as UTF-8 text.
#[derive(Debug)]
pub(crate) struct Case {
    /// The subdirectory name: a reader format (`text/`) or a writer feature (`ast/`).
    pub group: String,
    /// The file stem.
    pub label: String,
    pub input: String,
}

/// One discovered corpus case, kept as raw bytes for a byte-container reader format whose fixture is
/// a binary archive rather than UTF-8 text.
#[derive(Debug)]
pub(crate) struct BinaryCase {
    /// The subdirectory name: a reader format (`binary/<fmt>/`).
    pub group: String,
    /// The file stem.
    pub label: String,
    pub input: Vec<u8>,
}

/// `(group, label, path)` for every file under `corpus/<kind>/<group>/`, ordered by (group, label)
/// for stable snapshot names. Callers read each path as text ([`corpus_cases`]) or raw bytes
/// ([`corpus_binary_cases`]).
fn corpus_files(kind: &str) -> Vec<(String, String, PathBuf)> {
    let root = corpus_dir().join(kind);
    let mut groups: Vec<PathBuf> = read_dir_sorted(&root)
        .into_iter()
        .filter(|path| path.is_dir())
        .collect();
    groups.sort();

    let mut files = Vec::new();
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
            files.push((group.clone(), label, file));
        }
    }
    files
}

/// The subdirectory names under `corpus/<kind>/`, sorted: the group set the golden tests partition
/// into one test each. Returns empty when the `kind` tree is absent. The per-format guard tests
/// compare this against their macro's covered-group list so a new corpus directory without a matching
/// test fails.
pub(crate) fn corpus_groups(kind: &str) -> Vec<String> {
    let root = corpus_dir().join(kind);
    if !root.is_dir() {
        return Vec::new();
    }
    let mut groups: Vec<String> = read_dir_sorted(&root)
        .into_iter()
        .filter(|path| path.is_dir())
        .map(|path| file_name(&path))
        .collect();
    groups.sort();
    groups
}

/// Every file under `corpus/<kind>/<group>/` decoded as UTF-8 text, ordered by (group, label).
pub(crate) fn corpus_cases(kind: &str) -> Vec<Case> {
    corpus_files(kind)
        .into_iter()
        .map(|(group, label, file)| {
            let input = fs::read_to_string(&file)
                .unwrap_or_else(|error| panic!("read {}: {error}", file.display()));
            Case {
                group,
                label,
                input,
            }
        })
        .collect()
}

/// Every file under `corpus/<kind>/<group>/` read as RAW BYTES, ordered by (group, label). A byte-
/// container reader format (whose fixture is a binary archive, not UTF-8 text) keeps its cases here,
/// since [`corpus_cases`]'s `read_to_string` would reject them. Returns empty when the `kind` tree is
/// absent; the binary corpus exists only once a byte-container reader ships.
pub(crate) fn corpus_binary_cases(kind: &str) -> Vec<BinaryCase> {
    if !corpus_dir().join(kind).is_dir() {
        return Vec::new();
    }
    corpus_files(kind)
        .into_iter()
        .map(|(group, label, file)| {
            let input =
                fs::read(&file).unwrap_or_else(|error| panic!("read {}: {error}", file.display()));
            BinaryCase {
                group,
                label,
                input,
            }
        })
        .collect()
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
