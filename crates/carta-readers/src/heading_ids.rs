//! Shared heading-identifier derivation for the readers that build identifiers from header text.
//!
//! Two slug shapes are available, selected by the active extension:
//!
//! - `auto_identifiers` — keep alphanumerics, `_`, `-`, `.`, and whitespace; collapse each
//!   whitespace run to a single `-`; strip the leading run up to the first letter.
//! - `gfm_auto_identifiers` — keep alphanumerics, combining marks, `_`, and `-`; turn each
//!   whitespace character into a single `-`; drop everything else (including `.`); no leading strip.
//!
//! Two disambiguation strategies sit on top of the slug:
//!
//! - native — an empty slug becomes `section`; repeats increment a numeric suffix until the whole
//!   identifier is unused against every identifier already issued or reserved.
//! - count-suffix — repeats are disambiguated by a per-base occurrence count (which can itself
//!   collide with a slug that already carries that suffix), and an empty slug stays empty.
//!
//! The disambiguation strategy is chosen by the dialect, not the slug shape: the broad Markdown
//! dialect uses native disambiguation for every slug shape (so its GitHub-slug variant still maps an
//! empty slug to `section`), while the bare `CommonMark` engine pairs the GitHub slug with count-suffix.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

#[cfg(any(feature = "commonmark", feature = "rst", feature = "dokuwiki"))]
use carta_ast::{slug, slug_gfm};
#[cfg(any(
    feature = "commonmark",
    feature = "rst",
    feature = "dokuwiki",
    feature = "latex",
    feature = "man",
    feature = "org"
))]
use carta_core::{Extension, Extensions};

/// The identifier-derivation algorithm an auto-identifier extension selects.
#[cfg(any(
    feature = "commonmark",
    feature = "rst",
    feature = "dokuwiki",
    feature = "latex",
    feature = "man",
    feature = "org"
))]
#[derive(Clone, Copy)]
pub(crate) enum IdScheme {
    Plain,
    Gfm,
}

#[cfg(any(
    feature = "commonmark",
    feature = "rst",
    feature = "dokuwiki",
    feature = "latex",
    feature = "man",
    feature = "org"
))]
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
    #[cfg(any(feature = "commonmark", feature = "rst", feature = "dokuwiki"))]
    counts: BTreeMap<String, u32>,
    /// The next suffix to try for a given `(base, separator)`, used by the increment-until-unique
    /// strategy so a repeated collision resumes probing where the last one left off instead of
    /// restarting at 1. A value here is only ever a lower bound: `seen` remains the source of truth,
    /// since an explicit id can still occupy the next few candidates.
    next_suffix: BTreeMap<(String, char), u32>,
}

impl IdRegistry {
    /// Derive an identifier for `text` under `scheme`, disambiguating it against every identifier
    /// already emitted or reserved.
    #[cfg(any(feature = "commonmark", feature = "rst", feature = "dokuwiki"))]
    pub(crate) fn assign(&mut self, scheme: IdScheme, text: &str) -> String {
        match scheme {
            IdScheme::Plain => self.assign_native(slug(text)),
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

    /// Disambiguate an already-slugged `base` with the native strategy: an empty base becomes
    /// `section`, and a repeated base gains a numeric suffix incremented until the whole identifier
    /// is unused against every identifier already issued or reserved. The suffix is joined with `-`.
    #[cfg(any(
        feature = "commonmark",
        feature = "rst",
        feature = "dokuwiki",
        feature = "latex",
        feature = "man",
        feature = "org"
    ))]
    pub(crate) fn assign_native(&mut self, base: String) -> String {
        self.assign_with_separator(base, '-')
    }

    /// The native disambiguation strategy with a caller-chosen `separator` between the base and its
    /// numeric suffix: an empty base becomes `section`, and a repeated base gains a suffix
    /// incremented until the whole identifier is unused against every identifier already issued.
    pub(crate) fn assign_with_separator(&mut self, base: String, separator: char) -> String {
        let base = if base.is_empty() {
            "section".to_owned()
        } else {
            base
        };
        if self.seen.insert(base.clone()) {
            return base;
        }
        let key = (base.clone(), separator);
        let mut suffix = self.next_suffix.get(&key).copied().unwrap_or(1);
        loop {
            let candidate = format!("{base}{separator}{suffix}");
            if self.seen.insert(candidate.clone()) {
                self.next_suffix.insert(key, suffix + 1);
                return candidate;
            }
            suffix += 1;
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

    /// Derive an identifier under `scheme`'s slug algorithm but with the native (increment-until-
    /// unique, empty-becomes-`section`) disambiguation — the strategy the broad Markdown dialect
    /// applies to every slug shape, including the GitHub slug. The bare `CommonMark` engine instead
    /// pairs each slug shape with its own disambiguation via [`Self::assign`].
    #[cfg(feature = "commonmark")]
    pub(crate) fn assign_markdown(&mut self, scheme: IdScheme, text: &str) -> String {
        let base = match scheme {
            IdScheme::Plain => slug(text),
            IdScheme::Gfm => slug_gfm(text),
        };
        self.assign_native(base)
    }

    /// Reserve `id` for the increment-until-unique strategy so a later derived identifier avoids it,
    /// regardless of the slug shape in use. The broad Markdown dialect reserves every explicit
    /// identifier this way, and the latex and org readers reserve identically so a later derived id
    /// avoids a heading's explicit id.
    #[cfg(any(feature = "commonmark", feature = "latex", feature = "org"))]
    pub(crate) fn reserve_native(&mut self, id: &str) {
        self.seen.insert(id.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(feature = "commonmark", feature = "rst"))]
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

    #[cfg(any(feature = "commonmark", feature = "rst"))]
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
    fn markdown_pairs_the_gfm_slug_with_native_disambiguation() {
        // The broad Markdown dialect keeps the GitHub slug shape but disambiguates natively: an
        // empty slug becomes `section` and increments, unlike the bare engine's count-suffix.
        let mut registry = IdRegistry::default();
        assert_eq!(registry.assign_markdown(IdScheme::Gfm, "???"), "section");
        assert_eq!(registry.assign_markdown(IdScheme::Gfm, "!!!"), "section-1");
        // A non-empty GitHub slug is still produced by the GitHub algorithm.
        assert_eq!(
            registry.assign_markdown(IdScheme::Gfm, "Hello, World."),
            "hello-world"
        );
    }

    #[cfg(feature = "commonmark")]
    #[test]
    fn gfm_scheme_does_not_reserve_explicit_identifiers() {
        let mut registry = IdRegistry::default();
        registry.reserve(IdScheme::Gfm, "intro");
        assert_eq!(registry.assign(IdScheme::Gfm, "Intro"), "intro");
    }

    #[test]
    fn assign_with_separator_resumes_probing_from_the_last_issued_suffix() {
        let mut registry = IdRegistry::default();
        assert_eq!(
            registry.assign_with_separator("base".to_owned(), '-'),
            "base"
        );
        for suffix in 1..5 {
            assert_eq!(
                registry.assign_with_separator("base".to_owned(), '-'),
                format!("base-{suffix}")
            );
        }
    }

    #[test]
    fn assign_with_separator_skips_a_reserved_suffix_exactly_once() {
        let mut registry = IdRegistry::default();
        registry.seen.insert("base-2".to_owned());
        assert_eq!(
            registry.assign_with_separator("base".to_owned(), '-'),
            "base"
        );
        assert_eq!(
            registry.assign_with_separator("base".to_owned(), '-'),
            "base-1"
        );
        assert_eq!(
            registry.assign_with_separator("base".to_owned(), '-'),
            "base-3"
        );
    }

    #[test]
    fn assign_with_separator_stays_consistent_when_a_reservation_lands_on_the_memo() {
        let mut registry = IdRegistry::default();
        assert_eq!(
            registry.assign_with_separator("base".to_owned(), '-'),
            "base"
        );
        assert_eq!(
            registry.assign_with_separator("base".to_owned(), '-'),
            "base-1"
        );
        // The memo now points at suffix 2; reserve that exact candidate before the next assignment.
        registry.seen.insert("base-2".to_owned());
        assert_eq!(
            registry.assign_with_separator("base".to_owned(), '-'),
            "base-3"
        );
    }

    #[test]
    fn assign_with_separator_keys_the_memo_by_separator() {
        let mut registry = IdRegistry::default();
        assert_eq!(
            registry.assign_with_separator("same".to_owned(), '-'),
            "same"
        );
        assert_eq!(
            registry.assign_with_separator("same".to_owned(), '-'),
            "same-1"
        );
        // The `_` separator has its own memo, so it starts probing at 1, not at the `-` memo's 2 — the
        // bare `same` is already in `seen`, so `same_1` is the first available `_` candidate.
        assert_eq!(
            registry.assign_with_separator("same".to_owned(), '_'),
            "same_1"
        );
    }
}
