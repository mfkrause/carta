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
        '\u{1e00}' | '\u{1e01}' | '\u{1ea0}' | '\u{1ea1}' | '\u{1ea2}' | '\u{1ea3}'
        | '\u{1ea4}' | '\u{1ea5}' | '\u{1ea6}' | '\u{1ea7}' | '\u{1ea8}' | '\u{1ea9}'
        | '\u{1eaa}' | '\u{1eab}' | '\u{1eac}' | '\u{1ead}' | '\u{1eae}' | '\u{1eaf}'
        | '\u{1eb0}' | '\u{1eb1}' | '\u{1eb2}' | '\u{1eb3}' | '\u{1eb4}' | '\u{1eb5}'
        | '\u{1eb6}' | '\u{1eb7}' => 'a',
        '\u{1e02}' | '\u{1e03}' | '\u{1e04}' | '\u{1e05}' | '\u{1e06}' | '\u{1e07}' => 'b',
        '\u{1e08}' | '\u{1e09}' => 'c',
        '\u{1e0a}' | '\u{1e0b}' | '\u{1e0c}' | '\u{1e0d}' | '\u{1e0e}' | '\u{1e0f}'
        | '\u{1e10}' | '\u{1e11}' | '\u{1e12}' | '\u{1e13}' => 'd',
        '\u{1e14}' | '\u{1e15}' | '\u{1e16}' | '\u{1e17}' | '\u{1e18}' | '\u{1e19}'
        | '\u{1e1a}' | '\u{1e1b}' | '\u{1e1c}' | '\u{1e1d}' | '\u{1eb8}' | '\u{1eb9}'
        | '\u{1eba}' | '\u{1ebb}' | '\u{1ebc}' | '\u{1ebd}' | '\u{1ebe}' | '\u{1ebf}'
        | '\u{1ec0}' | '\u{1ec1}' | '\u{1ec2}' | '\u{1ec3}' | '\u{1ec4}' | '\u{1ec5}'
        | '\u{1ec6}' | '\u{1ec7}' => 'e',
        '\u{1e1e}' | '\u{1e1f}' => 'f',
        '\u{1e20}' | '\u{1e21}' => 'g',
        '\u{1e22}' | '\u{1e23}' | '\u{1e24}' | '\u{1e25}' | '\u{1e26}' | '\u{1e27}'
        | '\u{1e28}' | '\u{1e29}' | '\u{1e2a}' | '\u{1e2b}' | '\u{1e96}' => 'h',
        '\u{1e2c}' | '\u{1e2d}' | '\u{1e2e}' | '\u{1e2f}' | '\u{1ec8}' | '\u{1ec9}'
        | '\u{1eca}' | '\u{1ecb}' => 'i',
        '\u{1e30}' | '\u{1e31}' | '\u{1e32}' | '\u{1e33}' | '\u{1e34}' | '\u{1e35}' => 'k',
        '\u{1e36}' | '\u{1e37}' | '\u{1e38}' | '\u{1e39}' | '\u{1e3a}' | '\u{1e3b}'
        | '\u{1e3c}' | '\u{1e3d}' => 'l',
        '\u{1e3e}' | '\u{1e3f}' | '\u{1e40}' | '\u{1e41}' | '\u{1e42}' | '\u{1e43}' => 'm',
        '\u{1e44}' | '\u{1e45}' | '\u{1e46}' | '\u{1e47}' | '\u{1e48}' | '\u{1e49}'
        | '\u{1e4a}' | '\u{1e4b}' => 'n',
        '\u{1e4c}' | '\u{1e4d}' | '\u{1e4e}' | '\u{1e4f}' | '\u{1e50}' | '\u{1e51}'
        | '\u{1e52}' | '\u{1e53}' | '\u{1ecc}' | '\u{1ecd}' | '\u{1ece}' | '\u{1ecf}'
        | '\u{1ed0}' | '\u{1ed1}' | '\u{1ed2}' | '\u{1ed3}' | '\u{1ed4}' | '\u{1ed5}'
        | '\u{1ed6}' | '\u{1ed7}' | '\u{1ed8}' | '\u{1ed9}' | '\u{1eda}' | '\u{1edb}'
        | '\u{1edc}' | '\u{1edd}' | '\u{1ede}' | '\u{1edf}' | '\u{1ee0}' | '\u{1ee1}'
        | '\u{1ee2}' | '\u{1ee3}' => 'o',
        '\u{1e54}' | '\u{1e55}' | '\u{1e56}' | '\u{1e57}' => 'p',
        '\u{1e58}' | '\u{1e59}' | '\u{1e5a}' | '\u{1e5b}' | '\u{1e5c}' | '\u{1e5d}'
        | '\u{1e5e}' | '\u{1e5f}' => 'r',
        '\u{1e60}' | '\u{1e61}' | '\u{1e62}' | '\u{1e63}' | '\u{1e64}' | '\u{1e65}'
        | '\u{1e66}' | '\u{1e67}' | '\u{1e68}' | '\u{1e69}' => 's',
        '\u{1e6a}' | '\u{1e6b}' | '\u{1e6c}' | '\u{1e6d}' | '\u{1e6e}' | '\u{1e6f}'
        | '\u{1e70}' | '\u{1e71}' | '\u{1e97}' => 't',
        '\u{1e72}' | '\u{1e73}' | '\u{1e74}' | '\u{1e75}' | '\u{1e76}' | '\u{1e77}'
        | '\u{1e78}' | '\u{1e79}' | '\u{1e7a}' | '\u{1e7b}' | '\u{1ee4}' | '\u{1ee5}'
        | '\u{1ee6}' | '\u{1ee7}' | '\u{1ee8}' | '\u{1ee9}' | '\u{1eea}' | '\u{1eeb}'
        | '\u{1eec}' | '\u{1eed}' | '\u{1eee}' | '\u{1eef}' | '\u{1ef0}' | '\u{1ef1}' => 'u',
        '\u{1e7c}' | '\u{1e7d}' | '\u{1e7e}' | '\u{1e7f}' => 'v',
        '\u{1e80}' | '\u{1e81}' | '\u{1e82}' | '\u{1e83}' | '\u{1e84}' | '\u{1e85}'
        | '\u{1e86}' | '\u{1e87}' | '\u{1e88}' | '\u{1e89}' | '\u{1e98}' => 'w',
        '\u{1e8a}' | '\u{1e8b}' | '\u{1e8c}' | '\u{1e8d}' => 'x',
        '\u{1e8e}' | '\u{1e8f}' | '\u{1e99}' | '\u{1ef2}' | '\u{1ef3}' | '\u{1ef4}'
        | '\u{1ef5}' | '\u{1ef6}' | '\u{1ef7}' | '\u{1ef8}' | '\u{1ef9}' => 'y',
        '\u{1e90}' | '\u{1e91}' | '\u{1e92}' | '\u{1e93}' | '\u{1e94}' | '\u{1e95}' => 'z',
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
        // Latin Extended Additional letters fold to their base; an accented letter folds to the
        // lowercase base (bare ASCII keeps its case), so the dotted capitals here become h and z.
        assert_eq!(fold_to_ascii("Việt Ḩ Ẓ"), "Viet h z");
        // A letter in that block with no single-ASCII base is dropped.
        assert_eq!(fold_to_ascii("ẞ"), "");
    }
}
