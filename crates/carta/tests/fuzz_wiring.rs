//! Structural guard keeping the fuzz wiring in lockstep with the reader set.
//!
//! Fuzzing a reader takes three hand-maintained pieces that must agree: a target source in
//! `fuzz/fuzz_targets/`, a `[[bin]]` in `fuzz/Cargo.toml`, and at least one committed seed in
//! `fuzz/seeds/<target>/` for the deterministic PR replay. (Both CI workflows derive their target
//! list from `fuzz/Cargo.toml` at runtime, so there is no matrix to keep in sync.) Adding a reader
//! without all three is a silent coverage gap: the reader goes unfuzzed. These tests turn
//! that gap into a loud, offline failure that names what is missing.

// Test harness code: panicking on a malformed workspace layout is the idiomatic assertion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Repository root, relative to this package's manifest dir (`<root>/crates/carta`).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fuzz_targets() -> BTreeSet<String> {
    let dir = repo_root().join("fuzz/fuzz_targets");
    fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .map(|path| path.file_stem().unwrap().to_string_lossy().into_owned())
        .collect()
}

fn seed_dirs() -> BTreeSet<String> {
    let dir = repo_root().join("fuzz/seeds");
    fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

fn seed_corpus_is_nonempty(target: &str) -> bool {
    let dir = repo_root().join("fuzz/seeds").join(target);
    fs::read_dir(&dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .any(|entry| entry.path().is_file())
        })
        .unwrap_or(false)
}

fn cargo_bin_names() -> BTreeSet<String> {
    let manifest = repo_root().join("fuzz/Cargo.toml");
    let text = fs::read_to_string(&manifest)
        .unwrap_or_else(|error| panic!("read {}: {error}", manifest.display()));
    let mut names = BTreeSet::new();
    let mut in_bin = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_bin = trimmed == "[[bin]]";
        } else if in_bin
            && trimmed.starts_with("name")
            && let Some(name) = trimmed.split('"').nth(1)
        {
            names.insert(name.to_owned());
        }
    }
    names
}

#[test]
fn every_reader_has_a_fuzz_target() {
    let targets = fuzz_targets();
    for reader in carta::supported_input_formats() {
        let target = format!("read_{reader}");
        assert!(
            targets.contains(&target),
            "reader `{reader}` has no fuzz target. Add fuzz/fuzz_targets/{target}.rs (with a \
             matching [[bin]] in fuzz/Cargo.toml) and a seed under fuzz/seeds/{target}/; both CI \
             workflows pick the new target up from fuzz/Cargo.toml automatically."
        );
    }
}

#[test]
fn every_fuzz_target_has_a_nonempty_seed_corpus() {
    for target in fuzz_targets() {
        assert!(
            seed_corpus_is_nonempty(&target),
            "fuzz target `{target}` has no committed seed. Add at least one input under \
             fuzz/seeds/{target}/ — cargo-fuzz errors on an empty corpus dir, so an empty one also \
             breaks the deterministic PR replay."
        );
    }
}

#[test]
fn targets_seeds_and_bins_agree() {
    let targets = fuzz_targets();
    assert_eq!(
        targets,
        seed_dirs(),
        "fuzz/fuzz_targets/ and fuzz/seeds/ disagree: every target needs a seed directory and \
         every seed directory needs a target."
    );
    assert_eq!(
        targets,
        cargo_bin_names(),
        "fuzz/fuzz_targets/ and the [[bin]] table in fuzz/Cargo.toml disagree: each target source \
         needs a matching [[bin]] entry."
    );
}
