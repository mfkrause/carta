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
    let blocks = MetaValue::MetaBlocks(vec![
        Block::Para(vec![Inline::Str(Text::from("para"))]),
        Block::HorizontalRule,
    ]);
    assert_eq!(meta_plain_text(&meta_inlines(&blocks)), "para");
    assert_eq!(
        meta_plain_text(&meta_inlines(&MetaValue::MetaBool(true))),
        "true"
    );
    let list = MetaValue::MetaList(vec![meta_string("a"), meta_string("b")]);
    assert_eq!(meta_plain_text(&meta_inlines(&list)), "ab");
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
    // an empty recognized element is dropped: it neither blanks the projected title nor pushes empty entries
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
