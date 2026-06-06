//! Offline round-trip tests over the committed hand-authored fixtures. These need no oracle, so
//! they keep a meaningful round-trip gate green for agents and CI even without the corpus.

// This whole file is test code, where panicking on a known fixture is the idiomatic assertion.
// clippy's `allow-*-in-tests` only covers `#[cfg(test)]`/`#[test]` items, not integration-test
// helpers, so the panic-discipline lints are relaxed crate-wide here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::PathBuf;

use oxidoc_testkit::roundtrip::{self, Roundtrip};
use oxidoc_testkit::roundtrip_fixtures_dir;

fn fixture_files() -> Vec<PathBuf> {
    let dir = roundtrip_fixtures_dir();
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("read fixtures dir {}: {error}", dir.display()))
        .map(|entry| entry.expect("read fixture entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    files
}

#[test]
fn fixtures_round_trip() {
    let files = fixture_files();
    assert!(!files.is_empty(), "no fixtures found");

    let mut failures = Vec::new();
    for path in &files {
        let bytes = fs::read(path).expect("read fixture");
        match roundtrip::check(&bytes) {
            Ok(Roundtrip::Match) => {}
            Ok(Roundtrip::Mismatch { pointer }) => {
                failures.push(format!("{}: value mismatch at {pointer}", path.display()));
            }
            Err(error) => failures.push(format!("{}: parse failed: {error}", path.display())),
        }
    }

    assert!(
        failures.is_empty(),
        "{} fixture(s) failed to round-trip:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
