//! The document-property parts: `docProps/core.xml`, `docProps/app.xml` and `docProps/custom.xml`.
//!
//! Core properties carry a fixed set of well-known fields (title, author, category, description,
//! language, subject and keywords) alongside the creation and modification timestamps. Every other
//! metadata field routes to the custom properties, one named property each. The application part
//! identifies the producer and is otherwise fixed.

use carta_ast::{MetaValue, Text};
use carta_core::container::xml::Element;
use std::collections::BTreeMap;

/// The custom-property format identifier every entry shares.
const CUSTOM_FMTID: &str = "{D5CDD505-2E9C-101B-9397-08002B2CF9AE}";

/// Metadata keys that populate a core or application property. Every other key becomes a custom
/// property, so this set is what the custom part excludes.
const CORE_KEYS: &[&str] = &[
    "title",
    "author",
    "category",
    "description",
    "lang",
    "subject",
    "keywords",
];

/// Flattens a metadata value to plain text.
fn plain(value: &MetaValue) -> String {
    match value {
        MetaValue::MetaString(text) => text.to_string(),
        MetaValue::MetaInlines(inlines) => carta_ast::to_plain_text(inlines),
        MetaValue::MetaBlocks(blocks) => {
            carta_ast::to_plain_text(carta_ast::single_block_inlines(blocks))
        }
        MetaValue::MetaBool(flag) => flag.to_string(),
        MetaValue::MetaList(items) => items.iter().map(plain).collect::<Vec<_>>().join("; "),
        MetaValue::MetaMap(_) => String::new(),
    }
}

/// The plain-text value of a metadata field, or the empty string when absent.
fn field(meta: &BTreeMap<Text, MetaValue>, key: &str) -> String {
    meta.get(key).map(plain).unwrap_or_default()
}

/// A core-property element carrying flattened text.
fn text_element(name: &str, value: &str) -> Element {
    Element::new(name).text(value)
}

/// The `docProps/core.xml` part: title, author, language, subject, keywords, and reproducible
/// timestamps. Title, author and keywords are always present, empty when the document gives none;
/// language and subject appear only when the document carries them.
pub(super) fn core_xml(meta: &BTreeMap<Text, MetaValue>, epoch: i64) -> String {
    let stamp = iso_utc(epoch);
    let created = Element::new("dcterms:created")
        .attr("xsi:type", "dcterms:W3CDTF")
        .text(&stamp);
    let modified = Element::new("dcterms:modified")
        .attr("xsi:type", "dcterms:W3CDTF")
        .text(&stamp);
    let mut root = Element::new("cp:coreProperties")
        .attr(
            "xmlns:cp",
            "http://schemas.openxmlformats.org/package/2006/metadata/core-properties",
        )
        .attr("xmlns:dc", "http://purl.org/dc/elements/1.1/")
        .attr("xmlns:dcterms", "http://purl.org/dc/terms/")
        .attr("xmlns:dcmitype", "http://purl.org/dc/dcmitype/")
        .attr("xmlns:xsi", "http://www.w3.org/2001/XMLSchema-instance")
        .child(text_element("dc:title", &field(meta, "title")))
        .child(text_element("dc:creator", &field(meta, "author")));
    let category = field(meta, "category");
    if !category.is_empty() {
        root.push(text_element("cp:category", &category));
    }
    let description = field(meta, "description");
    if !description.is_empty() {
        root.push(text_element("dc:description", &description));
    }
    let language = field(meta, "lang");
    if !language.is_empty() {
        root.push(text_element("dc:language", &language));
    }
    let subject = field(meta, "subject");
    if !subject.is_empty() {
        root.push(text_element("dc:subject", &subject));
    }
    root.child(text_element("cp:keywords", &field(meta, "keywords")))
        .child(created)
        .child(modified)
        .render_document()
}

/// The `docProps/app.xml` part identifying the producer.
pub(super) fn app_xml() -> String {
    Element::new("Properties")
        .attr(
            "xmlns",
            "http://schemas.openxmlformats.org/officeDocument/2006/extended-properties",
        )
        .attr(
            "xmlns:vt",
            "http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes",
        )
        .child(text_element("Application", "carta"))
        .child(text_element("DocSecurity", "0"))
        .child(text_element("ScaleCrop", "false"))
        .child(text_element("SharedDoc", "false"))
        .child(text_element("HyperlinksChanged", "false"))
        .child(text_element("LinksUpToDate", "false"))
        .render_document()
}

/// The `docProps/custom.xml` part carrying any extended metadata fields, empty when there are none.
pub(super) fn custom_xml(meta: &BTreeMap<Text, MetaValue>) -> String {
    let mut properties = Element::new("Properties")
        .attr(
            "xmlns",
            "http://schemas.openxmlformats.org/officeDocument/2006/custom-properties",
        )
        .attr(
            "xmlns:vt",
            "http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes",
        );
    // Property identifiers begin at 2: identifiers 0 and 1 are reserved by the format.
    let mut pid = 2;
    for (key, value) in meta {
        if CORE_KEYS.contains(&key.as_str()) {
            continue;
        }
        properties.push(
            Element::new("property")
                .attr("fmtid", CUSTOM_FMTID)
                .attr("pid", &pid.to_string())
                .attr("name", key)
                .child(Element::new("vt:lpwstr").text(&plain(value))),
        );
        pid += 1;
    }
    properties.render_document()
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

/// Converts a count of days since 1970-01-01 to a `(year, month, day)` civil date. Uses the
/// era-based algorithm, valid across the full range of representable days.
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
    use super::iso_utc;

    #[test]
    fn epoch_one_is_the_first_second_of_1970() {
        assert_eq!(iso_utc(1), "1970-01-01T00:00:01Z");
    }

    #[test]
    fn a_known_instant_round_trips() {
        // 2021-01-01T00:00:00Z is 1_609_459_200 seconds after the epoch.
        assert_eq!(iso_utc(1_609_459_200), "2021-01-01T00:00:00Z");
    }
}
