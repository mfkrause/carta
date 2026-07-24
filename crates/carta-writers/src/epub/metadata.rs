//! Reading a document's metadata map into the Dublin Core fields an EPUB package records.
//!
//! The document model carries free-form metadata; an EPUB package needs a fixed set of publication
//! fields: title, authors, language, identifier, date and the rest. This module projects the map
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
/// entry, or a structured entry that names none, takes: an authorship for creators, none for
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

/// Render inlines to plain text, spelling smart quotes out as their glyphs: a double-quoted span as
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
    /// projected value (a supplied `dc:identifier` replaces the generated one, supplied creators
    /// replace the document's), so the fragment has the final say. Elements without a dedicated
    /// field are carried through verbatim in [`Self::extra`].
    pub(crate) fn apply_metadata_xml(&mut self, fragment: &str) {
        let mut creators = Vec::new();
        let mut contributors = Vec::new();
        let mut subjects = Vec::new();
        let mut identifiers = Vec::new();
        for element in parse_fragment(fragment) {
            match element.name.as_str() {
                // empty elements are dropped: applying them would blank a projected default or emit
                // an empty, schema-invalid Dublin Core element
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

/// Parse a Dublin Core metadata fragment (a flat sequence of `<dc:*>` and `<meta>` elements) into
/// its elements. Comments, processing instructions and declarations are skipped; nested markup
/// inside an element's text is retained as written. The scan never indexes past the input, so an
/// unterminated element ends the parse.
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
mod tests;
