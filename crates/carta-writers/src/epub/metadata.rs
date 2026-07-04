//! Reading a document's metadata map into the Dublin Core fields an EPUB package records.
//!
//! The document model carries free-form metadata; an EPUB package needs a fixed set of publication
//! fields — title, authors, language, identifier, date and the rest. This module projects the map
//! onto that set, filling in the deterministic defaults a container needs when a field is absent.

use carta_ast::{Inline, MetaValue, QuoteType, Text};
use carta_core::media::sha1_hex;
use std::collections::BTreeMap;

/// The language tag used when neither the document nor the process locale names one. EPUB requires
/// exactly one language, so a value is always emitted.
pub(crate) const DEFAULT_LANGUAGE: &str = "en-US";

/// The language a locale value (the `LANG` environment variable) implies: the tag before any
/// charset (`.UTF-8`) or modifier (`@euro`) suffix, with `_` separators written as `-`. A set but
/// empty locale yields an empty language; only an absent locale falls back to [`DEFAULT_LANGUAGE`].
pub(crate) fn language_from_locale(locale: Option<&str>) -> String {
    match locale {
        None => DEFAULT_LANGUAGE.to_owned(),
        Some(value) => value
            .split(['.', '@'])
            .next()
            .unwrap_or(value)
            .replace('_', "-"),
    }
}

/// The document title shown where a package requires a non-empty name (the navigation heading, the
/// NCX document title, the reading-order guide) but the document supplies none.
pub(crate) const UNTITLED: &str = "UNTITLED";

/// One creator (author, editor, …) of the work.
#[derive(Debug, Clone)]
pub(crate) struct Creator {
    /// The name as inline content, for the title page.
    pub inlines: Vec<Inline>,
    /// The name as plain text, for the package metadata.
    pub text: String,
    /// The MARC relator code (`aut` for an author) recorded against the creator, when one is known.
    /// A creator supplied without a role carries none, and no role is recorded.
    pub role: Option<String>,
    /// The sortable form of the name (`Doe, Jane`), recorded when supplied.
    pub file_as: Option<String>,
}

/// One publication identifier: its value and, when the document named it, the identifier's scheme (a
/// `DOI`, an `ISBN-13`, …). The scheme is recorded verbatim; the package document projects it as the
/// scheme name in EPUB 2 and as its ONIX codelist-5 code in EPUB 3.
#[derive(Debug, Clone)]
pub(crate) struct Identifier {
    pub text: String,
    pub scheme: Option<String>,
}

impl Identifier {
    /// The ONIX codelist-5 code recording what kind of identifier this is, present when a scheme was
    /// named.
    pub(crate) fn onix_code(&self) -> Option<String> {
        self.scheme.as_deref().map(onix_code)
    }
}

/// A publication metadata element carried through verbatim: one supplied by the Dublin Core fragment
/// that has no dedicated projection (`dc:source`, a custom `<meta>`, …).
#[derive(Debug, Clone)]
pub(crate) struct ExtraMeta {
    pub name: String,
    pub attributes: Vec<(String, String)>,
    pub text: String,
}

/// The publication fields an EPUB package records, projected from the document's metadata map.
#[derive(Debug, Clone)]
pub(crate) struct BookMeta {
    pub title_inlines: Vec<Inline>,
    pub title_text: String,
    pub subtitle_inlines: Option<Vec<Inline>>,
    pub creators: Vec<Creator>,
    pub contributors: Vec<Creator>,
    pub date: Option<String>,
    pub language: String,
    pub subjects: Vec<String>,
    pub description: Option<String>,
    pub publisher: Option<String>,
    pub rights_inlines: Option<Vec<Inline>>,
    pub rights_text: Option<String>,
    /// The publication identifiers, in document order. Always holds at least one: a stable generated
    /// identifier stands in when the document names none. The first is the package's unique
    /// identifier.
    pub identifiers: Vec<Identifier>,
    /// Metadata elements carried through verbatim from the Dublin Core fragment.
    pub extra: Vec<ExtraMeta>,
}

impl BookMeta {
    /// Project the document's metadata map onto the publication fields, resolving each default. The
    /// `content_seed` is hashed into a stable identifier when the document names none;
    /// `fallback_language` is recorded when the document names no `lang`.
    pub(crate) fn from_meta(
        meta: &BTreeMap<Text, MetaValue>,
        content_seed: &str,
        fallback_language: &str,
    ) -> Self {
        let title_inlines = meta.get("title").map(meta_inlines).unwrap_or_default();
        let title_text = meta_plain_text(&title_inlines);

        let creators = collect_creators(meta);
        let contributors = collect_contributors(meta);

        let language = meta
            .get("lang")
            .map(meta_text)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| fallback_language.to_owned());

        let identifiers = collect_identifiers(meta, content_seed);

        let rights_inlines = meta.get("rights").map(meta_inlines);
        let rights_text = rights_inlines.as_deref().map(meta_plain_text);

        Self {
            title_inlines,
            title_text,
            subtitle_inlines: meta.get("subtitle").map(meta_inlines),
            creators,
            contributors,
            date: meta.get("date").map(meta_text).filter(|v| !v.is_empty()),
            language,
            subjects: collect_texts(meta.get("subject")),
            description: meta
                .get("description")
                .map(meta_text)
                .filter(|v| !v.is_empty()),
            publisher: meta
                .get("publisher")
                .map(meta_text)
                .filter(|v| !v.is_empty()),
            rights_inlines,
            rights_text,
            identifiers,
            extra: Vec::new(),
        }
    }

    /// The identifier the package's unique-identifier attribute points at: the first identifier's
    /// value.
    pub(crate) fn primary_identifier(&self) -> &str {
        self.identifiers
            .first()
            .map_or("", |identifier| identifier.text.as_str())
    }

    /// The title where a non-empty name is required, falling back to [`UNTITLED`].
    pub(crate) fn display_title(&self) -> &str {
        if self.title_text.is_empty() {
            UNTITLED
        } else {
            &self.title_text
        }
    }
}

/// The creators named by the metadata: each `author` entry, then each `creator` entry, in that
/// order. An `author` names an authorship (`aut`) and is taken as plain text; a `creator` may
/// instead be a structured entry naming its own role and sort name. Entries resolving to no name are
/// dropped.
fn collect_creators(meta: &BTreeMap<Text, MetaValue>) -> Vec<Creator> {
    let mut creators = Vec::new();
    for value in meta.get("author").into_iter().flat_map(meta_items) {
        let inlines = meta_inlines(value);
        let text = meta_plain_text(&inlines);
        if text.is_empty() {
            continue;
        }
        creators.push(Creator {
            inlines,
            text,
            role: Some("aut".to_owned()),
            file_as: None,
        });
    }
    for value in meta.get("creator").into_iter().flat_map(meta_items) {
        if let Some(creator) = agent_from_value(value, Some("aut")) {
            creators.push(creator);
        }
    }
    creators
}

/// The contributors named by the metadata: each `contributor` entry, structured or plain. A plain
/// contributor names no role; a structured one may. Entries resolving to no name are dropped.
fn collect_contributors(meta: &BTreeMap<Text, MetaValue>) -> Vec<Creator> {
    meta.get("contributor")
        .into_iter()
        .flat_map(meta_items)
        .filter_map(|value| agent_from_value(value, None))
        .collect()
}

/// One agent from a `creator`/`contributor` entry, honoring an explicit `role`/`file-as`/`text` map
/// when present, otherwise taking the value as the agent's name. `default_role` is the role a plain
/// entry, or a structured entry that names none, takes — an authorship for creators, none for
/// contributors. Returns `None` when the entry resolves to no name.
fn agent_from_value(value: &MetaValue, default_role: Option<&str>) -> Option<Creator> {
    if let MetaValue::MetaMap(map) = value {
        let inlines = map.get("text").map(meta_inlines).unwrap_or_default();
        let text = meta_plain_text(&inlines);
        if text.is_empty() {
            return None;
        }
        let role = map
            .get("role")
            .map(meta_text)
            .filter(|value| !value.is_empty())
            .or_else(|| default_role.map(str::to_owned));
        let file_as = map
            .get("file-as")
            .map(meta_text)
            .filter(|value| !value.is_empty());
        return Some(Creator {
            inlines,
            text,
            role,
            file_as,
        });
    }
    let inlines = meta_inlines(value);
    let text = meta_plain_text(&inlines);
    if text.is_empty() {
        return None;
    }
    Some(Creator {
        inlines,
        text,
        role: default_role.map(str::to_owned),
        file_as: None,
    })
}

/// The publication identifiers named by the metadata, in document order. A plain value gives a bare
/// identifier; a structured `{scheme, text}` entry additionally records the scheme's ONIX code. When
/// the document names none, a single stable identifier derived from the content stands in.
fn collect_identifiers(meta: &BTreeMap<Text, MetaValue>, content_seed: &str) -> Vec<Identifier> {
    let mut identifiers: Vec<Identifier> = meta
        .get("identifier")
        .into_iter()
        .flat_map(meta_items)
        .filter_map(identifier_from_value)
        .collect();
    if identifiers.is_empty() {
        identifiers.push(Identifier {
            text: content_uuid(content_seed),
            scheme: None,
        });
    }
    identifiers
}

/// One identifier from an `identifier` entry. A structured `{scheme, text}` entry records its named
/// scheme; a bare value records none. Returns `None` when the entry resolves to no value.
fn identifier_from_value(value: &MetaValue) -> Option<Identifier> {
    if let MetaValue::MetaMap(map) = value {
        let text = map
            .get("text")
            .map(meta_text)
            .filter(|value| !value.is_empty())?;
        let scheme = map
            .get("scheme")
            .map(meta_text)
            .filter(|value| !value.is_empty());
        return Some(Identifier { text, scheme });
    }
    let text = meta_text(value);
    if text.is_empty() {
        return None;
    }
    Some(Identifier { text, scheme: None })
}

/// The ONIX codelist-5 code recording what kind of identifier a named scheme denotes. The mapping is
/// exact and case-sensitive; any scheme without a listed code falls back to `01` ("proprietary").
fn onix_code(scheme: &str) -> String {
    let code = match scheme {
        "ISBN-10" => "02",
        "GTIN-13" => "03",
        "UPC" => "04",
        "ISMN-10" => "05",
        "DOI" => "06",
        "LCCN" => "13",
        "GTIN-14" => "14",
        "ISBN-13" => "15",
        "URN" => "22",
        "ISMN-13" => "25",
        "ISBN-A" => "26",
        _ => "01",
    };
    code.to_owned()
}

/// The plain-text values a list-shaped field (subjects, …) contributes, empty entries included so a
/// deliberately blank value keeps its place.
fn collect_texts(value: Option<&MetaValue>) -> Vec<String> {
    value
        .into_iter()
        .flat_map(meta_items)
        .map(meta_text)
        .collect()
}

/// The elements of a list-shaped value, or the value itself treated as a single element.
fn meta_items(value: &MetaValue) -> Vec<&MetaValue> {
    match value {
        MetaValue::MetaList(items) => items.iter().collect(),
        other => vec![other],
    }
}

/// A metadata value rendered as inline content.
pub(crate) fn meta_inlines(value: &MetaValue) -> Vec<Inline> {
    match value {
        MetaValue::MetaInlines(inlines) => inlines.clone(),
        MetaValue::MetaString(text) => vec![Inline::Str(text.clone())],
        MetaValue::MetaBlocks(blocks) => blocks.iter().flat_map(block_inlines).collect(),
        MetaValue::MetaBool(value) => vec![Inline::Str(Text::from(value.to_string()))],
        MetaValue::MetaList(items) => items.iter().flat_map(meta_inlines).collect(),
        MetaValue::MetaMap(_) => Vec::new(),
    }
}

/// The inline content a block contributes when a metadata value is block-shaped (a bare paragraph or
/// plain line).
fn block_inlines(block: &carta_ast::Block) -> Vec<Inline> {
    match block {
        carta_ast::Block::Para(inlines) | carta_ast::Block::Plain(inlines) => inlines.clone(),
        _ => Vec::new(),
    }
}

/// A metadata value rendered as plain text.
fn meta_text(value: &MetaValue) -> String {
    meta_plain_text(&meta_inlines(value))
}

/// Render inlines to plain text, spelling smart quotes out as their glyphs — a double-quoted span as
/// `\u{201c}…\u{201d}`, a single-quoted span as `\u{2018}…\u{2019}`. A title, name, or other metadata
/// value carries its punctuation into the package document, where the quote characters must appear
/// literally rather than being dropped with the quotation node.
fn meta_plain_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    push_meta_plain_text(inlines, &mut out);
    out
}

fn push_meta_plain_text(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Str(text) | Inline::Code(_, text) | Inline::Math(_, text) => out.push_str(text),
            Inline::Space | Inline::SoftBreak | Inline::LineBreak => out.push(' '),
            Inline::Quoted(quote, xs) => {
                let (open, close) = match quote {
                    QuoteType::DoubleQuote => ('\u{201c}', '\u{201d}'),
                    QuoteType::SingleQuote => ('\u{2018}', '\u{2019}'),
                };
                out.push(open);
                push_meta_plain_text(xs, out);
                out.push(close);
            }
            Inline::Emph(xs)
            | Inline::Underline(xs)
            | Inline::Strong(xs)
            | Inline::Strikeout(xs)
            | Inline::Superscript(xs)
            | Inline::Subscript(xs)
            | Inline::SmallCaps(xs)
            | Inline::Cite(_, xs)
            | Inline::Link(_, xs, _)
            | Inline::Image(_, xs, _)
            | Inline::Span(_, xs) => push_meta_plain_text(xs, out),
            Inline::RawInline(..) | Inline::Note(_) => {}
        }
    }
}

impl BookMeta {
    /// Merge a Dublin Core metadata fragment into these fields. A recognized element overrides the
    /// projected value — a supplied `dc:identifier` replaces the generated one, supplied creators
    /// replace the document's — so the fragment has the final say. Elements without a dedicated
    /// field are carried through verbatim in [`Self::extra`].
    pub(crate) fn apply_metadata_xml(&mut self, fragment: &str) {
        let mut creators = Vec::new();
        let mut contributors = Vec::new();
        let mut subjects = Vec::new();
        let mut identifiers = Vec::new();
        for element in parse_fragment(fragment) {
            match element.name.as_str() {
                // An element that carries no text is dropped rather than applied: overriding a field
                // with an empty value would blank a projected default (the language, the modified
                // date) or emit an empty, schema-invalid Dublin Core element.
                "dc:identifier" => {
                    if !element.text.is_empty() {
                        let scheme = element.attribute("opf:scheme").map(str::to_owned);
                        identifiers.push(Identifier {
                            text: element.text,
                            scheme,
                        });
                    }
                }
                "dc:title" => {
                    if !element.text.is_empty() {
                        self.title_inlines = vec![Inline::Str(Text::from(element.text.clone()))];
                        self.title_text = element.text;
                    }
                }
                "dc:date" => {
                    if !element.text.is_empty() {
                        self.date = Some(element.text);
                    }
                }
                "dc:language" => {
                    if !element.text.is_empty() {
                        self.language = element.text;
                    }
                }
                "dc:description" => {
                    if !element.text.is_empty() {
                        self.description = Some(element.text);
                    }
                }
                "dc:publisher" => {
                    if !element.text.is_empty() {
                        self.publisher = Some(element.text);
                    }
                }
                "dc:rights" => {
                    if !element.text.is_empty() {
                        self.rights_inlines =
                            Some(vec![Inline::Str(Text::from(element.text.clone()))]);
                        self.rights_text = Some(element.text);
                    }
                }
                "dc:subject" => {
                    if !element.text.is_empty() {
                        subjects.push(element.text);
                    }
                }
                "dc:creator" => {
                    if !element.text.is_empty() {
                        creators.push(creator_from_element(&element));
                    }
                }
                "dc:contributor" => {
                    if !element.text.is_empty() {
                        contributors.push(creator_from_element(&element));
                    }
                }
                _ => self.extra.push(ExtraMeta {
                    name: element.name,
                    attributes: element.attributes,
                    text: element.text,
                }),
            }
        }
        if !identifiers.is_empty() {
            self.identifiers = identifiers;
        }
        if !creators.is_empty() {
            self.creators = creators;
        }
        if !contributors.is_empty() {
            self.contributors = contributors;
        }
        if !subjects.is_empty() {
            self.subjects = subjects;
        }
    }
}

/// One creator or contributor projected from a `dc:creator`/`dc:contributor` element, honoring its
/// `opf:role` and `opf:file-as` attributes. An element without a role carries none.
fn creator_from_element(element: &XmlElement) -> Creator {
    let role = element
        .attribute("opf:role")
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let file_as = element
        .attribute("opf:file-as")
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    Creator {
        inlines: vec![Inline::Str(Text::from(element.text.clone()))],
        text: element.text.clone(),
        role,
        file_as,
    }
}

/// A parsed XML element from a metadata fragment: its qualified name, attributes and text content.
struct XmlElement {
    name: String,
    attributes: Vec<(String, String)>,
    text: String,
}

impl XmlElement {
    fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Parse a Dublin Core metadata fragment — a flat sequence of `<dc:*>` and `<meta>` elements — into
/// its elements. Comments, processing instructions and declarations are skipped; nested markup
/// inside an element's text is retained as written. The scan never indexes past the input, so an
/// unterminated element simply ends the parse.
fn parse_fragment(input: &str) -> Vec<XmlElement> {
    let chars: Vec<char> = input.chars().collect();
    let mut index = 0;
    let mut elements = Vec::new();
    while index < chars.len() {
        if chars.get(index) != Some(&'<') {
            index += 1;
            continue;
        }
        index += 1;
        // A comment, processing instruction, declaration or stray close tag carries no element.
        if matches!(chars.get(index), Some('!' | '?' | '/')) {
            while index < chars.len() && chars.get(index) != Some(&'>') {
                index += 1;
            }
            index += 1;
            continue;
        }
        let name = take_name(&chars, &mut index);
        let (attributes, self_closing) = take_attributes(&chars, &mut index);
        let text = if self_closing {
            String::new()
        } else {
            take_text(&chars, &mut index, &name)
        };
        if !name.is_empty() {
            elements.push(XmlElement {
                name,
                attributes,
                text,
            });
        }
    }
    elements
}

/// Read an element or attribute name: characters up to whitespace or a tag delimiter.
fn take_name(chars: &[char], index: &mut usize) -> String {
    let mut name = String::new();
    while let Some(&c) = chars.get(*index) {
        if c.is_whitespace() || c == '>' || c == '/' || c == '=' {
            break;
        }
        name.push(c);
        *index += 1;
    }
    name
}

/// Read an element's attributes up to the closing `>`, reporting whether the tag self-closed (`/>`).
fn take_attributes(chars: &[char], index: &mut usize) -> (Vec<(String, String)>, bool) {
    let mut attributes = Vec::new();
    loop {
        skip_whitespace(chars, index);
        match chars.get(*index) {
            Some('/') => {
                *index += 1;
                skip_whitespace(chars, index);
                if chars.get(*index) == Some(&'>') {
                    *index += 1;
                }
                return (attributes, true);
            }
            Some('>') => {
                *index += 1;
                return (attributes, false);
            }
            None => return (attributes, true),
            Some(_) => {
                let key = take_name(chars, index);
                skip_whitespace(chars, index);
                let mut value = String::new();
                if chars.get(*index) == Some(&'=') {
                    *index += 1;
                    skip_whitespace(chars, index);
                    value = take_quoted(chars, index);
                }
                if key.is_empty() {
                    // No progress would be made; bail rather than spin.
                    *index += 1;
                } else {
                    attributes.push((key, unescape(&value)));
                }
            }
        }
    }
}

/// Read a quoted attribute value, consuming the surrounding quotes.
fn take_quoted(chars: &[char], index: &mut usize) -> String {
    let Some(&quote) = chars.get(*index) else {
        return String::new();
    };
    if quote != '"' && quote != '\'' {
        return String::new();
    }
    *index += 1;
    let mut value = String::new();
    while let Some(&c) = chars.get(*index) {
        *index += 1;
        if c == quote {
            break;
        }
        value.push(c);
    }
    value
}

/// Read an element's text up to and including its `</name>` close tag.
fn take_text(chars: &[char], index: &mut usize, name: &str) -> String {
    let close: Vec<char> = format!("</{name}>").chars().collect();
    let mut text = String::new();
    while *index < chars.len() {
        if chars.get(*index) == Some(&'<') && matches_at(chars, *index, &close) {
            *index += close.len();
            return unescape(text.trim());
        }
        if let Some(&c) = chars.get(*index) {
            text.push(c);
        }
        *index += 1;
    }
    unescape(text.trim())
}

/// Whether `needle` appears in `chars` starting at `start`.
fn matches_at(chars: &[char], start: usize, needle: &[char]) -> bool {
    needle
        .iter()
        .enumerate()
        .all(|(offset, expected)| chars.get(start + offset) == Some(expected))
}

fn skip_whitespace(chars: &[char], index: &mut usize) {
    while matches!(chars.get(*index), Some(c) if c.is_whitespace()) {
        *index += 1;
    }
}

/// Resolve the five predefined XML entities. An unrecognized `&…;` is left as written.
fn unescape(input: &str) -> String {
    if !input.contains('&') {
        return input.to_owned();
    }
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(amp) = rest.find('&') {
        let (before, from_amp) = rest.split_at(amp);
        out.push_str(before);
        let after_amp = from_amp.get(1..).unwrap_or("");
        if let Some(end) = after_amp.find(';') {
            match after_amp.get(..end).unwrap_or("") {
                "amp" => out.push('&'),
                "lt" => out.push('<'),
                "gt" => out.push('>'),
                "quot" => out.push('"'),
                "apos" => out.push('\''),
                other => {
                    out.push('&');
                    out.push_str(other);
                    out.push(';');
                }
            }
            rest = after_amp.get(end + 1..).unwrap_or("");
        } else {
            out.push('&');
            rest = after_amp;
        }
    }
    out.push_str(rest);
    out
}

/// A stable `urn:uuid` derived from the document, used as the publication identifier when the
/// document names none. Identical content always yields the same identifier.
fn content_uuid(seed: &str) -> String {
    let digest = sha1_hex(seed.as_bytes());
    let mut chars = digest.chars();
    let mut group = |width: usize| -> String { chars.by_ref().take(width).collect() };
    format!(
        "urn:uuid:{}-{}-{}-{}-{}",
        group(8),
        group(4),
        group(4),
        group(4),
        group(12),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        BookMeta, DEFAULT_LANGUAGE, Inline, MetaValue, QuoteType, Text, collect_contributors,
        collect_creators, collect_identifiers, collect_texts, identifier_from_value,
        language_from_locale, meta_plain_text, onix_code,
    };
    use std::collections::BTreeMap;

    fn meta_string(value: &str) -> MetaValue {
        MetaValue::MetaString(Text::from(value))
    }

    fn meta_map(pairs: &[(&str, MetaValue)]) -> MetaValue {
        MetaValue::MetaMap(
            pairs
                .iter()
                .map(|(key, value)| (Text::from(*key), value.clone()))
                .collect(),
        )
    }

    fn meta_with(key: &str, value: MetaValue) -> BTreeMap<Text, MetaValue> {
        let mut meta = BTreeMap::new();
        meta.insert(Text::from(key), value);
        meta
    }

    #[test]
    fn onix_code_maps_known_schemes_case_sensitively() {
        assert_eq!(onix_code("DOI"), "06");
        assert_eq!(onix_code("ISBN-13"), "15");
        assert_eq!(onix_code("ISBN-10"), "02");
        assert_eq!(onix_code("URN"), "22");
        // A miscased or unlisted scheme falls back to the proprietary code.
        assert_eq!(onix_code("doi"), "01");
        assert_eq!(onix_code("ISBN13"), "01");
        assert_eq!(onix_code("something-else"), "01");
    }

    #[test]
    fn identifier_from_map_records_scheme_and_onix_code() {
        let value = meta_map(&[
            ("scheme", meta_string("DOI")),
            ("text", meta_string("10.1000/x")),
        ]);
        let identifier = identifier_from_value(&value).expect("an identifier");
        assert_eq!(identifier.text, "10.1000/x");
        assert_eq!(identifier.scheme.as_deref(), Some("DOI"));
        assert_eq!(identifier.onix_code().as_deref(), Some("06"));
    }

    #[test]
    fn identifier_from_plain_value_has_no_scheme() {
        let identifier = identifier_from_value(&meta_string("book-1")).expect("an identifier");
        assert_eq!(identifier.text, "book-1");
        assert!(identifier.scheme.is_none());
        assert!(identifier.onix_code().is_none());
    }

    #[test]
    fn identifier_with_empty_text_is_dropped() {
        let value = meta_map(&[("scheme", meta_string("DOI")), ("text", meta_string(""))]);
        assert!(identifier_from_value(&value).is_none());
        assert!(identifier_from_value(&meta_string("")).is_none());
    }

    #[test]
    fn identifier_with_empty_scheme_records_no_scheme() {
        let value = meta_map(&[("scheme", meta_string("")), ("text", meta_string("book-1"))]);
        let identifier = identifier_from_value(&value).expect("an identifier");
        assert_eq!(identifier.text, "book-1");
        assert!(identifier.scheme.is_none());
        assert!(identifier.onix_code().is_none());
    }

    #[test]
    fn empty_fragment_fields_leave_projected_defaults_intact() {
        let mut meta = BookMeta::from_meta(&BTreeMap::new(), "seed", DEFAULT_LANGUAGE);
        let generated = meta.primary_identifier().to_owned();
        assert!(generated.starts_with("urn:uuid:"));
        meta.apply_metadata_xml(
            "<dc:language></dc:language>\n\
             <dc:identifier></dc:identifier>\n\
             <dc:date></dc:date>\n\
             <dc:publisher></dc:publisher>\n\
             <dc:description></dc:description>\n\
             <dc:rights></dc:rights>",
        );
        assert_eq!(meta.language, "en-US");
        assert_eq!(meta.primary_identifier(), generated);
        assert!(meta.date.is_none());
        assert!(meta.publisher.is_none());
        assert!(meta.description.is_none());
        assert!(meta.rights_text.is_none());
    }

    #[test]
    fn language_from_locale_reduces_the_tag_and_defaults_only_when_absent() {
        assert_eq!(language_from_locale(None), "en-US");
        assert_eq!(language_from_locale(Some("")), "");
        assert_eq!(language_from_locale(Some("C")), "C");
        assert_eq!(language_from_locale(Some("C.UTF-8")), "C");
        assert_eq!(language_from_locale(Some("POSIX")), "POSIX");
        assert_eq!(language_from_locale(Some("en_US.UTF-8")), "en-US");
        assert_eq!(language_from_locale(Some("de_AT.UTF-8@euro")), "de-AT");
        assert_eq!(language_from_locale(Some("fr")), "fr");
    }

    #[test]
    fn non_empty_fragment_fields_override_the_defaults() {
        let mut meta = BookMeta::from_meta(&BTreeMap::new(), "seed", DEFAULT_LANGUAGE);
        meta.apply_metadata_xml(
            "<dc:language>fr</dc:language>\n<dc:identifier>urn:isbn:123</dc:identifier>",
        );
        assert_eq!(meta.language, "fr");
        assert_eq!(meta.primary_identifier(), "urn:isbn:123");
    }

    #[test]
    fn collect_identifiers_falls_back_to_a_content_identifier() {
        let identifiers = collect_identifiers(&BTreeMap::new(), "seed");
        let only = identifiers.first().expect("an identifier");
        assert_eq!(identifiers.len(), 1);
        assert!(only.text.starts_with("urn:uuid:"));
        assert!(only.scheme.is_none());
    }

    #[test]
    fn a_structured_author_resolves_to_no_name_and_is_dropped() {
        let meta = meta_with(
            "author",
            MetaValue::MetaList(vec![
                meta_string("Ada Lovelace"),
                meta_map(&[("text", meta_string("Grace Hopper"))]),
            ]),
        );
        let creators = collect_creators(&meta);
        let only = creators.first().expect("a creator");
        assert_eq!(creators.len(), 1);
        assert_eq!(only.text, "Ada Lovelace");
        assert_eq!(only.role.as_deref(), Some("aut"));
    }

    #[test]
    fn a_structured_creator_names_its_role_and_sort_name() {
        let meta = meta_with(
            "creator",
            meta_map(&[
                ("text", meta_string("Jane Doe")),
                ("role", meta_string("edt")),
                ("file-as", meta_string("Doe, Jane")),
            ]),
        );
        let creators = collect_creators(&meta);
        let only = creators.first().expect("a creator");
        assert_eq!(only.role.as_deref(), Some("edt"));
        assert_eq!(only.file_as.as_deref(), Some("Doe, Jane"));
    }

    #[test]
    fn a_plain_contributor_names_no_role() {
        let meta = meta_with("contributor", meta_string("Ed Itor"));
        let contributors = collect_contributors(&meta);
        let only = contributors.first().expect("a contributor");
        assert_eq!(only.text, "Ed Itor");
        assert!(only.role.is_none());
    }

    #[test]
    fn smart_quotes_render_as_glyphs() {
        let inlines = vec![
            Inline::Quoted(QuoteType::DoubleQuote, vec![Inline::Str(Text::from("Hi"))]),
            Inline::Space,
            Inline::Quoted(QuoteType::SingleQuote, vec![Inline::Str(Text::from("x"))]),
        ];
        assert_eq!(
            meta_plain_text(&inlines),
            "\u{201c}Hi\u{201d} \u{2018}x\u{2019}"
        );
    }

    #[test]
    fn empty_list_entries_keep_their_place() {
        let value = MetaValue::MetaList(vec![meta_string(""), meta_string("Fiction")]);
        assert_eq!(
            collect_texts(Some(&value)),
            vec![String::new(), "Fiction".to_owned()]
        );
    }

    #[test]
    fn meta_inlines_renders_each_value_shape() {
        use super::meta_inlines;
        use carta_ast::Block;
        // A block-shaped value contributes each paragraph's inlines; other blocks contribute nothing.
        let blocks = MetaValue::MetaBlocks(vec![
            Block::Para(vec![Inline::Str(Text::from("para"))]),
            Block::HorizontalRule,
        ]);
        assert_eq!(meta_plain_text(&meta_inlines(&blocks)), "para");
        // A boolean renders its literal spelling.
        assert_eq!(
            meta_plain_text(&meta_inlines(&MetaValue::MetaBool(true))),
            "true"
        );
        // A list flattens each element in order.
        let list = MetaValue::MetaList(vec![meta_string("a"), meta_string("b")]);
        assert_eq!(meta_plain_text(&meta_inlines(&list)), "ab");
        // A map carries no inline rendering of its own.
        assert!(meta_inlines(&meta_map(&[("k", meta_string("v"))])).is_empty());
    }

    #[test]
    fn plain_text_flattens_formatting_and_drops_opaque_inlines() {
        use carta_ast::{Format, Target};
        let inlines = vec![
            Inline::Emph(vec![Inline::Str(Text::from("a"))]),
            Inline::Strong(vec![Inline::Str(Text::from("b"))]),
            Inline::Link(
                Box::default(),
                vec![Inline::Str(Text::from("c"))],
                Box::<Target>::default(),
            ),
            Inline::Code(Box::default(), Text::from("d")),
            // A raw span and a footnote carry no plain-text value into a package field.
            Inline::RawInline(Format(Text::from("html")), Text::from("<x>")),
            Inline::Note(Vec::new()),
        ];
        assert_eq!(meta_plain_text(&inlines), "abcd");
    }

    #[test]
    fn apply_metadata_xml_projects_every_dublin_core_element() {
        use super::BookMeta;
        let mut meta = BookMeta::from_meta(&BTreeMap::new(), "seed", DEFAULT_LANGUAGE);
        assert_eq!(meta.language, "en-US");
        let fragment = concat!(
            "<!-- a leading comment is skipped -->\n",
            "<?xml-stylesheet type=\"text/css\"?>\n",
            "<dc:identifier opf:scheme=\"DOI\">10.1000/xyz</dc:identifier>\n",
            "<dc:title>Overridden &amp; Titled</dc:title>\n",
            "<dc:date>2020-01-02</dc:date>\n",
            "<dc:language>fr</dc:language>\n",
            "<dc:description>A short &lt;book&gt; & more</dc:description>\n",
            "<dc:publisher>Press &frac; House</dc:publisher>\n",
            "<dc:rights>&quot;Quoted&quot; &apos;x&apos;</dc:rights>\n",
            "<dc:subject class='sci'>Science</dc:subject>\n",
            "<dc:subject>Fiction</dc:subject>\n",
            "<dc:creator opf:role=\"aut\" opf:file-as=\"Doe, Jane\">Jane Doe</dc:creator>\n",
            "<dc:contributor opf:role=\"edt\">Ed Itor</dc:contributor>\n",
            "<meta name=\"cover\" content=\"cover-image\" />\n",
            "<meta property=\"custom:field\">custom value</meta>\n",
        );
        meta.apply_metadata_xml(fragment);

        // A supplied identifier replaces the generated one and carries its scheme's ONIX code.
        assert_eq!(meta.identifiers.len(), 1);
        assert_eq!(meta.primary_identifier(), "10.1000/xyz");
        let identifier = meta.identifiers.first().expect("an identifier");
        assert_eq!(identifier.onix_code().as_deref(), Some("06"));

        assert_eq!(meta.title_text, "Overridden & Titled");
        assert_eq!(meta.date.as_deref(), Some("2020-01-02"));
        assert_eq!(meta.language, "fr");
        // Every predefined entity resolves; a bare `&` and an unknown entity are left as written.
        assert_eq!(meta.description.as_deref(), Some("A short <book> & more"));
        assert_eq!(meta.publisher.as_deref(), Some("Press &frac; House"));
        assert_eq!(meta.rights_text.as_deref(), Some("\"Quoted\" 'x'"));
        assert_eq!(
            meta.subjects,
            vec!["Science".to_owned(), "Fiction".to_owned()]
        );

        assert_eq!(meta.creators.len(), 1);
        let creator = meta.creators.first().expect("a creator");
        assert_eq!(creator.text, "Jane Doe");
        assert_eq!(creator.role.as_deref(), Some("aut"));
        assert_eq!(creator.file_as.as_deref(), Some("Doe, Jane"));

        assert_eq!(meta.contributors.len(), 1);
        let contributor = meta.contributors.first().expect("a contributor");
        assert_eq!(contributor.role.as_deref(), Some("edt"));

        // Elements with no dedicated field are carried through verbatim, self-closing ones included.
        assert_eq!(meta.extra.len(), 2);
        let cover = meta.extra.first().expect("first extra");
        assert_eq!(cover.name, "meta");
        assert!(cover.text.is_empty());
        assert_eq!(
            cover.attributes,
            vec![
                ("name".to_owned(), "cover".to_owned()),
                ("content".to_owned(), "cover-image".to_owned()),
            ]
        );
        let custom = meta.extra.get(1).expect("second extra");
        assert_eq!(custom.text, "custom value");
    }

    #[test]
    fn apply_metadata_xml_leaves_projection_intact_when_it_names_nothing() {
        use super::BookMeta;
        let source = meta_with("title", meta_string("Original"));
        let mut meta = BookMeta::from_meta(&source, "seed", DEFAULT_LANGUAGE);
        let identifier_count = meta.identifiers.len();
        meta.apply_metadata_xml("   \n<!-- nothing to project -->\n  ");
        assert_eq!(meta.title_text, "Original");
        assert_eq!(meta.identifiers.len(), identifier_count);
        assert!(meta.creators.is_empty());
        assert!(meta.contributors.is_empty());
        assert!(meta.subjects.is_empty());
        assert!(meta.extra.is_empty());
    }

    #[test]
    fn apply_metadata_xml_ignores_empty_overrides() {
        use super::BookMeta;
        let source = meta_with("title", meta_string("Original"));
        let mut meta = BookMeta::from_meta(&source, "seed", DEFAULT_LANGUAGE);
        // An empty recognized element is dropped rather than applied: it neither blanks the projected
        // title nor pushes an empty subject, creator, or contributor over the projected values.
        meta.apply_metadata_xml(concat!(
            "<dc:title></dc:title>\n",
            "<dc:subject></dc:subject>\n",
            "<dc:creator></dc:creator>\n",
            "<dc:contributor></dc:contributor>\n",
        ));
        assert_eq!(meta.title_text, "Original");
        assert!(meta.subjects.is_empty());
        assert!(meta.creators.is_empty());
        assert!(meta.contributors.is_empty());
    }
}
