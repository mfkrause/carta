//! Turns a syntax-definition XML document into a [`Grammar`].
//!
//! The definition format allows the DOCTYPE to declare entities that are then referenced inside
//! rule patterns (`&int;`, `&float;`, …). Those are expanded up front, since standard XML tooling
//! leaves document-defined entities untouched. The expanded text is then read into a small element
//! tree and projected onto the grammar model.

use std::collections::BTreeMap;

use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::grammar::{
    Context, ContextSwitch, ContextTarget, Grammar, KeywordInclude, KeywordSettings, Matcher, Rule,
};
use crate::token::TokenKind;

/// A failure while reading a syntax definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The XML was malformed.
    Xml(String),
    /// The document had no `<language>` root.
    MissingLanguage,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Xml(msg) => write!(f, "malformed syntax definition: {msg}"),
            ParseError::MissingLanguage => write!(f, "syntax definition has no <language> root"),
        }
    }
}

impl std::error::Error for ParseError {}

/// A minimal XML element tree: enough to walk a syntax definition without a streaming state machine.
#[derive(Debug, Default)]
struct Node {
    name: String,
    attrs: BTreeMap<String, String>,
    text: String,
    children: Vec<Node>,
}

impl Node {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.get(key).map(String::as_str)
    }

    fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> {
        self.children.iter().filter(move |c| c.name == name)
    }

    fn first_child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }
}

/// Parse a syntax definition into a grammar.
pub(crate) fn parse_grammar(xml: &str) -> Result<Grammar, ParseError> {
    let entities = collect_entities(xml);
    let expanded = expand_entities(xml, &entities);
    let root = read_dom(&expanded)?;
    let language = root
        .iter()
        .find(|n| n.name == "language")
        .ok_or(ParseError::MissingLanguage)?;
    Ok(build_grammar(language))
}

fn build_grammar(language: &Node) -> Grammar {
    let name = language.attr("name").unwrap_or_default().to_string();
    let section = language.attr("section").unwrap_or_default().to_string();
    let extensions = split_list(language.attr("extensions").unwrap_or_default());
    let priority = language
        .attr("priority")
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    let hidden = language.attr("hidden").is_some_and(is_truthy);

    let highlighting = language.first_child("highlighting");
    let mut keyword_lists = std::collections::BTreeMap::new();
    let mut keyword_includes = Vec::new();
    let mut item_styles = std::collections::BTreeMap::new();
    let mut contexts = Vec::new();

    if let Some(hl) = highlighting {
        for list in hl.children_named("list") {
            let list_name = list.attr("name").unwrap_or_default().to_string();
            let mut words = Vec::new();
            for child in &list.children {
                match child.name.as_str() {
                    "item" => words.push(child.text.trim().to_string()),
                    "include" => {
                        let target = ContextTarget::parse(child.text.trim());
                        match target {
                            ContextTarget::Local(source_list) => {
                                keyword_includes.push(KeywordInclude {
                                    target_list: list_name.clone(),
                                    source_list,
                                    source_language: name.clone(),
                                });
                            }
                            ContextTarget::Foreign { language, context } => {
                                keyword_includes.push(KeywordInclude {
                                    target_list: list_name.clone(),
                                    source_list: context.unwrap_or_default(),
                                    source_language: language,
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
            keyword_lists.insert(list_name, words);
        }

        if let Some(item_datas) = hl.first_child("itemDatas") {
            for item in item_datas.children_named("itemData") {
                let item_name = item.attr("name").unwrap_or_default().to_string();
                let style = TokenKind::from_default_style(item.attr("defStyleNum").unwrap_or(""));
                item_styles.insert(item_name, style);
            }
        }

        if let Some(ctx_list) = hl.first_child("contexts") {
            for ctx in ctx_list.children_named("context") {
                contexts.push(build_context(ctx));
            }
        }
    }

    let keywords = language
        .first_child("general")
        .and_then(|g| g.first_child("keywords"))
        .map(build_keyword_settings)
        .unwrap_or_default();

    Grammar {
        name,
        section,
        extensions,
        alternative_names: Vec::new(),
        priority,
        hidden,
        keyword_lists,
        keyword_includes,
        contexts,
        keywords,
        item_styles,
    }
}

fn build_keyword_settings(node: &Node) -> KeywordSettings {
    KeywordSettings {
        case_sensitive: node.attr("casesensitive").is_none_or(is_truthy),
        weak_deliminators: node.attr("weakDeliminator").unwrap_or_default().to_string(),
        additional_deliminators: node
            .attr("additionalDeliminator")
            .unwrap_or_default()
            .to_string(),
    }
}

fn build_context(node: &Node) -> Context {
    let attribute = node.attr("attribute").unwrap_or_default().to_string();
    let line_end_context = ContextSwitch::parse(node.attr("lineEndContext").unwrap_or("#stay"));
    let line_begin_context = ContextSwitch::parse(node.attr("lineBeginContext").unwrap_or("#stay"));
    let line_empty_context = node.attr("lineEmptyContext").map(ContextSwitch::parse);
    let fallthrough_context =
        ContextSwitch::parse(node.attr("fallthroughContext").unwrap_or("#stay"));
    // A real fallthrough target enables fallthrough on its own; the boolean form (with no target)
    // is honored too, taking a single pop.
    let fallthrough = node.attr("fallthrough").is_some_and(is_truthy)
        || !(fallthrough_context.pops == 0 && fallthrough_context.push.is_none());
    let dynamic = node.attr("dynamic").is_some_and(is_truthy);
    let rules = node.children.iter().filter_map(build_rule).collect();
    Context {
        name: node.attr("name").unwrap_or_default().to_string(),
        attribute,
        line_end_context,
        line_begin_context,
        line_empty_context,
        fallthrough,
        fallthrough_context,
        dynamic,
        rules,
    }
}

fn build_rule(node: &Node) -> Option<Rule> {
    let matcher = build_matcher(node)?;
    let attribute = node.attr("attribute").map(str::to_string);
    let context = ContextSwitch::parse(node.attr("context").unwrap_or("#stay"));
    let look_ahead = node.attr("lookAhead").is_some_and(is_truthy);
    let first_non_space = node.attr("firstNonSpace").is_some_and(is_truthy);
    let column = node.attr("column").and_then(|c| c.parse().ok());
    let dynamic = node.attr("dynamic").is_some_and(is_truthy);
    let children = node.children.iter().filter_map(build_rule).collect();
    Some(Rule {
        matcher,
        attribute,
        context,
        look_ahead,
        first_non_space,
        column,
        dynamic,
        children,
    })
}

fn build_matcher(node: &Node) -> Option<Matcher> {
    let insensitive = node.attr("insensitive").is_some_and(is_truthy);
    let first_char = |key: &str| node.attr(key).and_then(|s| s.chars().next());
    Some(match node.name.as_str() {
        "DetectChar" => Matcher::DetectChar(first_char("char")?),
        "Detect2Chars" => Matcher::Detect2Chars(first_char("char")?, first_char("char1")?),
        "AnyChar" => Matcher::AnyChar(node.attr("String").unwrap_or_default().to_string()),
        "StringDetect" => Matcher::StringDetect {
            text: node.attr("String").unwrap_or_default().to_string(),
            insensitive,
        },
        "WordDetect" => Matcher::WordDetect {
            text: node.attr("String").unwrap_or_default().to_string(),
            insensitive,
        },
        "RegExpr" => Matcher::RegExpr {
            pattern: node.attr("String").unwrap_or_default().to_string(),
            insensitive,
            minimal: node.attr("minimal").is_some_and(is_truthy),
        },
        "keyword" => Matcher::Keyword(node.attr("String").unwrap_or_default().to_string()),
        "Int" => Matcher::Int,
        "Float" => Matcher::Float,
        "HlCOct" => Matcher::HlCOct,
        "HlCHex" => Matcher::HlCHex,
        "HlCStringChar" => Matcher::HlCStringChar,
        "HlCChar" => Matcher::HlCChar,
        "RangeDetect" => Matcher::RangeDetect {
            start: first_char("char")?,
            end: first_char("char1")?,
        },
        "DetectSpaces" => Matcher::DetectSpaces,
        "DetectIdentifier" => Matcher::DetectIdentifier,
        "LineContinue" => Matcher::LineContinue(first_char("char").unwrap_or('\\')),
        "IncludeRules" => Matcher::IncludeRules {
            target: ContextTarget::parse(node.attr("context").unwrap_or_default()),
            include_attribute: node.attr("includeAttrib").is_some_and(is_truthy),
        },
        _ => return None,
    })
}

// --- entity handling ---------------------------------------------------------

/// Collect `<!ENTITY name "value">` declarations from the DOCTYPE internal subset.
fn collect_entities(xml: &str) -> BTreeMap<String, String> {
    let mut entities = BTreeMap::new();
    let bytes = xml.as_bytes();
    let mut search = xml;
    while let Some(rel) = search.find("<!ENTITY") {
        let after = &search[rel + "<!ENTITY".len()..];
        let after = after.trim_start();
        // The name runs until whitespace.
        let name_end = after
            .find(|c: char| c.is_whitespace())
            .unwrap_or(after.len());
        let name = &after[..name_end];
        let rest = after[name_end..].trim_start();
        let mut chars = rest.char_indices();
        if let Some((_, quote)) = chars.next()
            && (quote == '"' || quote == '\'')
        {
            let value_start = quote.len_utf8();
            if let Some(end_rel) = rest[value_start..].find(quote) {
                let value = &rest[value_start..value_start + end_rel];
                if !name.is_empty() {
                    entities.insert(name.to_string(), value.to_string());
                }
            }
        }
        // Advance past this declaration.
        let consumed = bytes.len() - search.len() + rel + "<!ENTITY".len();
        if consumed >= xml.len() {
            break;
        }
        search = &xml[consumed..];
    }
    entities
}

/// Replace `&name;` references with their entity values, resolving nested references up to a bounded
/// depth so a cyclic declaration cannot loop forever.
fn expand_entities(xml: &str, entities: &BTreeMap<String, String>) -> String {
    if entities.is_empty() {
        return xml.to_string();
    }
    let mut text = xml.to_string();
    for _ in 0..16 {
        let (next, changed) = expand_once(&text, entities);
        text = next;
        if !changed {
            break;
        }
    }
    text
}

fn expand_once(text: &str, entities: &BTreeMap<String, String>) -> (String, bool) {
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        if let Some(semi) = after.find(';') {
            let name = &after[..semi];
            if let Some(value) = entities.get(name) {
                out.push_str(value);
                changed = true;
                rest = &after[semi + 1..];
                continue;
            }
        }
        out.push('&');
        rest = after;
    }
    out.push_str(rest);
    (out, changed)
}

// --- DOM reader --------------------------------------------------------------

fn read_dom(xml: &str) -> Result<Vec<Node>, ParseError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().expand_empty_elements = false;
    reader.config_mut().trim_text(false);
    let mut stack: Vec<Node> = vec![Node::default()];
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                stack.push(element_from_start(&e));
            }
            Ok(Event::Empty(e)) => {
                let node = element_from_start(&e);
                if let Some(top) = stack.last_mut() {
                    top.children.push(node);
                }
            }
            Ok(Event::End(_)) => {
                if stack.len() > 1
                    && let Some(node) = stack.pop()
                    && let Some(top) = stack.last_mut()
                {
                    top.children.push(node);
                }
            }
            Ok(Event::Text(e)) => {
                if let Some(top) = stack.last_mut() {
                    let raw = String::from_utf8_lossy(e.as_ref());
                    top.text.push_str(&unescape_xml(&raw));
                }
            }
            Ok(Event::CData(e)) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&String::from_utf8_lossy(e.as_ref()));
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(ParseError::Xml(e.to_string())),
        }
    }
    Ok(stack
        .into_iter()
        .next()
        .map(|n| n.children)
        .unwrap_or_default())
}

fn element_from_start(e: &quick_xml::events::BytesStart<'_>) -> Node {
    let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
    let mut attrs = BTreeMap::new();
    for attr in e.attributes().flatten() {
        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
        let value = unescape_xml(&String::from_utf8_lossy(attr.value.as_ref()));
        attrs.insert(key, value);
    }
    Node {
        name,
        attrs,
        text: String::new(),
        children: Vec::new(),
    }
}

/// Resolve the standard XML entities and numeric character references, leaving any other `&…;`
/// sequence untouched so malformed input cannot abort a parse.
fn unescape_xml(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        if let Some(semi) = after.find(';') {
            let name = &after[..semi];
            if let Some(resolved) = resolve_reference(name) {
                out.push(resolved);
                rest = &after[semi + 1..];
                continue;
            }
        }
        out.push('&');
        rest = after;
    }
    out.push_str(rest);
    out
}

fn resolve_reference(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            let code =
                if let Some(hex) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
                    u32::from_str_radix(hex, 16).ok()?
                } else if let Some(dec) = name.strip_prefix('#') {
                    dec.parse().ok()?
                } else {
                    return None;
                };
            char::from_u32(code)
        }
    }
}

// --- helpers -----------------------------------------------------------------

fn split_list(raw: &str) -> Vec<String> {
    raw.split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn is_truthy(value: &str) -> bool {
    matches!(value.trim(), "1" | "true" | "TRUE" | "True")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn expands_nested_entities() {
        let mut entities = BTreeMap::new();
        entities.insert("int".to_string(), "[0-9]+".to_string());
        entities.insert("float".to_string(), "&int;\\.&int;".to_string());
        let expanded = expand_entities("x=&float;", &entities);
        assert_eq!(expanded, "x=[0-9]+\\.[0-9]+");
    }

    #[test]
    fn unknown_entity_passes_through() {
        assert_eq!(unescape_xml("a &amp; b &weird; c"), "a & b &weird; c");
    }

    #[test]
    fn resolves_numeric_references() {
        assert_eq!(unescape_xml("&#37;&#x25;"), "%%");
    }

    #[test]
    fn parses_a_small_grammar() {
        let xml = r##"<?xml version="1.0"?>
<!DOCTYPE language [ <!ENTITY digit "[0-9]"> ]>
<language name="Toy" section="Other" extensions="*.toy" priority="3">
  <highlighting>
    <list name="kw"><item>if</item><item>else</item></list>
    <contexts>
      <context name="Normal" attribute="Normal Text" lineEndContext="#stay">
        <keyword attribute="Keyword" context="#stay" String="kw"/>
        <RegExpr attribute="Number" context="#stay" String="&digit;+"/>
        <DetectChar attribute="String" context="Str" char="&quot;"/>
      </context>
      <context name="Str" attribute="String" lineEndContext="#pop">
        <DetectChar attribute="String" context="#pop" char="&quot;"/>
      </context>
    </contexts>
    <itemDatas>
      <itemData name="Normal Text" defStyleNum="dsNormal"/>
      <itemData name="Keyword" defStyleNum="dsKeyword"/>
      <itemData name="Number" defStyleNum="dsDecVal"/>
      <itemData name="String" defStyleNum="dsString"/>
    </itemDatas>
  </highlighting>
</language>"##;
        let g = parse_grammar(xml).expect("parse");
        assert_eq!(g.name, "Toy");
        assert_eq!(g.extensions, vec!["*.toy"]);
        assert_eq!(g.priority, 3);
        assert_eq!(g.contexts.len(), 2);
        assert_eq!(g.keyword_lists.get("kw").map(Vec::len), Some(2));
        assert_eq!(g.item_styles.get("Keyword"), Some(&TokenKind::Keyword));
        // The entity inside the RegExpr pattern is expanded.
        let normal = &g.contexts[0];
        let has_expanded = normal.rules.iter().any(|r| {
            matches!(
                &r.matcher,
                Matcher::RegExpr { pattern, .. } if pattern == "[0-9]+"
            )
        });
        assert!(has_expanded);
        // The &quot; char attribute resolves to a double quote.
        let has_quote = normal
            .rules
            .iter()
            .any(|r| matches!(r.matcher, Matcher::DetectChar('"')));
        assert!(has_quote);
    }

    #[test]
    fn reads_language_metadata() {
        let xml = r#"<language name="C++" section="Sources" extensions="*.cpp;*.h" priority="5" hidden="false"><highlighting/></language>"#;
        let g = parse_grammar(xml).expect("parse");
        assert_eq!(g.name, "C++");
        assert_eq!(g.extensions, vec!["*.cpp", "*.h"]);
        assert_eq!(g.priority, 5);
        assert!(!g.hidden);
    }
}
