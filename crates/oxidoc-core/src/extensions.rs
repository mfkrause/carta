//! Format extensions: the set of optional syntax features a reader or writer may honor.
//!
//! [`Extension`] is one named feature (matching pandoc's documented extension identifiers, an
//! observable contract); [`Extensions`] is a deterministic, allocation-free set of them backed by a
//! fixed array of 64-bit words. The set carries no 128-variant ceiling, so it scales to pandoc's
//! full extension count. [`presets`] holds the per-flavor sets; strict `CommonMark` is the empty set.

/// Generates the [`Extension`] enum together with the `ALL`/`COUNT`/`name` metadata, keeping the
/// variant list as the single source of truth for the bitset sizing in [`Extensions`].
macro_rules! define_extensions {
    ($($variant:ident => $name:literal),+ $(,)?) => {
        /// A single format extension. Each variant's position in [`Extension::ALL`] is its bit
        /// index in [`Extensions`].
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub enum Extension { $($variant),+ }

        impl Extension {
            /// Every extension, in declaration order.
            pub const ALL: &'static [Extension] = &[$(Extension::$variant),+];
            /// The number of distinct extensions.
            pub const COUNT: usize = Self::ALL.len();

            /// The extension's identifier (e.g. `"footnotes"`).
            #[must_use]
            pub const fn name(self) -> &'static str {
                match self { $(Extension::$variant => $name),+ }
            }
        }
    };
}

define_extensions! {
    Smart => "smart",
    Strikeout => "strikeout",
    Superscript => "superscript",
    Subscript => "subscript",
    PipeTables => "pipe_tables",
    Footnotes => "footnotes",
    TaskLists => "task_lists",
    Autolink => "autolink_bare_uris",
    TexMathDollars => "tex_math_dollars",
    FencedDivs => "fenced_divs",
    BracketedSpans => "bracketed_spans",
}

const WORD_BITS: usize = u64::BITS as usize;
const WORDS: usize = Extension::COUNT.div_ceil(WORD_BITS);

/// A deterministic, allocation-free set of [`Extension`]s, backed by a fixed array of 64-bit words
/// indexed by each variant's position in [`Extension::ALL`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Extensions([u64; WORDS]);

impl Default for Extensions {
    fn default() -> Self {
        Self::empty()
    }
}

impl Extensions {
    /// The empty set (strict `CommonMark`).
    #[must_use]
    pub const fn empty() -> Self {
        Self([0; WORDS])
    }

    /// The set containing exactly `list`. Const so presets are `const` values.
    #[must_use]
    // Const indexing: `bit < Extension::COUNT` (variant discriminants are `0..COUNT`), so
    // `bit / WORD_BITS < WORDS`; `i < list.len()`. Both indices are provably in bounds, and
    // slice `get` is not usable across all const contexts on the pinned toolchain.
    #[allow(clippy::indexing_slicing)]
    pub const fn from_list(list: &[Extension]) -> Self {
        let mut words = [0u64; WORDS];
        let mut i = 0;
        while i < list.len() {
            let bit = list[i] as usize;
            words[bit / WORD_BITS] |= 1u64 << (bit % WORD_BITS);
            i += 1;
        }
        Self(words)
    }

    /// Whether `ext` is in the set.
    #[must_use]
    pub fn contains(self, ext: Extension) -> bool {
        let bit = ext as usize;
        self.0
            .get(bit / WORD_BITS)
            .is_some_and(|word| (word >> (bit % WORD_BITS)) & 1 == 1)
    }

    /// Adds `ext` to the set.
    pub fn insert(&mut self, ext: Extension) {
        let bit = ext as usize;
        if let Some(word) = self.0.get_mut(bit / WORD_BITS) {
            *word |= 1u64 << (bit % WORD_BITS);
        }
    }

    /// Removes `ext` from the set.
    pub fn remove(&mut self, ext: Extension) {
        let bit = ext as usize;
        if let Some(word) = self.0.get_mut(bit / WORD_BITS) {
            *word &= !(1u64 << (bit % WORD_BITS));
        }
    }

    /// Whether the set is empty.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.0.iter().all(|&word| word == 0)
    }

    /// The set's extensions in [`Extension::ALL`] (deterministic) order.
    pub fn iter(self) -> impl Iterator<Item = Extension> {
        Extension::ALL
            .iter()
            .copied()
            .filter(move |&ext| self.contains(ext))
    }
}

impl core::fmt::Debug for Extensions {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_set()
            .entries(self.iter().map(Extension::name))
            .finish()
    }
}

/// Per-flavor extension sets.
pub mod presets {
    use super::{Extension, Extensions};

    /// Strict `CommonMark`: no extensions.
    pub const COMMONMARK: Extensions = Extensions::empty();

    /// `GitHub`-Flavored Markdown. A documented target for a future reader; no consumer yet.
    pub const GFM: Extensions = Extensions::from_list(&[
        Extension::Strikeout,
        Extension::PipeTables,
        Extension::TaskLists,
        Extension::Autolink,
    ]);
}

#[cfg(test)]
mod tests {
    use super::{Extension, Extensions, presets};

    #[test]
    fn words_cover_every_variant() {
        // Every variant's bit index must land inside the backing array.
        for ext in Extension::ALL {
            assert!((*ext as usize) / super::WORD_BITS < super::WORDS);
        }
    }

    #[test]
    fn insert_remove_contains_round_trip() {
        let mut set = Extensions::empty();
        assert!(set.is_empty());
        assert!(!set.contains(Extension::Footnotes));
        set.insert(Extension::Footnotes);
        assert!(set.contains(Extension::Footnotes));
        assert!(!set.is_empty());
        set.remove(Extension::Footnotes);
        assert!(!set.contains(Extension::Footnotes));
        assert!(set.is_empty());
    }

    #[test]
    fn from_list_and_iter_follow_declaration_order() {
        let set = Extensions::from_list(&[Extension::PipeTables, Extension::Smart]);
        let collected: Vec<Extension> = set.iter().collect();
        // `iter` yields in `ALL` order, regardless of `from_list` argument order.
        assert_eq!(collected, vec![Extension::Smart, Extension::PipeTables]);
    }

    #[test]
    fn commonmark_preset_is_empty_gfm_is_not() {
        assert!(presets::COMMONMARK.is_empty());
        assert!(presets::GFM.contains(Extension::Strikeout));
        assert!(presets::GFM.contains(Extension::TaskLists));
        assert!(!presets::GFM.contains(Extension::Footnotes));
    }

    #[test]
    fn names_are_stable() {
        assert_eq!(Extension::Footnotes.name(), "footnotes");
        assert_eq!(Extension::Autolink.name(), "autolink_bare_uris");
    }
}
