//! Auto-generated header identifiers.
//!
//! When an auto-identifier toggle is on, a header that carries no explicit identifier receives one
//! derived from its text. Two algorithms are supported and differ in which characters survive,
//! whitespace handling, leading-character stripping, lowercasing, the empty-result fallback, and
//! how repeated slugs are disambiguated:
//!
//! - `gfm_auto_identifiers` — full-Unicode lowercasing; keep alphanumerics, `_`, and `-`; turn each
//!   whitespace character into a single `-`; drop everything else (including `.`); no leading strip;
//!   an empty result stays empty. Repeats are disambiguated by a per-base occurrence count, which
//!   can itself produce a collision.
//! - `auto_identifiers` — keep alphanumerics, `_`, `-`, `.`, and whitespace; collapse each
//!   whitespace run to a single `-`; simple per-character lowercasing; strip the leading run up to
//!   the first letter; an empty result becomes `section`. Repeats increment a numeric suffix until
//!   the whole identifier is unused, and explicit identifiers are reserved against that set.
//!
//! See `docs/PORTING.md` for the derivation. Both read from the header's stringified inline text.

use std::collections::{BTreeMap, BTreeSet};

use carta_ast::{Block, Inline, Table};
use carta_core::{Extension, Extensions};

/// Fill in empty header identifiers across the document in reading order.
pub(crate) fn assign_header_identifiers(blocks: &mut [Block], ext: Extensions) {
    let algorithm = if ext.contains(Extension::GfmAutoIdentifiers) {
        Algorithm::Gfm
    } else if ext.contains(Extension::AutoIdentifiers) {
        Algorithm::Markdown
    } else {
        return;
    };
    let mut registry = Registry::default();
    walk(blocks, algorithm, &mut registry);
}

#[derive(Clone, Copy)]
enum Algorithm {
    Markdown,
    Gfm,
}

/// Tracks identifiers already in use so repeats can be disambiguated.
#[derive(Default)]
struct Registry {
    /// Every identifier emitted or reserved, used by the increment-until-unique strategy.
    seen: BTreeSet<String>,
    /// Per-base occurrence counts, used by the count-suffix strategy.
    counts: BTreeMap<String, u32>,
}

impl Registry {
    fn assign(&mut self, algorithm: Algorithm, base: String) -> String {
        match algorithm {
            Algorithm::Markdown => {
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
            Algorithm::Gfm => {
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

    fn reserve(&mut self, algorithm: Algorithm, id: &str) {
        if let Algorithm::Markdown = algorithm {
            self.seen.insert(id.to_owned());
        }
    }
}

fn walk(blocks: &mut [Block], algorithm: Algorithm, registry: &mut Registry) {
    for block in blocks {
        match block {
            Block::Header(_, attr, inlines) => {
                if attr.id.is_empty() {
                    let base = slug(algorithm, &stringify(inlines));
                    attr.id = registry.assign(algorithm, base);
                } else {
                    registry.reserve(algorithm, &attr.id);
                }
            }
            Block::BlockQuote(children)
            | Block::Div(_, children)
            | Block::Figure(_, _, children) => walk(children, algorithm, registry),
            Block::BulletList(items) | Block::OrderedList(_, items) => {
                for item in items {
                    walk(item, algorithm, registry);
                }
            }
            Block::DefinitionList(items) => {
                for (_, definitions) in items {
                    for definition in definitions {
                        walk(definition, algorithm, registry);
                    }
                }
            }
            Block::Table(table) => walk_table(table, algorithm, registry),
            _ => {}
        }
    }
}

fn walk_table(table: &mut Table, algorithm: Algorithm, registry: &mut Registry) {
    let body_rows = table
        .bodies
        .iter_mut()
        .flat_map(|body| body.head.iter_mut().chain(body.body.iter_mut()));
    let rows = table
        .head
        .rows
        .iter_mut()
        .chain(body_rows)
        .chain(table.foot.rows.iter_mut());
    for row in rows {
        for cell in &mut row.cells {
            walk(&mut cell.content, algorithm, registry);
        }
    }
}

/// Plain-text stringification of an inline list, as the slugifier sees it.
fn stringify(inlines: &[Inline]) -> String {
    let mut out = String::new();
    push_stringified(inlines, &mut out);
    out
}

fn push_stringified(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Str(text) | Inline::Code(_, text) | Inline::Math(_, text) => out.push_str(text),
            Inline::Space | Inline::SoftBreak | Inline::LineBreak => out.push(' '),
            Inline::Emph(children)
            | Inline::Underline(children)
            | Inline::Strong(children)
            | Inline::Strikeout(children)
            | Inline::Superscript(children)
            | Inline::Subscript(children)
            | Inline::SmallCaps(children)
            | Inline::Quoted(_, children)
            | Inline::Cite(_, children)
            | Inline::Link(_, children, _)
            | Inline::Image(_, children, _)
            | Inline::Span(_, children) => push_stringified(children, out),
            Inline::RawInline(..) | Inline::Note(_) => {}
        }
    }
}

fn slug(algorithm: Algorithm, text: &str) -> String {
    match algorithm {
        Algorithm::Markdown => slug_markdown(text),
        Algorithm::Gfm => slug_gfm(text),
    }
}

fn slug_markdown(text: &str) -> String {
    let kept: String = text
        .chars()
        .filter_map(|c| {
            if c.is_alphanumeric() || matches!(c, '_' | '-' | '.') {
                Some(c)
            } else if c.is_whitespace() {
                Some(' ')
            } else {
                None
            }
        })
        .collect();
    let hyphenated = kept.split_whitespace().collect::<Vec<_>>().join("-");
    let lowered: String = hyphenated
        .chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect();
    let trimmed: String = lowered.chars().skip_while(|c| !c.is_alphabetic()).collect();
    if trimmed.is_empty() {
        "section".to_owned()
    } else {
        trimmed
    }
}

fn slug_gfm(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .filter_map(|c| {
            if c.is_alphanumeric() || matches!(c, '_' | '-') {
                Some(c)
            } else if c.is_whitespace() {
                Some('-')
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{slug_gfm, slug_markdown};

    #[test]
    fn markdown_keeps_dots_and_strips_the_leading_non_letter_run() {
        assert_eq!(slug_markdown("Hello, World!"), "hello-world");
        assert_eq!(slug_markdown("1.2 Section A.B"), "section-a.b");
        assert_eq!(slug_markdown("a___b---c...d"), "a___b---c...d");
        assert_eq!(slug_markdown("9lives"), "lives");
    }

    #[test]
    fn markdown_collapses_punctuation_gaps_but_keeps_literal_hyphens() {
        assert_eq!(slug_markdown("Foo & Bar"), "foo-bar");
        assert_eq!(slug_markdown("a - b"), "a---b");
    }

    #[test]
    fn markdown_falls_back_to_section_for_empty_results() {
        assert_eq!(slug_markdown("!!! ???"), "section");
        assert_eq!(slug_markdown(""), "section");
    }

    #[test]
    fn gfm_drops_dots_keeps_leading_digits_and_does_not_collapse() {
        assert_eq!(slug_gfm("Hello, World!"), "hello-world");
        assert_eq!(slug_gfm("1.2 Section A.B"), "12-section-ab");
        assert_eq!(slug_gfm("Foo & Bar"), "foo--bar");
        assert_eq!(slug_gfm("a - b"), "a---b");
    }

    #[test]
    fn gfm_yields_an_empty_or_hyphen_string_with_no_fallback() {
        assert_eq!(slug_gfm("!!!"), "");
        assert_eq!(slug_gfm("!!! ???"), "-");
    }
}
