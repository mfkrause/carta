//! Reading a document's metadata map into the Dublin Core fields an EPUB package records.
//!
//! The document model carries free-form metadata; an EPUB package needs a fixed set of publication
//! fields — title, authors, language, identifier, date and the rest. This module projects the map
//! onto that set, filling in the deterministic defaults a container needs when a field is absent.

use carta_ast::{Inline, MetaValue, Text, to_plain_text};
use carta_core::media::sha1_hex;
use std::collections::BTreeMap;

/// The language tag used when the document names none. EPUB requires exactly one language, so a
/// value is always emitted.
const DEFAULT_LANGUAGE: &str = "en-US";

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
    pub identifier: String,
    /// Metadata elements carried through verbatim from the Dublin Core fragment.
    pub extra: Vec<ExtraMeta>,
}

impl BookMeta {
    /// Project the document's metadata map onto the publication fields, resolving each default. The
    /// `content_seed` is hashed into a stable identifier when the document names none.
    pub(crate) fn from_meta(meta: &BTreeMap<Text, MetaValue>, content_seed: &str) -> Self {
        let title_inlines = meta.get("title").map(meta_inlines).unwrap_or_default();
        let title_text = to_plain_text(&title_inlines);

        let creators = collect_creators(meta);

        let language = meta
            .get("lang")
            .map(meta_text)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_LANGUAGE.to_owned());

        let identifier = meta
            .get("identifier")
            .map(meta_text)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| content_uuid(content_seed));

        let rights_inlines = meta.get("rights").map(meta_inlines);
        let rights_text = rights_inlines.as_deref().map(to_plain_text);

        Self {
            title_inlines,
            title_text,
            subtitle_inlines: meta.get("subtitle").map(meta_inlines),
            creators,
            contributors: Vec::new(),
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
            identifier,
            extra: Vec::new(),
        }
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
/// order. An `author` is an authorship (`aut`); a `creator` may name its own role.
fn collect_creators(meta: &BTreeMap<Text, MetaValue>) -> Vec<Creator> {
    let mut creators = Vec::new();
    for value in meta.get("author").into_iter().flat_map(meta_items) {
        let inlines = meta_inlines(value);
        creators.push(Creator {
            text: to_plain_text(&inlines),
            inlines,
            role: Some("aut".to_owned()),
            file_as: None,
        });
    }
    for value in meta.get("creator").into_iter().flat_map(meta_items) {
        creators.push(creator_from_value(value));
    }
    creators
}

/// One creator from a `creator` entry, honoring an explicit `role`/`text` map when present.
fn creator_from_value(value: &MetaValue) -> Creator {
    if let MetaValue::MetaMap(map) = value {
        let inlines = map.get("text").map(meta_inlines).unwrap_or_default();
        let role = map
            .get("role")
            .map(meta_text)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "aut".to_owned());
        let file_as = map
            .get("file-as")
            .map(meta_text)
            .filter(|value| !value.is_empty());
        return Creator {
            text: to_plain_text(&inlines),
            inlines,
            role: Some(role),
            file_as,
        };
    }
    let inlines = meta_inlines(value);
    Creator {
        text: to_plain_text(&inlines),
        inlines,
        role: Some("aut".to_owned()),
        file_as: None,
    }
}

/// The plain-text values a list-shaped field (subjects, …) contributes.
fn collect_texts(value: Option<&MetaValue>) -> Vec<String> {
    value
        .into_iter()
        .flat_map(meta_items)
        .map(meta_text)
        .filter(|value| !value.is_empty())
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
    to_plain_text(&meta_inlines(value))
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
        for element in parse_fragment(fragment) {
            match element.name.as_str() {
                "dc:identifier" => self.identifier = element.text,
                "dc:title" => {
                    self.title_inlines = vec![Inline::Str(Text::from(element.text.clone()))];
                    self.title_text = element.text;
                }
                "dc:date" => self.date = Some(element.text),
                "dc:language" => self.language = element.text,
                "dc:description" => self.description = Some(element.text),
                "dc:publisher" => self.publisher = Some(element.text),
                "dc:rights" => {
                    self.rights_inlines = Some(vec![Inline::Str(Text::from(element.text.clone()))]);
                    self.rights_text = Some(element.text);
                }
                "dc:subject" => subjects.push(element.text),
                "dc:creator" => creators.push(creator_from_element(&element)),
                "dc:contributor" => contributors.push(creator_from_element(&element)),
                _ => self.extra.push(ExtraMeta {
                    name: element.name,
                    attributes: element.attributes,
                    text: element.text,
                }),
            }
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
