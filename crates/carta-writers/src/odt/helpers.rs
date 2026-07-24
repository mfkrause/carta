//! Small shared predicates and structural helpers for the ODT writer.

use carta_ast::{Attr, Block, Format, Inline, MathType, Row, Table};

/// Whether an inline sequence carries no visible content, so a flowing paragraph of it is dropped.
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
    leads_with_plain(items.first().map(Vec::as_slice).unwrap_or_default())
}

/// Whether a block sequence begins with a `Plain`, the marker distinguishing a tight list from a
/// loose one. An empty sequence counts as tight.
pub(super) fn leads_with_plain(blocks: &[Block]) -> bool {
    blocks
        .first()
        .is_none_or(|block| matches!(block, Block::Plain(_)))
}

/// Gathers the inline runs a caption contributes, in document order, descending through wrapper
/// blocks so a caption written as a `Div` still surfaces its text.
pub(super) fn collect_caption_runs<'a>(blocks: &'a [Block], runs: &mut Vec<&'a [Inline]>) {
    for block in blocks {
        match block {
            Block::Para(inlines) | Block::Plain(inlines) => runs.push(inlines),
            Block::Div(_, children) | Block::BlockQuote(children) => {
                collect_caption_runs(children, runs);
            }
            _ => {}
        }
    }
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

/// The spreadsheet-style column label for a zero-based index (`A`, `B`, …, `Z`, `AA`, …).
#[allow(clippy::cast_possible_truncation)]
pub(super) fn column_letter(mut index: usize) -> String {
    let mut label = String::new();
    loop {
        let remainder = index % 26;
        label.insert(0, char::from(b'A' + remainder as u8));
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    label
}

/// Whether a raw format targets this writer.
pub(super) fn is_opendocument(format: &Format) -> bool {
    let name = format.0.as_str();
    name.eq_ignore_ascii_case("opendocument") || name.eq_ignore_ascii_case("odt")
}

/// The byte offset at which a link target takes a `../` step toward the package root, or `None` when
/// the target resolves without one. A relative reference resolves against the directory that holds
/// the content part, so its path component gains one `../`; the step is spliced in front of the path,
/// after any `//authority`, leaving scheme, authority, query, and fragment untouched. The step is
/// withheld from a target with a URI scheme (already absolute), from one whose path component is
/// empty (a bare query or fragment addresses the document itself), and from one carrying a character
/// no URI reference admits (a non-ASCII letter, a control byte, a stray backslash, a space, a
/// bracket, or a malformed percent escape), since that cannot be resolved as a path.
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

/// Whether `url` is a well-formed relative reference that a `../` prefix can resolve.
///
/// Every byte must be admissible in the URI grammar: ASCII within `0x21..=0x7E`, none of the
/// characters the grammar excludes (space, `"`, `<`, `>`, `[`, `\`, `]`, `^`, `` ` ``, `{`, `|`,
/// `}`), and every `%` the start of a two-digit hexadecimal escape. The brackets `[` and `]` are
/// admitted only within a `//authority`, where they delimit an IP-literal host. The first path
/// segment (the run before the first `/`, `?`, or `#`) additionally admits no colon, since a colon
/// there would parse as a scheme delimiter rather than as part of the path.
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
    fn column_letter_counts_in_base_26() {
        assert_eq!(column_letter(0), "A");
        assert_eq!(column_letter(25), "Z");
        assert_eq!(column_letter(26), "AA");
        assert_eq!(column_letter(27), "AB");
        assert_eq!(column_letter(51), "AZ");
        assert_eq!(column_letter(701), "ZZ");
        assert_eq!(column_letter(702), "AAA");
    }

    #[test]
    fn parent_prefix_index_marks_relative_paths() {
        assert_eq!(parent_prefix_index("images/logo.png"), Some(0));
        // The step is spliced in after a `//authority`.
        assert_eq!(parent_prefix_index("//host/images/logo.png"), Some(6));
        // Brackets are admitted inside the authority as an IP-literal host.
        assert_eq!(parent_prefix_index("//[::1]/img.png"), Some(7));
    }

    #[test]
    fn parent_prefix_index_declines_absolute_or_document_targets() {
        assert_eq!(parent_prefix_index("https://example.com/a"), None);
        assert_eq!(parent_prefix_index("#section"), None);
        assert_eq!(parent_prefix_index("?q=1"), None);
    }

    #[test]
    fn parent_prefix_index_declines_malformed_references() {
        // A space is inadmissible in a URI reference.
        assert_eq!(parent_prefix_index("a file.png"), None);
        // A truncated percent escape is malformed.
        assert_eq!(parent_prefix_index("a%2.png"), None);
        // A colon in the first path segment would parse as a scheme delimiter.
        assert_eq!(parent_prefix_index("a_b:c/d.png"), None);
    }

    #[test]
    fn is_opendocument_matches_writer_targets() {
        assert!(is_opendocument(&Format("opendocument".into())));
        assert!(is_opendocument(&Format("ODT".into())));
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
