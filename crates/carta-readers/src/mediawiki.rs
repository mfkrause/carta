//! Reader for `MediaWiki`'s wikitext markup.
//!
//! Heading identifiers follow the enabled identifier scheme: with `gfm_auto_identifiers` the GitHub
//! algorithm (hyphen separators), otherwise `auto_identifiers` lowercases the text, keeps
//! alphanumerics together with `_` and `.`, turns spaces and `-` into single `_`, and drops a
//! leading run of non-letters; duplicates gain a numeric suffix and an empty result becomes
//! `section`. With neither enabled, headings carry no identifier.
//!
//! The scanner is panic-free on malformed input: unbalanced or unterminated constructs degrade to
//! literal text rather than being rejected.

use std::collections::BTreeMap;

use carta_ast::{ApiVersion, Block, Document, Format, Inline, MetaValue};
use carta_core::{Extension, Extensions, Reader, ReaderOptions, Result};

use self::preprocess::{expand_tabs, extract_behavior_switches, strip_comments};
use crate::heading_ids;

mod blocks;
mod emphasis;
mod ids;
mod inline;
mod links;
mod lists;
mod preprocess;
mod tables;
mod tags;

/// Parses a wikitext document into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct MediawikiReader;

impl Reader for MediawikiReader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document> {
        let stripped = strip_comments(&expand_tabs(input));
        let (source, behavior_switches) = extract_behavior_switches(&stripped);
        let chars: Vec<char> = source.chars().collect();
        let mut parser = Parser::new(options);
        let mut blocks = parser.parse_blocks(&chars);
        // Categories are pulled from the inline flow into one trailing paragraph, in document order.
        if !parser.categories.is_empty() {
            let mut inlines: Vec<Inline> = Vec::new();
            for (index, category) in parser.categories.drain(..).enumerate() {
                if index > 0 {
                    inlines.push(Inline::Space);
                }
                inlines.push(category);
            }
            blocks.push(Block::Para(inlines));
        }
        let mut meta: BTreeMap<String, MetaValue> = BTreeMap::new();
        for switch in behavior_switches {
            meta.insert(switch, MetaValue::MetaBool(true));
        }
        Ok(Document {
            api_version: ApiVersion::default(),
            meta: meta.into_iter().map(|(k, v)| (k.into(), v)).collect(),
            blocks,
        })
    }
}

/// Carries the state that spans a whole document: the enabled extensions, the running counter for
/// unlabeled external links, and the heading identifiers already issued (for de-duplication).
struct Parser {
    extensions: Extensions,
    link_counter: usize,
    ids: heading_ids::IdRegistry,
    /// Category links pulled out of the inline flow, to be emitted as one trailing paragraph.
    categories: Vec<Inline>,
    /// Current block-nesting depth, capped to keep adversarially deep input from exhausting the stack.
    depth: usize,
}

/// Block-nesting depth past which parsing stops descending: deeply stacked blockquotes, list levels,
/// notes, and table cells degrade to flat content rather than recursing without bound. The cap sits
/// far below the point where either parsing or serialization would overflow the stack.
const MAX_BLOCK_DEPTH: usize = 64;

/// A lexical unit of inline text: a finished inline node, a run of apostrophes whose emphasis role is
/// resolved once the surrounding run structure is known, a block-level HTML tag that interrupts the
/// paragraph, or a paragraph break carried by a block-level tag that leaves no output.
enum Tok {
    Inline(Inline),
    Apostrophes(usize),
    BlockRaw(String),
    BlockBreak,
    /// A verbatim block element (`<pre>`, `<blockquote>`, `<syntaxhighlight>`) found mid-paragraph:
    /// it interrupts the paragraph and emerges as its own block.
    Block(Block),
}

/// The role a recognized HTML tag plays in the inline stream.
enum HtmlTagRole {
    /// An inline element: its opening and closing tags pass through as raw inline HTML.
    Inline,
    /// A block element: its tags interrupt the paragraph and pass through as raw block HTML.
    Block,
    /// A paragraph-only element (`p`, `gallery`): its tags interrupt the paragraph but leave no output.
    Break,
}

impl Parser {
    fn new(options: &ReaderOptions) -> Self {
        Self {
            extensions: options.extensions,
            link_counter: 0,
            ids: heading_ids::IdRegistry::default(),
            categories: Vec::new(),
            depth: 0,
        }
    }

    /// Whether straight double quotes should fold into typographic quote runs.
    fn smart(&self) -> bool {
        self.extensions.contains(Extension::Smart)
    }
}

/// The positions that cap how far a tag scan can succeed within a slice: `last_gt` is the index of
/// the final `>` and `last_close` the index of the final `</`. An open tag cannot complete past the
/// former and a closing tag cannot begin past the latter, so a scan starting beyond the relevant
/// bound fails without touching the rest of the input. Precomputing them once per slice turns a run
/// of unterminated tags from a rescan-to-end at every `<` into O(1)-per-tag failures.
#[derive(Clone, Copy)]
struct ScanBounds {
    last_gt: Option<usize>,
    last_close: Option<usize>,
}

impl ScanBounds {
    fn of(chars: &[char]) -> Self {
        let mut last_gt = None;
        let mut last_close = None;
        let mut i = 0;
        let n = chars.len();
        while i < n {
            match at(chars, i) {
                Some('>') => last_gt = Some(i),
                Some('<') if at(chars, i + 1) == Some('/') => last_close = Some(i),
                _ => {}
            }
            i += 1;
        }
        Self {
            last_gt,
            last_close,
        }
    }

    fn open_possible(&self, start: usize) -> bool {
        self.last_gt.is_some_and(|gt| start <= gt)
    }

    fn close_possible(&self, start: usize) -> bool {
        self.last_close.is_some_and(|close| start <= close)
    }
}

/// If an inline construct opens at `i`, the index just past it: `{{…}}`, `[[…]]`, `[…]`, or `<…>`.
fn skip_construct(chars: &[char], i: usize) -> Option<usize> {
    match at(chars, i) {
        Some('{') if at(chars, i + 1) == Some('{') => balanced_braces(chars, i),
        Some('[') if at(chars, i + 1) == Some('[') => {
            find_seq(chars, i + 2, &[']', ']']).map(|c| c + 2)
        }
        Some('[') => find_char(chars, i + 1, ']').map(|c| c + 1),
        Some('<') => find_char(chars, i, '>').map(|c| c + 1),
        _ => None,
    }
}

/// Whether the `{{` at `i` opens a template transclusion. A template name begins with a letter, a
/// digit, or a `:` (a leading-colon main-namespace reference); a `{{` followed by anything else
/// (whitespace, a parser-function `#`, a pipe, or `}}`) is literal braces, not a template.
fn template_opens(chars: &[char], i: usize) -> bool {
    matches!(at(chars, i + 2), Some(c) if c.is_alphanumeric() || c == ':')
}

/// The index just past the `}}` that balances the `{{` at `i`, accounting for nesting.
fn balanced_braces(chars: &[char], i: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut j = i;
    let n = chars.len();
    while j < n {
        if at(chars, j) == Some('{') && at(chars, j + 1) == Some('{') {
            depth += 1;
            j += 2;
        } else if at(chars, j) == Some('}') && at(chars, j + 1) == Some('}') {
            depth -= 1;
            j += 2;
            if depth == 0 {
                return Some(j);
            }
        } else {
            j += 1;
        }
    }
    None
}

fn raw_html(text: String) -> Inline {
    Inline::RawInline(Format("html".into()), text.into())
}

fn format_mediawiki() -> Format {
    Format("mediawiki".into())
}

fn format_html() -> Format {
    Format("html".into())
}

fn at(chars: &[char], i: usize) -> Option<char> {
    chars.get(i).copied()
}

fn collect_range(chars: &[char], start: usize, end: usize) -> String {
    if end <= start {
        return String::new();
    }
    chars.iter().skip(start).take(end - start).collect()
}

fn line_end(chars: &[char], pos: usize) -> usize {
    find_char(chars, pos, '\n').unwrap_or(chars.len())
}

fn is_blank(chars: &[char], start: usize, end: usize) -> bool {
    (start..end).all(|j| at(chars, j).is_none_or(char::is_whitespace))
}

fn find_char(chars: &[char], from: usize, target: char) -> Option<usize> {
    (from..chars.len()).find(|&j| at(chars, j) == Some(target))
}

fn find_seq(chars: &[char], from: usize, seq: &[char]) -> Option<usize> {
    let n = chars.len();
    let m = seq.len();
    if m == 0 || n < m {
        return None;
    }
    (from..=n - m).find(|&j| (0..m).all(|k| at(chars, j + k) == seq.get(k).copied()))
}

fn matches_prefix_ci(chars: &[char], i: usize, prefix: &str) -> bool {
    prefix
        .chars()
        .enumerate()
        .all(|(k, pc)| match at(chars, i + k) {
            Some(c) => c.eq_ignore_ascii_case(&pc),
            None => false,
        })
}

#[cfg(test)]
mod tests;
