//! The catalog of available syntax definitions and color themes.
//!
//! Bundled definitions are embedded compressed and decompressed and parsed on first use, then
//! cached. User-supplied definitions (from `--syntax-definition`) take precedence over bundled ones
//! that share a name. Resolution follows the definition format's documented order: full name, then
//! short name, then file extension.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::grammar::Grammar;
use crate::parse::{ParseError, parse_grammar};
use crate::style::{Error as StyleError, Theme};

include!(concat!(env!("OUT_DIR"), "/bundled.rs"));

/// The authoritative list of listable language short names, as vendored data.
const LANGUAGE_LIST: &str = include_str!("../data/languages.txt");

/// The built-in color themes, embedded verbatim.
const STYLES: &[(&str, &str)] = &[
    ("pygments", include_str!("../data/styles/pygments.theme")),
    ("tango", include_str!("../data/styles/tango.theme")),
    ("espresso", include_str!("../data/styles/espresso.theme")),
    ("zenburn", include_str!("../data/styles/zenburn.theme")),
    ("kate", include_str!("../data/styles/kate.theme")),
    (
        "monochrome",
        include_str!("../data/styles/monochrome.theme"),
    ),
    (
        "breezedark",
        include_str!("../data/styles/breezedark.theme"),
    ),
    ("haddock", include_str!("../data/styles/haddock.theme")),
];

/// A catalog of syntax definitions, resolving names to parsed grammars on demand.
///
/// Both lookup entry points memoize their results — including misses — keyed by the query string
/// as given, so a document naming the same language on many code blocks pays the scan over the
/// bundled catalog once.
#[derive(Default)]
pub struct Registry {
    parsed: RefCell<BTreeMap<usize, Rc<Grammar>>>,
    user: Vec<Rc<Grammar>>,
    resolved: RefCell<BTreeMap<String, Option<Rc<Grammar>>>>,
    references: RefCell<BTreeMap<String, Option<Rc<Grammar>>>>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field("user_definitions", &self.user.len())
            .finish_non_exhaustive()
    }
}

impl Registry {
    /// A registry over only the bundled definitions.
    #[must_use]
    pub fn new() -> Self {
        Registry::default()
    }

    /// Add a user-supplied definition; it overrides any bundled definition of the same name.
    pub fn add_definition(&mut self, xml: &str) -> Result<String, ParseError> {
        let grammar = parse_grammar(xml)?;
        let name = grammar.name.clone();
        self.user.push(Rc::new(grammar));
        self.resolved.borrow_mut().clear();
        self.references.borrow_mut().clear();
        Ok(name)
    }

    /// The listable language short names, in the order they are published.
    pub fn languages(&self) -> Vec<String> {
        LANGUAGE_LIST
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// Whether a code-block language string resolves to a definition.
    pub fn is_known(&self, lang: &str) -> bool {
        self.resolve(lang).is_some()
    }

    /// Resolve a code-block language string to a grammar, following the documented lookup order and
    /// the format's fixed aliases.
    pub fn resolve(&self, lang: &str) -> Option<Rc<Grammar>> {
        if let Some(hit) = self.resolved.borrow().get(lang) {
            return hit.clone();
        }
        let lower = lang.to_lowercase();
        let result = match lower.as_str() {
            "csharp" => self.resolve("cs"),
            "fortran" => self.resolve("for"),
            _ => self
                .by_full_name(&lower)
                .or_else(|| self.by_short_name(&lower))
                .or_else(|| self.by_extension(&lower)),
        };
        self.resolved
            .borrow_mut()
            .insert(lang.to_string(), result.clone());
        result
    }

    /// Resolve a cross-definition reference, which addresses a definition by its full name (or, as a
    /// fallback, its short name).
    pub fn resolve_reference(&self, name: &str) -> Option<Rc<Grammar>> {
        if let Some(hit) = self.references.borrow().get(name) {
            return hit.clone();
        }
        let lower = name.to_lowercase();
        let result = self
            .by_full_name(&lower)
            .or_else(|| self.by_short_name(&lower));
        self.references
            .borrow_mut()
            .insert(name.to_string(), result.clone());
        result
    }

    fn by_full_name(&self, lower: &str) -> Option<Rc<Grammar>> {
        if let Some(g) = self.user.iter().find(|g| g.name.to_lowercase() == lower) {
            return Some(Rc::clone(g));
        }
        let idx = BUNDLED
            .iter()
            .position(|b| b.name.to_lowercase() == lower)?;
        Some(self.load(idx))
    }

    fn by_short_name(&self, lower: &str) -> Option<Rc<Grammar>> {
        if let Some(g) = self
            .user
            .iter()
            .find(|g| short_name(&g.name) == lower || g.name.to_lowercase() == lower)
        {
            return Some(Rc::clone(g));
        }
        let idx = BUNDLED.iter().position(|b| b.short == lower)?;
        Some(self.load(idx))
    }

    fn by_extension(&self, lower: &str) -> Option<Rc<Grammar>> {
        // When several definitions claim an extension, the highest priority wins; hidden helpers are
        // not selected by extension.
        let idx = BUNDLED
            .iter()
            .enumerate()
            .filter(|(_, b)| !b.hidden && b.extensions.iter().any(|glob| match_glob(glob, lower)))
            .max_by_key(|(_, b)| b.priority)
            .map(|(i, _)| i)?;
        Some(self.load(idx))
    }

    fn load(&self, idx: usize) -> Rc<Grammar> {
        if let Some(g) = self.parsed.borrow().get(&idx) {
            return Rc::clone(g);
        }
        let grammar = BUNDLED
            .get(idx)
            .and_then(|b| miniz_oxide::inflate::decompress_to_vec(b.data).ok())
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .and_then(|xml| parse_grammar(&xml).ok())
            .unwrap_or_else(empty_grammar);
        let shared = Rc::new(grammar);
        self.parsed.borrow_mut().insert(idx, Rc::clone(&shared));
        shared
    }
}

/// Retrieve a built-in theme by name.
#[must_use]
pub fn builtin_style(name: &str) -> Option<Result<Theme, StyleError>> {
    let lower = name.to_lowercase();
    STYLES
        .iter()
        .find(|(n, _)| *n == lower)
        .map(|(_, json)| Theme::from_json(json.as_bytes()))
}

/// The names of the built-in themes, in published order.
#[must_use]
pub fn style_names() -> Vec<String> {
    STYLES.iter().map(|(n, _)| (*n).to_string()).collect()
}

fn empty_grammar() -> Grammar {
    Grammar {
        name: String::new(),
        section: String::new(),
        extensions: Vec::new(),
        alternative_names: Vec::new(),
        priority: 0,
        hidden: false,
        keyword_lists: std::collections::BTreeMap::new(),
        keyword_includes: Vec::new(),
        contexts: Vec::new(),
        keywords: crate::grammar::KeywordSettings::default(),
        item_styles: std::collections::BTreeMap::new(),
    }
}

fn short_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Match a filename against a glob that may contain `*` wildcards.
fn match_glob(glob: &str, name: &str) -> bool {
    fn helper(pat: &[u8], text: &[u8]) -> bool {
        match pat.split_first() {
            None => text.is_empty(),
            Some((b'*', rest)) => {
                (0..=text.len()).any(|i| text.get(i..).is_some_and(|tail| helper(rest, tail)))
            }
            Some((p, rest)) => match text.split_first() {
                Some((t, trest)) if t == p => helper(rest, trest),
                _ => false,
            },
        }
    }
    helper(glob.as_bytes(), name.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_expected_languages() {
        let reg = Registry::new();
        let langs = reg.languages();
        assert!(langs.contains(&"cpp".to_string()));
        assert!(langs.contains(&"bash".to_string()));
        assert!(langs.iter().all(|l| l == &l.to_lowercase()));
        // The hidden helper is resolvable but not listed.
        assert!(!langs.contains(&"alert".to_string()));
    }

    #[test]
    fn resolves_by_short_and_full_name() {
        let reg = Registry::new();
        assert!(reg.resolve("cpp").is_some());
        assert_eq!(
            reg.resolve("cpp").map(|g| g.name.clone()).as_deref(),
            Some("C++")
        );
        // Full display name (case-insensitive).
        assert!(reg.resolve("C++").is_some());
        // Hyphenated stems only resolve without the hyphen.
        assert!(reg.resolve("fortranfree").is_some());
        assert!(reg.resolve("fortran-free").is_none());
    }

    #[test]
    fn honors_fixed_aliases() {
        let reg = Registry::new();
        assert_eq!(
            reg.resolve("csharp").map(|g| g.name.clone()),
            reg.resolve("cs").map(|g| g.name.clone())
        );
        assert!(reg.resolve("csharp").is_some());
    }

    #[test]
    fn unknown_language_is_none() {
        let reg = Registry::new();
        assert!(reg.resolve("not-a-language").is_none());
        assert!(!reg.is_known("not-a-language"));
    }

    #[test]
    fn builtin_styles_load() {
        for name in style_names() {
            assert!(builtin_style(&name).expect("known").is_ok());
        }
        assert!(builtin_style("nonexistent").is_none());
    }

    #[test]
    fn glob_matching() {
        assert!(match_glob("*.c", "foo.c"));
        assert!(!match_glob("*.c", "foo.h"));
        assert!(match_glob("*.tar.gz", "a.tar.gz"));
    }
}
