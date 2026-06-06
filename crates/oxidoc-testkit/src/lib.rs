//! The oxidoc differential test harness.
//!
//! Drives the pinned pandoc binary — installed black-box into the gitignored `.pandoc-ref/`
//! directory by `tools/install-pandoc.sh` (see `docs/PORTING.md` §5) — as the correctness oracle,
//! and diffs its output against oxidoc across the two oracle surfaces (reader and writer).
//!
//! The runner, fixture cache, and diff/reporting land in step 4; this exposes only path discovery
//! for now so the workspace builds.

use std::path::{Path, PathBuf};

/// Path to the gitignored pandoc reference directory at the workspace root.
///
/// Resolved relative to this crate's manifest, so it is independent of the current working
/// directory. The directory exists only after `tools/install-pandoc.sh` has been run.
#[must_use]
pub fn pandoc_ref_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pandoc-ref")
}

/// Path to the pinned pandoc binary inside [`pandoc_ref_dir`].
#[must_use]
pub fn pandoc_bin() -> PathBuf {
    pandoc_ref_dir().join("bin/pandoc")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ref_paths_anchor_at_workspace_root() {
        assert!(pandoc_ref_dir().ends_with(".pandoc-ref"));
        assert!(pandoc_bin().ends_with("bin/pandoc"));
    }
}
