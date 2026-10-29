//! Metadata, manifest, and formula-part serialization for the ODT writer.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use carta_ast::{Block, Caption, Inline, MetaValue, Text};
use carta_core::container::xml::{escape_attribute, escape_text_with_quotes};

use super::{DATE_EPOCH, DECL, Formula, Image, MIMETYPE, meta_keywords};
use crate::common::metadata_text;

/// The `meta.xml` part: the producer, the document's title and creators, and reproducible
/// timestamps.
pub(super) fn meta_xml(meta: &BTreeMap<Text, MetaValue>) -> String {
    let stamp = iso_utc(DATE_EPOCH);
    let escaped = |key: &str| {
        let mut out = String::new();
        escape_text_with_quotes(&meta_text(meta, key), &mut out);
        out
    };
    let title = escaped("title");
    let description = escaped("description");
    let subject = escaped("subject");
    let mut keywords = String::new();
    escape_text_with_quotes(&meta_keywords(meta), &mut keywords);
    let language = escaped("lang");
    let creator = escaped("author");

    let mut out = String::new();
    out.push_str(DECL);
    out.push_str(concat!(
        "<office:document-meta xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" ",
        "xmlns:xlink=\"http://www.w3.org/1999/xlink\" ",
        "xmlns:dc=\"http://purl.org/dc/elements/1.1/\" ",
        "xmlns:meta=\"urn:oasis:names:tc:opendocument:xmlns:meta:1.0\" ",
        "xmlns:ooo=\"http://openoffice.org/2004/office\" ",
        "xmlns:grddl=\"http://www.w3.org/2003/g/data-view#\" office:version=\"1.3\">\n  <office:meta>\n",
    ));
    let content_start = out.len();
    out.push_str("<meta:generator>carta</meta:generator>\n");
    let _ = writeln!(out, "<dc:title>{title}</dc:title>");
    let _ = writeln!(out, "<dc:description>{description}</dc:description>");
    let _ = writeln!(out, "<dc:subject>{subject}</dc:subject>");
    let _ = writeln!(out, "<meta:keyword>{keywords}</meta:keyword>");
    if !language.is_empty() {
        let _ = writeln!(out, "<dc:language>{language}</dc:language>");
    }
    let _ = writeln!(
        out,
        "<meta:initial-creator>{creator}</meta:initial-creator>"
    );
    let _ = writeln!(out, "<dc:creator>{creator}</dc:creator>");
    let _ = writeln!(out, "<meta:creation-date>{stamp}</meta:creation-date>");
    let _ = writeln!(out, "<dc:date>{stamp}</dc:date>");
    for (key, value) in meta {
        if is_standard_meta(key.as_str()) {
            continue;
        }
        let mut name = String::new();
        escape_attribute(key.as_str(), &mut name);
        let mut text = String::new();
        escape_text_with_quotes(&user_defined_value(value), &mut text);
        let _ = writeln!(
            out,
            "<meta:user-defined meta:name=\"{name}\" meta:value-type=\"string\">{text}\
             </meta:user-defined>"
        );
    }
    let content =
        crate::common::indent_block(out.get(content_start..).unwrap_or_default(), "    ", "    ");
    out.truncate(content_start);
    out.push_str(&content);
    out.push_str("  </office:meta>\n</office:document-meta>");
    out
}

/// Whether a metadata key is carried by a dedicated `meta.xml` element rather than a custom property.
fn is_standard_meta(key: &str) -> bool {
    matches!(
        key,
        "title" | "description" | "subject" | "keywords" | "lang" | "author"
    )
}

/// Flattens a custom metadata field to the plain text of a user-defined property: scalar fields
/// yield their text, a boolean its capitalized name, and structured fields (lists, maps) yield
/// nothing.
fn user_defined_value(value: &MetaValue) -> String {
    match value {
        MetaValue::MetaString(text) => text.to_string(),
        MetaValue::MetaInlines(inlines) => metadata_text(inlines),
        MetaValue::MetaBlocks(blocks) => blocks_text(blocks),
        MetaValue::MetaBool(flag) => if *flag { "True" } else { "False" }.to_string(),
        MetaValue::MetaList(_) | MetaValue::MetaMap(_) => String::new(),
    }
}

/// The `Formula-N/content.xml` part wrapping a Presentation MathML document.
pub(super) fn formula_content_xml(mathml: &str) -> String {
    let mut out = String::with_capacity(mathml.len() + DECL.len());
    out.push_str("<?xml version='1.0' ?>\n");
    if let Some(root) =
        mathml.strip_prefix("<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"")
        && let Some((display, body)) = root.split_once('"')
    {
        let body = if body == "></math>" {
            "><mrow></mrow></math>"
        } else {
            body
        };
        let _ = write!(
            out,
            "<math display=\"{display}\" xmlns=\"http://www.w3.org/1998/Math/MathML\"{body}"
        );
    } else {
        out.push_str(mathml);
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// The `META-INF/manifest.xml` part listing every package member.
pub(super) fn manifest_xml(images: &[Image], formulas: &[Formula]) -> String {
    let mut out = String::new();
    out.push_str(DECL);
    out.push_str(
        "<manifest:manifest xmlns:manifest=\"urn:oasis:names:tc:opendocument:xmlns:manifest:1.0\" \
         manifest:version=\"1.3\">\n",
    );
    let _ = writeln!(
        out,
        "  <manifest:file-entry manifest:media-type=\"{MIMETYPE}\" manifest:full-path=\"/\" manifest:version=\"1.3\" />"
    );
    let mut entry = |path: &str, media_type: &str| {
        out.push_str("  <manifest:file-entry manifest:media-type=\"");
        escape_attribute(media_type, &mut out);
        out.push_str("\" manifest:full-path=\"");
        escape_attribute(path, &mut out);
        out.push_str("\" />\n");
    };
    entry("content.xml", "application/xml");
    let mut objects = Vec::new();
    for image in images.iter().filter(|image| !image.mime.is_empty()) {
        objects.push((
            image.index,
            format!("Pictures/{}", image.file_name),
            image.mime.as_str(),
        ));
    }
    for formula in formulas {
        for part in ["settings.xml", "content.xml"] {
            objects.push((
                formula.index,
                format!("Formula-{}/{part}", formula.index),
                "application/xml",
            ));
        }
    }
    objects.sort_by_key(|(index, _, _)| std::cmp::Reverse(*index));
    for (_, path, media_type) in objects {
        entry(&path, media_type);
    }
    for (path, media_type) in [
        ("manifest.rdf", "application/rdf+xml"),
        ("styles.xml", "application/xml"),
        ("meta.xml", "application/xml"),
    ] {
        entry(path, media_type);
    }
    for formula in formulas.iter().rev() {
        entry(
            &format!("Formula-{}/", formula.index),
            "application/vnd.oasis.opendocument.formula",
        );
    }
    out.push_str("</manifest:manifest>");
    out
}

/// Flattens a metadata field to plain text, joining a list with `; `.
pub(super) fn meta_text(meta: &BTreeMap<Text, MetaValue>, key: &str) -> String {
    meta.get(key).map(plain_meta).unwrap_or_default()
}

pub(super) fn plain_meta(value: &MetaValue) -> String {
    match value {
        MetaValue::MetaString(text) => text.to_string(),
        MetaValue::MetaInlines(inlines) => metadata_text(inlines),
        MetaValue::MetaBlocks(blocks) => blocks_text(blocks),
        MetaValue::MetaBool(flag) => flag.to_string(),
        MetaValue::MetaList(items) => items.iter().map(plain_meta).collect::<Vec<_>>().join("; "),
        MetaValue::MetaMap(_) => String::new(),
    }
}

fn blocks_text(blocks: &[Block]) -> String {
    let mut output = String::new();
    push_blocks_text(blocks, &mut output);
    output
}

fn push_blocks_text(blocks: &[Block], output: &mut String) {
    carta_core::with_stack(|| {
        for block in blocks {
            match block {
                Block::Plain(inlines) | Block::Para(inlines) | Block::Header(_, _, inlines) => {
                    output.push_str(&metadata_text(inlines));
                }
                Block::LineBlock(lines) => {
                    for line in lines {
                        output.push_str(&metadata_text(line));
                    }
                }
                Block::BlockQuote(inner) | Block::Div(_, inner) => push_blocks_text(inner, output),
                Block::BulletList(items) | Block::OrderedList(_, items) => {
                    for item in items {
                        push_blocks_text(item, output);
                    }
                }
                Block::DefinitionList(items) => {
                    for (term, definitions) in items {
                        output.push_str(&metadata_text(term));
                        for definition in definitions {
                            push_blocks_text(definition, output);
                        }
                    }
                }
                Block::Figure(_, caption, inner) => {
                    push_caption_text(caption, output);
                    push_blocks_text(inner, output);
                }
                Block::Table(table) => {
                    push_caption_text(&table.caption, output);
                    let rows = table
                        .head
                        .rows
                        .iter()
                        .chain(
                            table
                                .bodies
                                .iter()
                                .flat_map(|body| body.head.iter().chain(&body.body)),
                        )
                        .chain(&table.foot.rows);
                    for row in rows {
                        for cell in &row.cells {
                            push_blocks_text(&cell.content, output);
                        }
                    }
                }
                Block::CodeBlock(..) | Block::RawBlock(..) | Block::HorizontalRule => {}
            }
        }
    });
}

fn push_caption_text(caption: &Caption, output: &mut String) {
    if let Some(short) = &caption.short {
        output.push_str(&metadata_text(short));
    }
    push_blocks_text(&caption.long, output);
}

/// Inline content for a conditional metadata field, excluding missing, empty, and false values.
pub(super) fn meta_inlines(meta: &BTreeMap<Text, MetaValue>, key: &str) -> Option<Vec<Inline>> {
    let value = meta.get(key)?;
    if matches!(value, MetaValue::MetaBool(false)) {
        return None;
    }
    let inlines = value_inlines(value);
    if inlines.is_empty() {
        None
    } else {
        Some(inlines)
    }
}

fn value_inlines(value: &MetaValue) -> Vec<Inline> {
    match value {
        MetaValue::MetaInlines(inlines) => inlines.clone(),
        MetaValue::MetaString(text) => {
            if text.is_empty() {
                Vec::new()
            } else {
                vec![Inline::Str(text.clone())]
            }
        }
        MetaValue::MetaBlocks(blocks) => carta_ast::single_block_inlines(blocks).to_vec(),
        MetaValue::MetaList(items) => {
            let mut out = Vec::new();
            for item in items {
                out.extend(value_inlines(item));
            }
            out
        }
        MetaValue::MetaBool(value) => vec![Inline::Str(value.to_string().into())],
        MetaValue::MetaMap(_) => vec![Inline::Str("true".into())],
    }
}

/// The abstract's paragraphs, one inline sequence per `Para`/`Plain` block, or a single sequence for
/// an inline-valued field.
pub(super) fn abstract_paragraphs(value: &MetaValue) -> Vec<Vec<Inline>> {
    if matches!(value, MetaValue::MetaBool(false)) {
        return Vec::new();
    }
    if let MetaValue::MetaBlocks(blocks) = value {
        return blocks
            .iter()
            .filter_map(|block| match block {
                Block::Para(inlines) | Block::Plain(inlines) => Some(inlines.clone()),
                _ => None,
            })
            .collect();
    }
    let inlines = value_inlines(value);
    if inlines.is_empty() {
        Vec::new()
    } else {
        vec![inlines]
    }
}

/// The document's authors, one inline sequence per author.
pub(super) fn meta_authors(meta: &BTreeMap<Text, MetaValue>) -> Vec<Vec<Inline>> {
    match meta.get("author") {
        Some(MetaValue::MetaList(items)) => items.iter().map(value_inlines).collect(),
        Some(value) => {
            let inlines = value_inlines(value);
            if inlines.is_empty() {
                Vec::new()
            } else {
                vec![inlines]
            }
        }
        None => Vec::new(),
    }
}

/// Formats seconds since the Unix epoch as a W3C date-time in UTC (`YYYY-MM-DDThh:mm:ssZ`).
fn iso_utc(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let seconds = epoch.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds / 3_600;
    let minute = (seconds % 3_600) / 60;
    let second = seconds % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Converts a count of days since 1970-01-01 to a `(year, month, day)` civil date.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_position + 2) / 5 + 1;
    let month = if month_position < 10 {
        month_position + 3
    } else {
        month_position - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use carta_ast::QuoteType;

    #[test]
    fn metadata_indents_multiline_values_without_removing_blank_lines() {
        let meta = BTreeMap::from([("title".into(), MetaValue::MetaString("A\n\nB\n".into()))]);
        let xml = meta_xml(&meta);
        assert!(xml.contains("<dc:title>A\n\n    B\n    </dc:title>"));
    }

    #[test]
    fn manifest_orders_mixed_objects_and_lists_formula_directories_once() {
        let images = [Image {
            index: 2,
            file_name: "2.png".into(),
            mime: "image/png".into(),
            bytes: Vec::new(),
        }];
        let formulas: Vec<_> = [0, 3]
            .into_iter()
            .map(|index| Formula {
                index,
                mathml: String::new(),
                text_mode: true,
            })
            .collect();
        let xml = manifest_xml(&images, &formulas);
        let paths: Vec<_> = xml
            .split("manifest:full-path=\"")
            .skip(1)
            .map(|tail| tail.split('"').next().unwrap())
            .collect();
        assert_eq!(
            paths,
            [
                "/",
                "content.xml",
                "Formula-3/settings.xml",
                "Formula-3/content.xml",
                "Pictures/2.png",
                "Formula-0/settings.xml",
                "Formula-0/content.xml",
                "manifest.rdf",
                "styles.xml",
                "meta.xml",
                "Formula-3/",
                "Formula-0/",
            ]
        );
        assert!(xml.contains(
            "manifest:media-type=\"application/xml\" manifest:full-path=\"Formula-3/content.xml\""
        ));
        assert!(!xml.contains("text/xml"));
    }

    #[test]
    fn formula_parts_keep_math_content_and_serialize_empty_roots() {
        let math = crate::math::to_mathml("x", false).unwrap();
        assert_eq!(
            formula_content_xml(&math),
            concat!(
                "<?xml version='1.0' ?>\n",
                "<math display=\"inline\" xmlns=\"http://www.w3.org/1998/Math/MathML\"><mi>x</mi></math>\n",
            )
        );
        let math = crate::math::to_mathml("", true).unwrap();
        assert_eq!(
            formula_content_xml(&math),
            concat!(
                "<?xml version='1.0' ?>\n",
                "<math display=\"block\" xmlns=\"http://www.w3.org/1998/Math/MathML\"><mrow></mrow></math>\n",
            )
        );
    }

    #[test]
    fn custom_metadata_keeps_nested_block_text_and_quotes() {
        let value = MetaValue::MetaBlocks(vec![
            Block::Para(vec![Inline::Str("First".into())]),
            Block::BlockQuote(vec![Block::Para(vec![Inline::Quoted(
                QuoteType::DoubleQuote,
                vec![Inline::Str("Second".into())],
            )])]),
        ]);
        assert_eq!(user_defined_value(&value), "First“Second”");
    }

    #[test]
    fn author_values_keep_text_that_conditional_fields_hide() {
        let mut meta = BTreeMap::new();
        meta.insert("author".into(), MetaValue::MetaMap(BTreeMap::new()));
        assert_eq!(meta_authors(&meta), vec![vec![Inline::Str("true".into())]]);
        meta.insert("author".into(), MetaValue::MetaBool(false));
        assert_eq!(meta_authors(&meta), vec![vec![Inline::Str("false".into())]]);
        meta.insert("title".into(), MetaValue::MetaBool(false));
        assert!(meta_inlines(&meta, "title").is_none());
        assert!(abstract_paragraphs(&MetaValue::MetaBool(false)).is_empty());
    }
}
