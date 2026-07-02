//! Format extensions: the set of optional syntax features a reader or writer may honor.
//!
//! [`Extension`] is one named feature; [`Extensions`] is a deterministic, allocation-free set of them
//! backed by a fixed array of 64-bit words. The set carries no 128-variant ceiling, so it scales to
//! the full extension set. [`presets`] holds the per-flavor sets; strict `CommonMark` is the empty set.

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

            /// The extension named `name`, or `None` if no extension uses that identifier.
            #[must_use]
            pub fn from_name(name: &str) -> Option<Extension> {
                match name { $($name => Some(Extension::$variant),)+ _ => None }
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
    HardLineBreaks => "hard_line_breaks",
    RawHtml => "raw_html",
    // Attribute-bearing syntax: `{#id .class key=val}` on headers, code, links/images, and spans.
    HeaderAttributes => "header_attributes",
    FencedCodeAttributes => "fenced_code_attributes",
    InlineCodeAttributes => "inline_code_attributes",
    LinkAttributes => "link_attributes",
    // The combined attribute toggle (enables the attribute syntaxes as a group).
    Attributes => "attributes",
    // Block constructs.
    DefinitionLists => "definition_lists",
    GridTables => "grid_tables",
    MultilineTables => "multiline_tables",
    SimpleTables => "simple_tables",
    TableCaptions => "table_captions",
    LineBlocks => "line_blocks",
    // List-marker richness.
    FancyLists => "fancy_lists",
    ExampleLists => "example_lists",
    Startnum => "startnum",
    // Document metadata.
    YamlMetadataBlock => "yaml_metadata_block",
    PandocTitleBlock => "pandoc_title_block",
    // Header identifiers and the references they enable.
    AutoIdentifiers => "auto_identifiers",
    GfmAutoIdentifiers => "gfm_auto_identifiers",
    // Fold a derived identifier down to ASCII, dropping diacritics before the slug is formed.
    AsciiIdentifiers => "ascii_identifiers",
    // A header's explicit identifier is written in MultiMarkdown's trailing `[id]` form rather than
    // the `{#id}` attribute block.
    MmdHeaderIdentifiers => "mmd_header_identifiers",
    ImplicitHeaderReferences => "implicit_header_references",
    // Bare images with a caption become figures.
    ImplicitFigures => "implicit_figures",
    // Raw passthrough: `` `code`{=fmt} `` inline and ```` ```{=fmt} ```` fenced blocks.
    RawAttribute => "raw_attribute",
    // A `^[…]` inline note expands to a footnote in place.
    InlineNotes => "inline_notes",
    // Block-level `<div>`/inline `<span>` HTML become `Div`/`Span`, with markdown parsed inside.
    NativeDivs => "native_divs",
    NativeSpans => "native_spans",
    // Markdown is parsed inside block-level HTML, which is otherwise split tag-by-tag.
    MarkdownInHtmlBlocks => "markdown_in_html_blocks",
    // A `<div>`/`<span>` emitted for a div/span carries a `data-markdown="1"` marker so its contents
    // are still parsed as Markdown; this also forces a div with no native syntax into an HTML wrap.
    MarkdownAttribute => "markdown_attribute",
    // Inline raw TeX (`\command{…}`, `\begin{env}…\end{env}`) passes through verbatim.
    RawTex => "raw_tex",
    // `[@key]` / `@key` citation references.
    Citations => "citations",
    // An attribute block on a table's caption line attaches to the table.
    TableAttributes => "table_attributes",
    // A blank line is required before a blockquote / header, so neither interrupts a paragraph.
    BlankBeforeBlockquote => "blank_before_blockquote",
    BlankBeforeHeader => "blank_before_header",
    // `==text==` highlight spans.
    Mark => "mark",
    // `:name:` emoji shortcodes.
    Emoji => "emoji",
    // `> [!NOTE]`-style admonition blockquotes become classed divs.
    Alerts => "alerts",
    // Single- and double-backslash math delimiters: `\(…\)`/`\[…\]` and `\\(…\\)`/`\\[…\\]`.
    TexMathSingleBackslash => "tex_math_single_backslash",
    TexMathDoubleBackslash => "tex_math_double_backslash",
    // Code-block surface: backtick-fenced ```` ``` ```` and tilde-fenced `~~~` blocks. When neither is
    // available the writer drops to the four-space indented form.
    FencedCodeBlocks => "fenced_code_blocks",
    BacktickCodeBlocks => "backtick_code_blocks",
    // GitHub math surface: inline `` $`…`$ `` and a ```` ```math ```` display block, as opposed to the
    // `$…$`/`$$…$$` dollar form.
    TexMathGfm => "tex_math_gfm",
    // A backslash at a line's end is a hard line break, written as a trailing `\`; without it the
    // writer falls back to two trailing spaces.
    EscapedLineBreaks => "escaped_line_breaks",
    // An underscore inside a word opens no emphasis, so the writer leaves intra-word `_` literal;
    // without it every `_` is escaped so the strict reader cannot start emphasis mid-word.
    IntrawordUnderscores => "intraword_underscores",
    // A list may begin directly after a paragraph line with no intervening blank line, interrupting
    // it; without it a list marker on the line after a paragraph folds into that paragraph.
    ListsWithoutPrecedingBlankline => "lists_without_preceding_blankline",
    // `*[SHY]: Soft hyphen` abbreviation definitions, applied to later occurrences of the term.
    Abbreviations => "abbreviations",
    // A backslash escapes any symbol, not only the ASCII-punctuation subset.
    AllSymbolsEscapable => "all_symbols_escapable",
    // A backslash before `<` or `>` escapes the angle bracket.
    AngleBracketsEscapable => "angle_brackets_escapable",
    // Line breaks between East Asian wide characters carry no width and are dropped.
    EastAsianLineBreaks => "east_asian_line_breaks",
    // An indented code block requires four spaces of indentation rather than one tab stop.
    FourSpaceRule => "four_space_rule",
    // Typographic conventions of the Project Gutenberg style for plain-text output.
    Gutenberg => "gutenberg",
    // Soft line breaks within a paragraph are discarded rather than kept as spaces.
    IgnoreLineBreaks => "ignore_line_breaks",
    // User-defined LaTeX macros are expanded in math and raw TeX.
    LatexMacros => "latex_macros",
    // Bird-track (`> `) literate-program code sections.
    LiterateHaskell => "literate_haskell",
    // An attribute block following a link or image in the MultiMarkdown position.
    MmdLinkAttributes => "mmd_link_attributes",
    // A MultiMarkdown metadata block at the top of the document.
    MmdTitleBlock => "mmd_title_block",
    // `-` and `--` map to en/em dashes under the older dash convention.
    OldDashes => "old_dashes",
    // A raw block or inline may be written directly as Markdown for round-tripping.
    RawMarkdown => "raw_markdown",
    // Relative paths in links and images are rebased onto the source file's location.
    RebaseRelativePaths => "rebase_relative_paths",
    // `~x` / `^x` subscript and superscript bind only the single following character.
    ShortSubsuperscripts => "short_subsuperscripts",
    // A defined label may be referenced by `[label]` alone, with no following `[]` or `(…)`.
    ShortcutReferenceLinks => "shortcut_reference_links",
    // An ATX header requires a space between the opening `#` run and the heading text.
    SpaceInAtxHeader => "space_in_atx_header",
    // A reference link's label and its following `[id]` may be separated by whitespace.
    SpacedReferenceLinks => "spaced_reference_links",
    // `[[target|title]]` wiki links, with the title following the pipe.
    WikilinksTitleAfterPipe => "wikilinks_title_after_pipe",
    // `[[title|target]]` wiki links, with the title preceding the pipe.
    WikilinksTitleBeforePipe => "wikilinks_title_before_pipe",
}

const WORD_BITS: usize = u64::BITS as usize;
const WORDS: usize = Extension::COUNT.div_ceil(WORD_BITS);

// The bitset indexing in `from_list` is sound only while each variant's discriminant equals its
// position in `ALL` (so every `ext as usize` lands in `0..COUNT`). The macro emits no explicit
// discriminants, so this holds — asserted at compile time here, turning a future edit that breaks
// contiguity into a build failure rather than an out-of-bounds index.
#[allow(clippy::indexing_slicing)]
const _: () = {
    let mut i = 0;
    while i < Extension::ALL.len() {
        assert!(Extension::ALL[i] as usize == i);
        i += 1;
    }
};

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
    // Const indexing: contiguity (asserted above) gives `bit < COUNT`, so `bit / WORD_BITS < WORDS`;
    // `i < list.len()`. Both indices are in bounds, and slice `get` is not usable across all const
    // contexts on the pinned toolchain.
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

    /// The union of this set and `other`.
    #[must_use]
    pub fn union(self, other: Extensions) -> Extensions {
        let mut words = self.0;
        for (word, &add) in words.iter_mut().zip(other.0.iter()) {
            *word |= add;
        }
        Extensions(words)
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

    /// `GitHub`-Flavored Markdown.
    pub const GFM: Extensions = Extensions::from_list(&[
        Extension::Strikeout,
        Extension::PipeTables,
        Extension::BacktickCodeBlocks,
        Extension::TaskLists,
        Extension::Autolink,
        Extension::Footnotes,
        Extension::TexMathDollars,
        Extension::TexMathGfm,
        Extension::GfmAutoIdentifiers,
        Extension::RawHtml,
        Extension::Emoji,
        Extension::Alerts,
    ]);

    /// The `commonmark_x` dialect: `CommonMark` with a broad set of inline and block extensions
    /// enabled. `backtick_code_blocks` is additionally carried because the shared Markdown engine
    /// fences code on that flag, which `CommonMark` does natively.
    pub const COMMONMARK_X: Extensions = Extensions::from_list(&[
        Extension::Smart,
        Extension::Strikeout,
        Extension::Superscript,
        Extension::Subscript,
        Extension::PipeTables,
        Extension::Footnotes,
        Extension::TaskLists,
        Extension::TexMathDollars,
        Extension::FencedDivs,
        Extension::BracketedSpans,
        Extension::BacktickCodeBlocks,
        Extension::RawHtml,
        Extension::RawAttribute,
        Extension::Attributes,
        Extension::HeaderAttributes,
        Extension::FencedCodeAttributes,
        Extension::InlineCodeAttributes,
        Extension::LinkAttributes,
        Extension::DefinitionLists,
        Extension::FancyLists,
        Extension::GfmAutoIdentifiers,
        Extension::ImplicitHeaderReferences,
        Extension::Emoji,
        Extension::Alerts,
    ]);

    /// The extended Markdown dialect: the broad default extension set.
    pub const MARKDOWN: Extensions = Extensions::from_list(&[
        Extension::Smart,
        Extension::Strikeout,
        Extension::Superscript,
        Extension::Subscript,
        Extension::PipeTables,
        Extension::Footnotes,
        Extension::TaskLists,
        Extension::TexMathDollars,
        Extension::FencedDivs,
        Extension::BracketedSpans,
        Extension::RawHtml,
        Extension::HeaderAttributes,
        Extension::FencedCodeAttributes,
        Extension::FencedCodeBlocks,
        Extension::BacktickCodeBlocks,
        Extension::InlineCodeAttributes,
        Extension::LinkAttributes,
        Extension::DefinitionLists,
        Extension::GridTables,
        Extension::MultilineTables,
        Extension::SimpleTables,
        Extension::TableCaptions,
        Extension::LineBlocks,
        Extension::FancyLists,
        Extension::ExampleLists,
        Extension::Startnum,
        Extension::YamlMetadataBlock,
        Extension::PandocTitleBlock,
        Extension::AutoIdentifiers,
        Extension::ImplicitHeaderReferences,
        Extension::ImplicitFigures,
        Extension::RawAttribute,
        Extension::InlineNotes,
        Extension::NativeDivs,
        Extension::NativeSpans,
        Extension::MarkdownInHtmlBlocks,
        Extension::RawTex,
        Extension::Citations,
        Extension::TableAttributes,
        Extension::BlankBeforeBlockquote,
        Extension::BlankBeforeHeader,
        Extension::EscapedLineBreaks,
        Extension::IntrawordUnderscores,
        Extension::SpaceInAtxHeader,
    ]);

    /// The legacy GitHub Markdown dialect (`markdown_github`). The set is restricted to the
    /// variants that exist and affect writer output: backtick-fenced code, pipe tables, strikeout,
    /// task lists, footnotes, autolinking, emoji, and alerts, but no smart typography, math, spans,
    /// or fenced divs.
    pub const MARKDOWN_GITHUB: Extensions = Extensions::from_list(&[
        Extension::Strikeout,
        Extension::PipeTables,
        Extension::Footnotes,
        Extension::TaskLists,
        Extension::Autolink,
        Extension::RawHtml,
        Extension::FencedCodeBlocks,
        Extension::BacktickCodeBlocks,
        Extension::AutoIdentifiers,
        Extension::GfmAutoIdentifiers,
        Extension::Emoji,
        Extension::Alerts,
        Extension::IntrawordUnderscores,
    ]);

    /// The PHP Markdown Extra dialect (`markdown_phpextra`). The set is restricted to the variants
    /// that exist and affect writer output: definition lists, fenced (tilde) code blocks, footnotes,
    /// header and link attributes, pipe tables, and raw HTML. It has no backtick code fences, so code
    /// fences are written with tildes, and no smart typography, math, strikeout, spans, or fenced divs.
    pub const MARKDOWN_PHPEXTRA: Extensions = Extensions::from_list(&[
        Extension::DefinitionLists,
        Extension::FencedCodeBlocks,
        Extension::Footnotes,
        Extension::HeaderAttributes,
        Extension::IntrawordUnderscores,
        Extension::LinkAttributes,
        Extension::MarkdownAttribute,
        Extension::PipeTables,
        Extension::RawHtml,
    ]);

    /// The `MultiMarkdown` dialect (`markdown_mmd`). The set is restricted to the variants that
    /// exist and affect writer output: backtick-fenced code, definition lists, footnotes, pipe
    /// tables, implicit figures and header references, sub/superscript, dollar math, raw HTML and raw
    /// attributes, auto identifiers, `MultiMarkdown`'s trailing `[id]` header identifiers, and the
    /// `data-markdown`
    /// div marker. It has no header attribute blocks, strikeout, task lists, smart typography, spans,
    /// or fenced divs. With `tex_math_dollars` on and taking precedence, a `tex_math_double_backslash`
    /// surface would not change this dialect's writer output, so it is left out of the preset and math
    /// is emitted as `$…$`.
    pub const MARKDOWN_MMD: Extensions = Extensions::from_list(&[
        Extension::AutoIdentifiers,
        Extension::BacktickCodeBlocks,
        Extension::DefinitionLists,
        Extension::Footnotes,
        Extension::ImplicitFigures,
        Extension::ImplicitHeaderReferences,
        Extension::IntrawordUnderscores,
        Extension::MarkdownAttribute,
        Extension::MmdHeaderIdentifiers,
        Extension::PipeTables,
        Extension::RawAttribute,
        Extension::RawHtml,
        Extension::Subscript,
        Extension::Superscript,
        Extension::TexMathDollars,
    ]);

    /// The original Markdown dialect (`markdown_strict`). The set is restricted to the variants that
    /// exist and affect writer output — only raw HTML. With no fenced or backtick code, tables,
    /// definition lists,
    /// footnotes, task lists, math, or any attribute syntax, every richer construct falls back to
    /// indented code, an HTML block, or a raw glyph. Lacking `intraword_underscores`, every `_` is
    /// escaped; lacking `pipe_tables`, a literal `|` is left unescaped.
    pub const MARKDOWN_STRICT: Extensions = Extensions::from_list(&[Extension::RawHtml]);

    // The reader default sets below are broader than the writer presets above: a reader enables every
    // construct the dialect can parse, whereas the writer presets carry only the extensions that shape
    // the emitted text. Some entries name constructs the shared Markdown engine does not yet branch on;
    // they are recorded so the dialect's default surface is complete and takes effect once modeled.

    /// Reader defaults for the original Markdown dialect (`markdown_strict`): only raw HTML, plus the
    /// shortcut and spaced reference-link forms.
    pub const MARKDOWN_STRICT_READ: Extensions = Extensions::from_list(&[
        Extension::RawHtml,
        Extension::ShortcutReferenceLinks,
        Extension::SpacedReferenceLinks,
    ]);

    /// Reader defaults for the GitHub Markdown dialect (`markdown_github`): the GitHub construct set —
    /// strikeout, task lists, pipe tables, footnotes, bare-URI autolinking, emoji, alerts, backtick and
    /// fenced code, auto identifiers in both forms, intra-word underscores, lists that open without a
    /// preceding blank line, and the escaping/heading-spacing leniencies.
    pub const MARKDOWN_GITHUB_READ: Extensions = Extensions::from_list(&[
        Extension::Alerts,
        Extension::AllSymbolsEscapable,
        Extension::AutoIdentifiers,
        Extension::Autolink,
        Extension::BacktickCodeBlocks,
        Extension::Emoji,
        Extension::FencedCodeBlocks,
        Extension::Footnotes,
        Extension::GfmAutoIdentifiers,
        Extension::IntrawordUnderscores,
        Extension::ListsWithoutPrecedingBlankline,
        Extension::PipeTables,
        Extension::RawHtml,
        Extension::ShortcutReferenceLinks,
        Extension::SpaceInAtxHeader,
        Extension::Strikeout,
        Extension::TaskLists,
    ]);

    /// Reader defaults for the PHP Markdown Extra dialect (`markdown_phpextra`): abbreviations,
    /// definition lists, fenced code, footnotes, header and link attributes, intra-word underscores,
    /// the `data-markdown` div marker, pipe tables, raw HTML, and the reference-link forms.
    pub const MARKDOWN_PHPEXTRA_READ: Extensions = Extensions::from_list(&[
        Extension::Abbreviations,
        Extension::DefinitionLists,
        Extension::FencedCodeBlocks,
        Extension::Footnotes,
        Extension::HeaderAttributes,
        Extension::IntrawordUnderscores,
        Extension::LinkAttributes,
        Extension::MarkdownAttribute,
        Extension::PipeTables,
        Extension::RawHtml,
        Extension::ShortcutReferenceLinks,
        Extension::SpacedReferenceLinks,
    ]);

    /// Reader defaults for the `MultiMarkdown` dialect (`markdown_mmd`): auto identifiers, backtick
    /// code, definition lists, footnotes, implicit figures and header references, intra-word
    /// underscores, the `data-markdown` div marker, `MultiMarkdown`'s trailing `[id]` header
    /// identifiers, its link-attribute and title-block forms, pipe tables, raw HTML and raw attributes,
    /// single-character sub/superscripts, the reference-link forms, sub/superscript spans, dollar math,
    /// and the double-backslash math delimiters.
    pub const MARKDOWN_MMD_READ: Extensions = Extensions::from_list(&[
        Extension::AllSymbolsEscapable,
        Extension::AutoIdentifiers,
        Extension::BacktickCodeBlocks,
        Extension::DefinitionLists,
        Extension::Footnotes,
        Extension::ImplicitFigures,
        Extension::ImplicitHeaderReferences,
        Extension::IntrawordUnderscores,
        Extension::MarkdownAttribute,
        Extension::MmdHeaderIdentifiers,
        Extension::MmdLinkAttributes,
        Extension::MmdTitleBlock,
        Extension::PipeTables,
        Extension::RawAttribute,
        Extension::RawHtml,
        Extension::ShortSubsuperscripts,
        Extension::ShortcutReferenceLinks,
        Extension::SpacedReferenceLinks,
        Extension::Subscript,
        Extension::Superscript,
        Extension::TexMathDollars,
        Extension::TexMathDoubleBackslash,
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
        assert!(presets::GFM.contains(Extension::PipeTables));
        // GFM has no subscript/superscript; those belong to the broader Markdown dialects.
        assert!(!presets::GFM.contains(Extension::Subscript));
        assert!(!presets::GFM.contains(Extension::Superscript));
    }

    #[test]
    fn markdown_and_commonmark_x_presets_are_broad() {
        assert!(presets::MARKDOWN.contains(Extension::DefinitionLists));
        assert!(presets::MARKDOWN.contains(Extension::YamlMetadataBlock));
        assert!(presets::MARKDOWN.contains(Extension::Smart));
        assert!(presets::COMMONMARK_X.contains(Extension::FencedDivs));
        assert!(presets::COMMONMARK_X.contains(Extension::Attributes));
        // The strict CommonMark dialect keeps none of these.
        assert!(presets::COMMONMARK.is_empty());
    }

    #[test]
    fn code_and_math_surface_variants_round_trip_and_seed_presets() {
        for token in ["fenced_code_blocks", "backtick_code_blocks", "tex_math_gfm"] {
            let ext = Extension::from_name(token).expect("a declared variant");
            assert_eq!(ext.name(), token);
        }
        // The Markdown dialect fences code with both backtick and tilde forms.
        assert!(presets::MARKDOWN.contains(Extension::FencedCodeBlocks));
        assert!(presets::MARKDOWN.contains(Extension::BacktickCodeBlocks));
        // GFM fences with backticks and renders math in its own surface; it has no tilde-fence form.
        assert!(presets::GFM.contains(Extension::BacktickCodeBlocks));
        assert!(presets::GFM.contains(Extension::TexMathGfm));
        assert!(!presets::GFM.contains(Extension::FencedCodeBlocks));
    }

    #[test]
    fn names_are_stable() {
        assert_eq!(Extension::Footnotes.name(), "footnotes");
        assert_eq!(Extension::Autolink.name(), "autolink_bare_uris");
        assert_eq!(Extension::HardLineBreaks.name(), "hard_line_breaks");
        assert_eq!(Extension::RawHtml.name(), "raw_html");
    }

    #[test]
    fn from_name_round_trips_every_variant() {
        for ext in Extension::ALL {
            assert_eq!(Extension::from_name(ext.name()), Some(*ext));
        }
        assert_eq!(Extension::from_name("not_an_extension"), None);
        assert_eq!(Extension::from_name(""), None);
    }

    #[test]
    fn union_combines_both_sides() {
        let a = Extensions::from_list(&[Extension::Strikeout]);
        let b = Extensions::from_list(&[Extension::Subscript]);
        let combined = a.union(b);
        assert!(combined.contains(Extension::Strikeout));
        assert!(combined.contains(Extension::Subscript));
        assert!(!combined.contains(Extension::Superscript));
        assert_eq!(a.union(Extensions::empty()), a);
    }
}
