//! The oxidoc differential test harness.
//!
//! Drives the pinned pandoc binary — installed black-box into the gitignored `.oracle/`
//! directory by `tools/install-pandoc.sh` (see `docs/PORTING.md` §5) — as the correctness oracle,
//! and diffs its output against oxidoc across the two oracle surfaces (reader and writer).
//!
//! The runner, fixture cache, and diff/reporting land in step 4; this exposes only path discovery
//! for now so the workspace builds.

use std::path::{Path, PathBuf};

pub mod command_test;

/// Path to the gitignored oracle directory at the workspace root, holding the pinned pandoc binary
/// and fetched test corpus.
///
/// Resolved relative to this crate's manifest, so it is independent of the current working
/// directory. The directory exists only after `tools/install-pandoc.sh` has been run.
#[must_use]
pub fn oracle_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.oracle")
}

/// Path to the pinned pandoc binary inside [`oracle_dir`].
#[must_use]
pub fn pandoc_bin() -> PathBuf {
    oracle_dir().join("bin/pandoc")
}

/// Root of the fetched pandoc test corpus (`.oracle/tests/test`), populated by
/// `tools/fetch-pandoc-tests.sh`. Exists only after that script has run.
#[must_use]
pub fn pandoc_tests_dir() -> PathBuf {
    oracle_dir().join("tests/test")
}

/// Path to pandoc's command-test files (`test/command`) within the fetched corpus.
#[must_use]
pub fn command_tests_dir() -> PathBuf {
    pandoc_tests_dir().join("command")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ref_paths_anchor_at_workspace_root() {
        assert!(oracle_dir().ends_with(".oracle"));
        assert!(pandoc_bin().ends_with("bin/pandoc"));
        assert!(pandoc_tests_dir().ends_with("tests/test"));
        assert!(command_tests_dir().ends_with("command"));
    }
}
