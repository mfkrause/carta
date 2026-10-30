//! Heading identifier construction.

use carta_ast::{Inline, slug_gfm, to_plain_text};
use carta_core::Extension;

use crate::emoji;
use crate::transliterate::transliterate_ascii;

use super::Parser;

impl Parser<'_> {
    pub(super) fn make_id(&mut self, inlines: &[Inline]) -> String {
        let plain = to_plain_text(inlines);
        if self.extensions.contains(Extension::GfmAutoIdentifiers) {
            let base = self.finish_id(slug_gfm, &emoji::to_aliases(&plain));
            self.ids.assign_with_separator(base, '-')
        } else if self.extensions.contains(Extension::AutoIdentifiers) {
            let base = self.finish_id(mediawiki_slug, &plain);
            self.ids.assign_with_separator(base, '_')
        } else {
            String::new()
        }
    }

    /// Builds an identifier with `slug`, then, when `ascii_identifiers` is on, folds the finished
    /// slug to pure ASCII (accents stripped, non-Latin letters dropped) and re-slugs it, so a dropped
    /// letter leaves its separators intact while a now-leading separator is trimmed. An empty result
    /// is mapped to a placeholder during disambiguation.
    fn finish_id(&self, slug: fn(&str) -> String, source: &str) -> String {
        let mut base = slug(source);
        if self.extensions.contains(Extension::AsciiIdentifiers) {
            base = slug(&transliterate_ascii(&base));
        }
        base
    }
}

/// Builds a heading identifier under the `auto_identifiers` scheme: lowercase, keep alphanumerics
/// with `_` and `.`, collapse each whitespace run to a single `_`, turn each hyphen into its own
/// `_`, drop other punctuation without breaking an adjacent whitespace run, and strip a leading run
/// of non-letters.
fn mediawiki_slug(text: &str) -> String {
    let mut out = String::new();
    let mut in_ws = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !in_ws {
                out.push('_');
                in_ws = true;
            }
        } else if ch == '-' {
            out.push('_');
            in_ws = false;
        } else if ch.is_alphanumeric() || ch == '_' || ch == '.' {
            out.extend(ch.to_lowercase());
            in_ws = false;
        }
        // Other punctuation is transparent: emits nothing, leaves the whitespace collapse intact, so `Foo : Bar` yields one `_`.
    }
    out.chars().skip_while(|c| !c.is_alphabetic()).collect()
}
