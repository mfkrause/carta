//! Differential round-trip tests over pandoc's `*.native` corpus, minted to JSON by the pinned
//! binary. The corpus is hard-required: its absence fails (with provisioning instructions) rather
//! than silently skipping, so a green run means real coverage.

// This whole file is test code, where panicking on a known fixture is the idiomatic assertion.
// clippy's `allow-*-in-tests` only covers `#[cfg(test)]`/`#[test]` items, not integration-test
// helpers, so the panic-discipline lints are relaxed crate-wide here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use oxidoc_ast::{BLOCK_TAGS, INLINE_TAGS, META_VALUE_TAGS};
use oxidoc_testkit::roundtrip::{self, Mint, Roundtrip};
use oxidoc_testkit::{pandoc_tests_dir, roundtrip_fixtures_dir};

fn require_corpus() -> Vec<PathBuf> {
    let files = roundtrip::native_files().expect("enumerate corpus");
    assert!(
        !files.is_empty(),
        "pandoc test corpus not found at {}.\nRun tools/install-pandoc.sh then tools/fetch-pandoc-tests.sh.",
        pandoc_tests_dir().display()
    );
    files
}

#[test]
fn corpus_round_trips_through_the_model() {
    let files = require_corpus();

    let mut failures = Vec::new();
    let mut rejected = 0usize;
    let mut checked = 0usize;

    for file in &files {
        match roundtrip::mint_golden(file).expect("mint golden json") {
            Mint::Rejected { .. } => rejected += 1,
            Mint::Ok(golden) => match roundtrip::check(&golden) {
                Ok(Roundtrip::Match) => checked += 1,
                Ok(Roundtrip::Mismatch { pointer }) => {
                    failures.push(format!("{}: value mismatch at {pointer}", file.display()));
                }
                Err(error) => failures.push(format!("{}: parse failed: {error}", file.display())),
            },
        }
    }

    assert!(
        failures.is_empty(),
        "{}/{} corpus documents failed ({rejected} pandoc-rejected, {checked} matched):\n{}",
        failures.len(),
        files.len(),
        failures.join("\n")
    );
}

#[test]
fn corpus_and_fixtures_cover_every_node_tag() {
    let mut seen = BTreeSet::new();

    for file in require_corpus() {
        if let Mint::Ok(golden) = roundtrip::mint_golden(&file).expect("mint golden json") {
            let value: serde_json::Value =
                serde_json::from_slice(&golden).expect("minted json is valid");
            roundtrip::collect_tags(&value, &mut seen);
        }
    }

    let fixtures_dir = roundtrip_fixtures_dir();
    for entry in fs::read_dir(&fixtures_dir).expect("read fixtures dir") {
        let path = entry.expect("read fixture entry").path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let bytes = fs::read(&path).expect("read fixture");
            let value: serde_json::Value =
                serde_json::from_slice(&bytes).expect("fixture is valid json");
            roundtrip::collect_tags(&value, &mut seen);
        }
    }

    let missing: Vec<&str> = BLOCK_TAGS
        .iter()
        .chain(INLINE_TAGS)
        .chain(META_VALUE_TAGS)
        .copied()
        .filter(|tag| !seen.contains(*tag))
        .collect();

    assert!(
        missing.is_empty(),
        "node tags modeled but never exercised by corpus or fixtures: {missing:?}\nAdd a fixture under {} that uses them.",
        fixtures_dir.display()
    );
}
