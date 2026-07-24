//! Metadata, manifest, and formula-part serialization for the ODT writer.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use carta_ast::{Block, Inline, MetaValue, Text};
use carta_core::container::xml::{escape_attribute, escape_text};

use super::{DATE_EPOCH, DECL, Formula, Image, MIMETYPE, NS, meta_keywords};

/// The `meta.xml` part: the producer, the document's title and creators, and reproducible
/// timestamps.
pub(super) fn meta_xml(meta: &BTreeMap<Text, MetaValue>) -> String {
    let stamp = iso_utc(DATE_EPOCH);
    let escaped = |key: &str| {
        let mut out = String::new();
        escape_text(&meta_text(meta, key), &mut out);
        out
    };
    let title = escaped("title");
    let description = escaped("description");
    let subject = escaped("subject");
    let mut keywords = String::new();
    escape_text(&meta_keywords(meta), &mut keywords);
    let language = escaped("lang");
    let creator = escaped("author");

    let mut out = String::new();
    out.push_str(DECL);
    out.push_str("<office:document-meta");
    out.push_str(NS);
    out.push_str(" office:version=\"1.3\"><office:meta>");
    out.push_str("<meta:generator>carta</meta:generator>");
    let _ = write!(out, "<dc:title>{title}</dc:title>");
    let _ = write!(out, "<dc:description>{description}</dc:description>");
    let _ = write!(out, "<dc:subject>{subject}</dc:subject>");
    let _ = write!(out, "<meta:keyword>{keywords}</meta:keyword>");
    if !language.is_empty() {
        let _ = write!(out, "<dc:language>{language}</dc:language>");
    }
    let _ = write!(
        out,
        "<meta:initial-creator>{creator}</meta:initial-creator>"
    );
    let _ = write!(out, "<dc:creator>{creator}</dc:creator>");
    let _ = write!(out, "<meta:creation-date>{stamp}</meta:creation-date>");
    let _ = write!(out, "<dc:date>{stamp}</dc:date>");
    // Unclaimed fields become custom properties keyed by name; map order keeps them sorted.
    for (key, value) in meta {
        if is_standard_meta(key.as_str()) {
            continue;
        }
        let mut name = String::new();
        escape_attribute(key.as_str(), &mut name);
        let mut text = String::new();
        escape_text(&user_defined_value(value), &mut text);
        let _ = write!(
            out,
            "<meta:user-defined meta:name=\"{name}\" meta:value-type=\"string\">{text}\
             </meta:user-defined>"
        );
    }
    out.push_str("</office:meta></office:document-meta>");
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
        MetaValue::MetaInlines(inlines) => carta_ast::to_plain_text(inlines),
        MetaValue::MetaBlocks(blocks) => {
            carta_ast::to_plain_text(carta_ast::single_block_inlines(blocks))
        }
        MetaValue::MetaBool(flag) => if *flag { "True" } else { "False" }.to_string(),
        MetaValue::MetaList(_) | MetaValue::MetaMap(_) => String::new(),
    }
}

/// The `Formula-N/content.xml` part wrapping a Presentation MathML document.
pub(super) fn formula_content_xml(mathml: &str) -> String {
    let mut out = String::with_capacity(mathml.len() + DECL.len());
    out.push_str(DECL);
    out.push_str(mathml);
    out
}

/// The `META-INF/manifest.xml` part listing every package member.
pub(super) fn manifest_xml(images: &[Image], formulas: &[Formula]) -> String {
    let mut out = String::new();
    out.push_str(DECL);
    out.push_str(
        "<manifest:manifest xmlns:manifest=\"urn:oasis:names:tc:opendocument:xmlns:manifest:1.0\" \
         manifest:version=\"1.3\">",
    );
    let _ = write!(
        out,
        "<manifest:file-entry manifest:full-path=\"/\" manifest:version=\"1.3\" \
         manifest:media-type=\"{MIMETYPE}\" />"
    );
    for (path, media_type) in [
        ("content.xml", "application/xml"),
        ("styles.xml", "application/xml"),
        ("meta.xml", "application/xml"),
        ("manifest.rdf", "application/rdf+xml"),
    ] {
        let _ = write!(
            out,
            "<manifest:file-entry manifest:full-path=\"{path}\" \
             manifest:media-type=\"{media_type}\" />"
        );
    }
    for image in images {
        let mut media_type = String::new();
        escape_attribute(&image.mime, &mut media_type);
        let _ = write!(
            out,
            "<manifest:file-entry manifest:full-path=\"Pictures/{}\" \
             manifest:media-type=\"{media_type}\" />",
            image.file_name
        );
    }
    for formula in formulas {
        let _ = write!(
            out,
            "<manifest:file-entry \
             manifest:media-type=\"application/vnd.oasis.opendocument.formula\" \
             manifest:full-path=\"Formula-{index}/\" manifest:version=\"1.3\" />\
             <manifest:file-entry manifest:full-path=\"Formula-{index}/content.xml\" \
             manifest:media-type=\"text/xml\" />\
             <manifest:file-entry manifest:full-path=\"Formula-{index}/settings.xml\" \
             manifest:media-type=\"text/xml\" />",
            index = formula.index
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
        MetaValue::MetaInlines(inlines) => carta_ast::to_plain_text(inlines),
        MetaValue::MetaBlocks(blocks) => {
            carta_ast::to_plain_text(carta_ast::single_block_inlines(blocks))
        }
        MetaValue::MetaBool(flag) => flag.to_string(),
        MetaValue::MetaList(items) => items.iter().map(plain_meta).collect::<Vec<_>>().join("; "),
        MetaValue::MetaMap(_) => String::new(),
    }
}

/// The inline content of a metadata field, or `None` when it is absent or empty.
pub(super) fn meta_inlines(meta: &BTreeMap<Text, MetaValue>, key: &str) -> Option<Vec<Inline>> {
    let inlines = value_inlines(meta.get(key)?);
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
                if !out.is_empty() {
                    out.push(Inline::Space);
                }
                out.extend(value_inlines(item));
            }
            out
        }
        _ => Vec::new(),
    }
}

/// The abstract's paragraphs, one inline sequence per `Para`/`Plain` block, or a single sequence for
/// an inline-valued field.
pub(super) fn abstract_paragraphs(value: &MetaValue) -> Vec<Vec<Inline>> {
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
        Some(MetaValue::MetaList(items)) => items
            .iter()
            .map(value_inlines)
            .filter(|inlines| !inlines.is_empty())
            .collect(),
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
