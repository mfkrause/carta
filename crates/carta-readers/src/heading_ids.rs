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
    pub(crate) fn select(extensions: Extensions) -> Option<Self> {
        if extensions.contains(Extension::GfmAutoIdentifiers) {
            Some(Self::Gfm)
        } else if extensions.contains(Extension::AutoIdentifiers) {
            Some(Self::Plain)
        } else {
            None
        }
    }
}

/// Transliterates header text to ASCII for the `ascii_identifiers` extension: each accented letter is
/// folded to its unaccented base, plain ASCII is kept, and every other character (a letter with no
/// ASCII base, or a non-Latin script) is dropped. The result is then slugged as usual.
pub(crate) fn fold_to_ascii(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c);
        } else if let Some(base) = ascii_base(c) {
            out.push(base);
        }
    }
    out
}

/// The unaccented ASCII letter underlying a Latin letter with a diacritic, or `None` for a character
/// with no single-letter ASCII base (so the caller drops it).
#[allow(clippy::match_same_arms)]
fn ascii_base(c: char) -> Option<char> {
    let base = match c {
        'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' | 'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'Ā' | 'ā' | 'Ă'
        | 'ă' | 'Ą' | 'ą' | 'Ǎ' | 'ǎ' | 'Ǟ' | 'ǟ' | 'Ǡ' | 'ǡ' | 'Ǻ' | 'ǻ' | 'Ȁ' | 'ȁ' | 'Ȃ'
        | 'ȃ' | 'Ȧ' | 'ȧ' => 'a',
        'Ç' | 'ç' | 'Ć' | 'ć' | 'Ĉ' | 'ĉ' | 'Ċ' | 'ċ' | 'Č' | 'č' => 'c',
        'Ď' | 'ď' => 'd',
        'È' | 'É' | 'Ê' | 'Ë' | 'è' | 'é' | 'ê' | 'ë' | 'Ē' | 'ē' | 'Ĕ' | 'ĕ' | 'Ė' | 'ė' | 'Ę'
        | 'ę' | 'Ě' | 'ě' | 'Ȅ' | 'ȅ' | 'Ȇ' | 'ȇ' | 'Ȩ' | 'ȩ' => 'e',
        'Ĝ' | 'ĝ' | 'Ğ' | 'ğ' | 'Ġ' | 'ġ' | 'Ģ' | 'ģ' | 'Ǧ' | 'ǧ' | 'Ǵ' | 'ǵ' => 'g',
        'Ĥ' | 'ĥ' | 'Ȟ' | 'ȟ' => 'h',
        'Ì' | 'Í' | 'Î' | 'Ï' | 'ì' | 'í' | 'î' | 'ï' | 'Ĩ' | 'ĩ' | 'Ī' | 'ī' | 'Ĭ' | 'ĭ' | 'Į'
        | 'į' | 'İ' | 'ı' | 'Ǐ' | 'ǐ' | 'Ȉ' | 'ȉ' | 'Ȋ' | 'ȋ' => 'i',
        'Ĵ' | 'ĵ' | 'ǰ' => 'j',
        'Ķ' | 'ķ' | 'Ǩ' | 'ǩ' => 'k',
        'Ĺ' | 'ĺ' | 'Ļ' | 'ļ' | 'Ľ' | 'ľ' => 'l',
        'Ñ' | 'ñ' | 'Ń' | 'ń' | 'Ņ' | 'ņ' | 'Ň' | 'ň' | 'Ǹ' | 'ǹ' => 'n',
        'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'Ō' | 'ō' | 'Ŏ' | 'ŏ' | 'Ő'
        | 'ő' | 'Ơ' | 'ơ' | 'Ǒ' | 'ǒ' | 'Ǫ' | 'ǫ' | 'Ǭ' | 'ǭ' | 'Ȍ' | 'ȍ' | 'Ȏ' | 'ȏ' | 'Ȫ'
        | 'ȫ' | 'Ȭ' | 'ȭ' | 'Ȯ' | 'ȯ' | 'Ȱ' | 'ȱ' => 'o',
        'Ŕ' | 'ŕ' | 'Ŗ' | 'ŗ' | 'Ř' | 'ř' | 'Ȑ' | 'ȑ' | 'Ȓ' | 'ȓ' => 'r',
        'Ś' | 'ś' | 'Ŝ' | 'ŝ' | 'Ş' | 'ş' | 'Š' | 'š' | 'Ș' | 'ș' => 's',
        'Ţ' | 'ţ' | 'Ť' | 'ť' | 'Ț' | 'ț' => 't',
        'Ù' | 'Ú' | 'Û' | 'Ü' | 'ù' | 'ú' | 'û' | 'ü' | 'Ũ' | 'ũ' | 'Ū' | 'ū' | 'Ŭ' | 'ŭ' | 'Ů'
        | 'ů' | 'Ű' | 'ű' | 'Ų' | 'ų' | 'Ư' | 'ư' | 'Ǔ' | 'ǔ' | 'Ǖ' | 'ǖ' | 'Ǘ' | 'ǘ' | 'Ǚ'
        | 'ǚ' | 'Ǜ' | 'ǜ' | 'Ȕ' | 'ȕ' | 'Ȗ' | 'ȗ' => 'u',
        'Ŵ' | 'ŵ' => 'w',
        'Ý' | 'ý' | 'ÿ' | 'Ŷ' | 'ŷ' | 'Ÿ' | 'Ȳ' | 'ȳ' => 'y',
        'Ź' | 'ź' | 'Ż' | 'ż' | 'Ž' | 'ž' => 'z',
        _ => return None,
    };
    Some(base)
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
    /// is unused against every identifier already issued or reserved.
    pub(crate) fn assign_native(&mut self, base: String) -> String {
        let base = if base.is_empty() {
            "section".to_owned()
        } else {
            base
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

    #[test]
    fn ascii_fold_keeps_base_letters_and_drops_the_rest() {
        assert_eq!(fold_to_ascii("Café Münch"), "Cafe Munch");
        // A letter with no single-ASCII base is dropped, not transliterated.
        assert_eq!(fold_to_ascii("Straße"), "Strae");
        // A non-Latin script leaves nothing behind.
        assert_eq!(fold_to_ascii("Λόγος"), "");
    }
}
