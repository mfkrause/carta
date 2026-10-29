//! Small shared predicates and structural helpers for the ODT writer.

use std::borrow::Cow;

use carta_ast::{Attr, Block, Format, Inline, MathType, Row, Table};
use carta_core::container::xml::is_xml_char;

use super::{STACK_RED_ZONE, STACK_SEGMENT};

/// Whether a paragraph segment beside display math has no visible content.
pub(super) fn inlines_are_empty(inlines: &[Inline]) -> bool {
    inlines.iter().all(|inline| match inline {
        Inline::Space | Inline::SoftBreak => true,
        Inline::RawInline(format, _) => !is_opendocument(format),
        _ => false,
    })
}

/// Whether an inline is a display formula, which occupies a paragraph of its own.
pub(super) fn is_block_math(inline: &Inline) -> bool {
    matches!(inline, Inline::Math(MathType::DisplayMath, _))
}

/// The inline slice with any leading and trailing inter-word spacing (spaces and soft breaks) removed,
/// so text lifted out from beside a display formula does not carry the gap that abutted the formula.
pub(super) fn trim_flanking_spacing(inlines: &[Inline]) -> &[Inline] {
    let is_spacing = |inline: &&Inline| matches!(inline, Inline::Space | Inline::SoftBreak);
    let start = inlines.iter().take_while(is_spacing).count();
    let trailing = inlines.iter().rev().take_while(is_spacing).count();
    let end = inlines.len().saturating_sub(trailing);
    inlines.get(start..end).unwrap_or_default()
}

/// Whether a list renders tight (compact) rather than loose. A list is tight when the first block of
/// its first item is plain, flowing text; a leading paragraph or a leading block structure makes the
/// whole list loose.
pub(super) fn is_tight(items: &[Vec<Block>]) -> bool {
    matches!(
        items.first().and_then(|item| item.first()),
        Some(Block::Plain(_))
    )
}

/// Gathers caption runs while retaining inline formatting and paragraph boundaries.
pub(super) fn collect_caption_runs<'a>(blocks: &'a [Block], runs: &mut Vec<Cow<'a, [Inline]>>) {
    stacker::maybe_grow(STACK_RED_ZONE, STACK_SEGMENT, || {
        for block in blocks {
            match block {
                Block::Para(inlines) | Block::Plain(inlines) | Block::Header(_, _, inlines) => {
                    runs.push(Cow::Borrowed(inlines));
                }
                Block::LineBlock(lines) => {
                    runs.extend(lines.iter().map(|line| Cow::Borrowed(line.as_slice())));
                }
                Block::RawBlock(format, text) => {
                    runs.push(Cow::Owned(vec![Inline::RawInline(
                        format.clone(),
                        text.clone(),
                    )]));
                }
                Block::CodeBlock(attr, text) => {
                    runs.push(Cow::Owned(vec![Inline::Code(attr.clone(), text.clone())]));
                }
                Block::Div(_, children) | Block::BlockQuote(children) => {
                    collect_caption_runs(children, runs);
                }
                Block::BulletList(items) | Block::OrderedList(_, items) => {
                    runs.push(Cow::Owned(
                        items
                            .iter()
                            .flat_map(|item| caption_inlines(item))
                            .collect(),
                    ));
                }
                Block::DefinitionList(items) => {
                    let mut inlines = Vec::new();
                    for (term, definitions) in items {
                        inlines.extend(term.iter().cloned());
                        inlines.push(Inline::Str(": ".into()));
                        for definition in definitions {
                            inlines.extend(caption_inlines(definition));
                        }
                    }
                    runs.push(Cow::Owned(inlines));
                }
                _ => {}
            }
        }
    });
}

pub(super) fn caption_inlines(blocks: &[Block]) -> Vec<Inline> {
    let mut runs = Vec::new();
    collect_caption_runs(blocks, &mut runs);
    let mut inlines = Vec::new();
    for (index, run) in runs.into_iter().enumerate() {
        if index > 0 {
            inlines.push(Inline::LineBreak);
        }
        inlines.extend(run.into_owned());
    }
    inlines
}

/// The number of columns a table has: its column specs, or the widest row when it declares none.
pub(super) fn table_column_count(table: &Table) -> usize {
    if !table.col_specs.is_empty() {
        return table.col_specs.len();
    }
    let row_width = |rows: &[Row]| rows.iter().map(|row| row.cells.len()).max().unwrap_or(0);
    let mut columns = row_width(&table.head.rows).max(row_width(&table.foot.rows));
    for section in &table.bodies {
        columns = columns
            .max(row_width(&section.head))
            .max(row_width(&section.body));
    }
    columns
}

/// The column label at the character offset from A.
/// Invalid XML characters get unique numeric names.
pub(super) fn column_letter(index: usize) -> String {
    u32::try_from(index)
        .ok()
        .and_then(|column| column.checked_add(u32::from(b'A')))
        .and_then(char::from_u32)
        .filter(|character| is_xml_char(*character))
        .map_or_else(|| format!("Column{index}"), |column| column.to_string())
}

/// Whether a raw format targets this writer.
pub(super) fn is_opendocument(format: &Format) -> bool {
    format.0.eq_ignore_ascii_case("opendocument")
}

/// Return the insertion offset for a relative target's package-root `../` step.
///
/// Insert after any authority, before the path. Targets with schemes, empty paths,
/// or invalid URI syntax return `None`; queries and fragments remain unchanged.
pub(super) fn parent_prefix_index(url: &str) -> Option<usize> {
    if has_scheme(url) || !is_relative_reference(url) {
        return None;
    }
    let path_start = authority_end(url);
    let reference = url.split(['?', '#']).next().unwrap_or_default();
    (reference.len() > path_start).then_some(path_start)
}

/// The byte offset just past a `//authority`, or `0` when the reference has none. The authority runs
/// from the opening `//` to the first `/`, `?`, or `#`, or to the end of the reference.
fn authority_end(url: &str) -> usize {
    match url.strip_prefix("//") {
        Some(rest) => rest
            .find(['/', '?', '#'])
            .map_or(url.len(), |offset| 2 + offset),
        None => 0,
    }
}

/// Check whether `url` is a valid relative URI reference.
///
/// Require URI-admissible ASCII and valid percent escapes. Brackets are permitted only
/// in an authority, and a colon in the first path segment would instead name a scheme.
fn is_relative_reference(url: &str) -> bool {
    let authority = authority_end(url);
    let bytes = url.as_bytes();
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'%' => {
                if !bytes.get(index + 1).is_some_and(u8::is_ascii_hexdigit)
                    || !bytes.get(index + 2).is_some_and(u8::is_ascii_hexdigit)
                {
                    return false;
                }
                index += 3;
            }
            b'[' | b']' if (2..authority).contains(&index) => index += 1,
            b' ' | b'"' | b'<' | b'>' | b'[' | b'\\' | b']' | b'^' | b'`' | b'{' | b'|' | b'}' => {
                return false;
            }
            0x21..=0x7E => index += 1,
            _ => return false,
        }
    }
    let first_segment = url.split(['/', '?', '#']).next().unwrap_or_default();
    !first_segment.contains(':')
}

/// Whether a URL opens with a `scheme:` prefix: a non-empty run of scheme characters before a colon.
fn has_scheme(url: &str) -> bool {
    match url.split_once(':') {
        Some((scheme, _)) => {
            !scheme.is_empty()
                && scheme
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '+'))
        }
        None => false,
    }
}

pub(super) fn attr_value<'a>(attr: &'a Attr, key: &str) -> Option<&'a str> {
    attr.attributes
        .iter()
        .find(|(name, _)| name.as_str() == key)
        .map(|(_, value)| value.as_str())
}

pub(super) fn custom_style(attr: &Attr) -> Option<&str> {
    attr_value(attr, "custom-style").filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_labels_follow_character_offsets() {
        assert_eq!(column_letter(0), "A");
        assert_eq!(column_letter(25), "Z");
        assert_eq!(column_letter(26), "[");
        assert_eq!(column_letter(27), "\\");
        assert_eq!(column_letter(51), "t");
        assert_eq!(column_letter(701), "\u{2fe}");
        assert_eq!(column_letter(702), "\u{2ff}");
        assert_ne!(column_letter(0xd7bf), column_letter(0xd7c0));
    }

    #[test]
    fn parent_prefix_index_marks_relative_paths() {
        assert_eq!(parent_prefix_index("images/logo.png"), Some(0));
        assert_eq!(parent_prefix_index("//host/images/logo.png"), Some(6));
        assert_eq!(parent_prefix_index("//[::1]/img.png"), Some(7));
    }

    #[test]
    fn parent_prefix_index_declines_absolute_or_document_targets() {
        assert_eq!(parent_prefix_index(""), None);
        assert_eq!(parent_prefix_index("https://example.com/a"), None);
        assert_eq!(parent_prefix_index("#section"), None);
        assert_eq!(parent_prefix_index("?q=1"), None);
    }

    #[test]
    fn parent_prefix_index_declines_malformed_references() {
        assert_eq!(parent_prefix_index("a file.png"), None);
        assert_eq!(parent_prefix_index("a%2.png"), None);
        assert_eq!(parent_prefix_index("a_b:c/d.png"), None);
    }

    #[test]
    fn raw_format_requires_opendocument() {
        assert!(is_opendocument(&Format("opendocument".into())));
        assert!(is_opendocument(&Format("OPENDOCUMENT".into())));
        assert!(!is_opendocument(&Format("odt".into())));
        assert!(!is_opendocument(&Format("ODT".into())));
        assert!(!is_opendocument(&Format("html".into())));
    }

    #[test]
    fn attr_value_and_custom_style_read_named_attributes() {
        let attr = Attr {
            attributes: vec![
                ("width".into(), "50%".into()),
                ("custom-style".into(), "Quote".into()),
            ],
            ..Attr::default()
        };
        assert_eq!(attr_value(&attr, "width"), Some("50%"));
        assert_eq!(attr_value(&attr, "missing"), None);
        assert_eq!(custom_style(&attr), Some("Quote"));

        let empty = Attr {
            attributes: vec![("custom-style".into(), "".into())],
            ..Attr::default()
        };
        assert_eq!(custom_style(&empty), None);
    }
}
