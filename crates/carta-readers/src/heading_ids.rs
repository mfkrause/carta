//! Shared heading-identifier disambiguation for the readers that derive identifiers from header
//! text. Two algorithms are supported; they differ in which characters survive, whitespace handling,
//! leading-character stripping, the empty-result fallback, and how repeated slugs are disambiguated:
//!
//! - `auto_identifiers` — keep alphanumerics, `_`, `-`, `.`, and whitespace; collapse each
//!   whitespace run to a single `-`; strip the leading run up to the first letter; an empty result
//!   becomes `section`. Repeats increment a numeric suffix until the whole identifier is unused, and
//!   explicit identifiers are reserved against that set.
//! - `gfm_auto_identifiers` — keep alphanumerics, `_`, and `-`; turn each whitespace character into a
//!   single `-`; drop everything else (including `.`); no leading strip; an empty result stays empty.
//!   Repeats are disambiguated by a per-base occurrence count, which can itself produce a collision.

use std::collections::{BTreeMap, BTreeSet};

use carta_ast::{slug, slug_gfm};
use carta_core::{Extension, Extensions};

/// The identifier-derivation algorithm an auto-identifier extension selects.
#[derive(Clone, Copy)]
pub(crate) enum IdScheme {
    Plain,
    Gfm,
}

impl IdScheme {
    /// The scheme the active extensions select, or `None` when no auto-identifier extension is on.
    ///
    /// In the broad Markdown dialect, `auto_identifiers` is the master switch: with it off, no header
    /// is numbered even if `gfm_auto_identifiers` is on (the latter only chooses the slug algorithm).
    /// The bare `CommonMark` engine has no such master switch — there `gfm_auto_identifiers` alone
    /// drives numbering.
    pub(crate) fn select(extensions: Extensions, markdown: bool) -> Option<Self> {
        if markdown && !extensions.contains(Extension::AutoIdentifiers) {
            return None;
        }
        if extensions.contains(Extension::GfmAutoIdentifiers) {
            Some(Self::Gfm)
        } else if extensions.contains(Extension::AutoIdentifiers) {
            Some(Self::Plain)
        } else {
            None
        }
    }
}

/// Tracks identifiers already in use so repeats can be disambiguated.
#[derive(Default)]
pub(crate) struct IdRegistry {
    /// Every identifier emitted or reserved, used by the increment-until-unique strategy.
    seen: BTreeSet<String>,
    /// Per-base occurrence counts, used by the count-suffix strategy.
    counts: BTreeMap<String, u32>,
}

impl IdRegistry {
    /// Derive an identifier for `text` under `scheme`, disambiguating it against every identifier
    /// already emitted or reserved.
    pub(crate) fn assign(&mut self, scheme: IdScheme, text: &str) -> String {
        match scheme {
            IdScheme::Plain => {
                let base = {
                    let slugged = slug(text);
                    if slugged.is_empty() {
                        "section".to_owned()
                    } else {
                        slugged
                    }
                };
                if self.seen.insert(base.clone()) {
                    return base;
                }
                let mut suffix = 1u32;
                loop {
                    let candidate = format!("{base}-{suffix}");
                    if self.seen.insert(candidate.clone()) {
                        return candidate;
                    }
                    suffix += 1;
                }
            }
            IdScheme::Gfm => {
                let base = slug_gfm(text);
                let count = self.counts.entry(base.clone()).or_insert(0);
                let result = if *count == 0 {
                    base.clone()
                } else {
                    format!("{base}-{count}")
                };
                *count += 1;
                result
            }
        }
    }

    /// Reserve an explicit identifier so later derived ids avoid it. Only the increment-until-unique
    /// (`Plain`) scheme reserves; the count-suffix scheme tracks bases independently.
    #[cfg(feature = "commonmark")]
    pub(crate) fn reserve(&mut self, scheme: IdScheme, id: &str) {
        if let IdScheme::Plain = scheme {
            self.seen.insert(id.to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_scheme_falls_back_to_section_and_increments() {
        let mut registry = IdRegistry::default();
        assert_eq!(
            registry.assign(IdScheme::Plain, "Hello, World!"),
            "hello-world"
        );
        assert_eq!(
            registry.assign(IdScheme::Plain, "Hello World"),
            "hello-world-1"
        );
        assert_eq!(registry.assign(IdScheme::Plain, "!!!"), "section");
        assert_eq!(registry.assign(IdScheme::Plain, "???"), "section-1");
    }

    #[cfg(feature = "commonmark")]
    #[test]
    fn plain_scheme_avoids_reserved_identifiers() {
        let mut registry = IdRegistry::default();
        registry.reserve(IdScheme::Plain, "intro");
        assert_eq!(registry.assign(IdScheme::Plain, "Intro"), "intro-1");
    }

    #[test]
    fn gfm_scheme_counts_repeats_per_base() {
        let mut registry = IdRegistry::default();
        assert_eq!(registry.assign(IdScheme::Gfm, "Hello World"), "hello-world");
        assert_eq!(
            registry.assign(IdScheme::Gfm, "Hello World"),
            "hello-world-1"
        );
    }

    #[cfg(feature = "commonmark")]
    #[test]
    fn gfm_scheme_does_not_reserve_explicit_identifiers() {
        let mut registry = IdRegistry::default();
        registry.reserve(IdScheme::Gfm, "intro");
        assert_eq!(registry.assign(IdScheme::Gfm, "Intro"), "intro");
    }
}
