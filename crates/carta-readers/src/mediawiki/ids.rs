//! Heading identifier construction.

use carta_ast::{Inline, slug_gfm, to_plain_text};
use carta_core::Extension;

use crate::emoji;
use crate::transliterate::transliterate_ascii;

use super::Parser;

impl Parser {
    pub(super) fn make_id(&mut self, inlines: &[Inline]) -> String {
        let plain = to_plain_text(inlines);
        if self.extensions.contains(Extension::GfmAutoIdentifiers) {
            let base = self.finish_id(slug_gfm, &emoji_to_aliases(&plain));
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

/// Under the `gfm_auto_identifiers` scheme each emoji that has a known shortname contributes that
/// name to the identifier in place of the raw character. Spans of text with no emoji pass through
/// unchanged; the shortname is spliced in directly, without inserting word boundaries.
fn emoji_to_aliases(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        if let Some((alias, len)) = emoji::alias_at(rest) {
            out.push_str(alias);
            rest = rest.get(len..).unwrap_or("");
        } else if let Some(ch) = rest.chars().next() {
            out.push(ch);
            rest = rest.get(ch.len_utf8()..).unwrap_or("");
        } else {
            break;
        }
    }
    out
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
