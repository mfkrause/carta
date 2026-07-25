//! The catalog of available syntax definitions and color themes.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
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
/// Both lookup entry points memoize their results (including misses) keyed by the query string
/// as given, so a document naming the same language on many code blocks pays the scan over the
/// bundled catalog once.
#[derive(Default)]
pub struct Registry {
    parsed: RefCell<BTreeMap<usize, Rc<Grammar>>>,
    user: Vec<UserDefinition>,
    /// Whether every directory definition's header has been read; set once a lookup needs metadata
    /// that a file stem cannot answer.
    headers_read: Cell<bool>,
    resolved: RefCell<BTreeMap<String, Option<Rc<Grammar>>>>,
    references: RefCell<BTreeMap<String, Option<Rc<Grammar>>>>,
}

/// A definition added at runtime, with the short name it resolves under.
struct UserDefinition {
    short: String,
    source: DefinitionSource,
}

/// Resolution metadata read from a definition's `<language>` element.
#[derive(Default)]
struct Metadata {
    name: String,
    extensions: Vec<String>,
    priority: i64,
    hidden: bool,
}

/// Where a runtime definition comes from, and how much of it has been read.
enum DefinitionSource {
    /// Supplied as text and parsed when it was added, so its metadata comes from the grammar.
    Parsed(Rc<Grammar>),
    /// A file in a grammar directory. Its header is read on the first lookup that needs metadata,
    /// and its rules are parsed only once it is the definition selected.
    File {
        path: PathBuf,
        metadata: OnceCell<Metadata>,
        grammar: OnceCell<Option<Rc<Grammar>>>,
    },
}

/// Bytes of a definition read when looking for its `<language>` element. A large internal entity
/// block can push the element well into the file, so the window is generous; a definition whose
/// element lies beyond it falls back to a full read.
const HEADER_WINDOW: usize = 16 * 1024;

impl UserDefinition {
    /// The definition's display name, reading its header if that has not happened yet.
    fn name(&self) -> &str {
        match &self.source {
            DefinitionSource::Parsed(grammar) => &grammar.name,
            DefinitionSource::File { metadata, .. } => {
                metadata.get().map_or("", |meta| meta.name.as_str())
            }
        }
    }

    fn extensions(&self) -> &[String] {
        match &self.source {
            DefinitionSource::Parsed(grammar) => &grammar.extensions,
            DefinitionSource::File { metadata, .. } => {
                metadata.get().map_or(&[][..], |meta| &meta.extensions)
            }
        }
    }

    fn priority(&self) -> i64 {
        match &self.source {
            DefinitionSource::Parsed(grammar) => grammar.priority,
            DefinitionSource::File { metadata, .. } => {
                metadata.get().map_or(0, |meta| meta.priority)
            }
        }
    }

    fn hidden(&self) -> bool {
        match &self.source {
            DefinitionSource::Parsed(grammar) => grammar.hidden,
            DefinitionSource::File { metadata, .. } => {
                metadata.get().is_some_and(|meta| meta.hidden)
            }
        }
    }

    /// The parsed grammar, reading and parsing the definition on first use. `None` when the file
    /// cannot be read or does not parse, so a broken definition simply does not resolve.
    fn grammar(&self) -> Option<Rc<Grammar>> {
        match &self.source {
            DefinitionSource::Parsed(grammar) => Some(Rc::clone(grammar)),
            DefinitionSource::File {
                path,
                grammar,
                metadata,
            } => grammar
                .get_or_init(|| {
                    let xml = std::fs::read_to_string(path).ok()?;
                    let parsed = parse_grammar(&xml).ok()?;
                    // A stem-answered lookup can select a definition before its header was read.
                    let _ = metadata.set(Metadata {
                        name: parsed.name.clone(),
                        extensions: parsed.extensions.clone(),
                        priority: parsed.priority,
                        hidden: parsed.hidden,
                    });
                    Some(Rc::new(parsed))
                })
                .clone(),
        }
    }

    /// Read this definition's header so its metadata can answer a lookup.
    fn read_header(&self) {
        let DefinitionSource::File { path, metadata, .. } = &self.source else {
            return;
        };
        if metadata.get().is_some() {
            return;
        }
        let _ = metadata.set(read_metadata(path).unwrap_or_default());
    }
}

/// Read a definition's `<language>` element from the head of `path`, falling back to the whole file
/// when a large entity block pushes the element past the window.
fn read_metadata(path: &Path) -> Option<Metadata> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut head = vec![0u8; HEADER_WINDOW];
    let mut filled = 0usize;
    while filled < head.len() {
        let read = file.read(head.get_mut(filled..)?).ok()?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    head.truncate(filled);
    let text = String::from_utf8_lossy(&head);
    if let Some(meta) = language_metadata(&text) {
        return Some(meta);
    }
    let whole = std::fs::read_to_string(path).ok()?;
    language_metadata(&whole)
}

/// Parse the attributes of a definition's `<language>` element into resolution metadata.
fn language_metadata(xml: &str) -> Option<Metadata> {
    let start = xml.find("<language")?;
    let after = xml.get(start + "<language".len()..)?;
    let body = after.get(..after.find('>')?)?;
    let mut meta = Metadata::default();
    for (key, value) in attributes(body) {
        match key {
            "name" => meta.name = value,
            "extensions" => {
                meta.extensions = value
                    .split(';')
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            "priority" => meta.priority = value.parse().unwrap_or(0),
            "hidden" => meta.hidden = matches!(value.as_str(), "1" | "true" | "True" | "TRUE"),
            _ => {}
        }
    }
    Some(meta)
}

/// The `key="value"` pairs in an element's attribute text, with the basic entities decoded.
fn attributes(body: &str) -> Vec<(&str, String)> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(equals) = rest.find('=') {
        let key = rest.get(..equals).map(str::trim).unwrap_or_default();
        let key = key.rsplit(|c: char| c.is_whitespace()).next().unwrap_or("");
        let after = rest.get(equals + 1..).unwrap_or("").trim_start();
        let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            rest = after;
            continue;
        };
        let after = after.get(quote.len_utf8()..).unwrap_or("");
        let Some(close) = after.find(quote) else {
            break;
        };
        let value = after.get(..close).unwrap_or("");
        if !key.is_empty() {
            out.push((key, unescape_basic(value)));
        }
        rest = after.get(close + quote.len_utf8()..).unwrap_or("");
    }
    out
}

/// Decode the five predefined XML entities, which is all a `<language>` attribute can carry.
fn unescape_basic(value: &str) -> String {
    if !value.contains('&') {
        return value.to_string();
    }
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
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

    /// Add a user-supplied definition; it overrides any bundled definition of the same name. Its
    /// short lookup name derives from the definition's language name.
    pub fn add_definition(&mut self, xml: &str) -> Result<String, ParseError> {
        self.add_definition_entry(xml, None)
    }

    /// Add a user-supplied definition from a file, where `stem` is the file's name without its
    /// extension. The stem provides the short lookup name, matching how bundled definitions
    /// resolve (`cpp.xml` answers to `cpp` even though its language name is `C++`).
    pub fn add_definition_with_stem(
        &mut self,
        xml: &str,
        stem: &str,
    ) -> Result<String, ParseError> {
        self.add_definition_entry(xml, Some(stem))
    }

    fn add_definition_entry(
        &mut self,
        xml: &str,
        stem: Option<&str>,
    ) -> Result<String, ParseError> {
        let grammar = parse_grammar(xml)?;
        let name = grammar.name.clone();
        let short = short_name(stem.unwrap_or(&name));
        self.user.push(UserDefinition {
            short,
            source: DefinitionSource::Parsed(Rc::new(grammar)),
        });
        self.resolved.borrow_mut().clear();
        self.references.borrow_mut().clear();
        Ok(name)
    }

    /// Register every `*.xml` in `directory` as a definition resolvable by its file stem, without
    /// opening any of them: a definition is read only when a lookup selects it or needs metadata a
    /// stem cannot answer. Returns how many were registered.
    ///
    /// Definitions registered here override bundled ones of the same name, and a directory added
    /// earlier wins a collision with one added later. A file that cannot be read, or does not parse,
    /// is ignored when it would be resolved.
    ///
    /// # Errors
    /// The [`std::io::Error`] from listing `directory`.
    pub fn add_directory(&mut self, directory: &Path) -> std::io::Result<usize> {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(directory)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "xml"))
            .collect();
        paths.sort();
        let added = paths.len();
        for path in paths {
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_string();
            self.user.push(UserDefinition {
                short: short_name(&stem),
                source: DefinitionSource::File {
                    path,
                    metadata: OnceCell::new(),
                    grammar: OnceCell::new(),
                },
            });
        }
        self.headers_read.set(false);
        self.resolved.borrow_mut().clear();
        self.references.borrow_mut().clear();
        Ok(added)
    }

    /// Read every directory definition's header, so a lookup by display name or file extension sees
    /// the same catalog an eagerly loaded registry would.
    fn read_headers(&self) {
        if self.headers_read.replace(true) {
            return;
        }
        for entry in &self.user {
            entry.read_header();
        }
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
        self.read_headers();
        // A definition that does not load is passed over, leaving the bundled catalog to answer.
        if let Some(grammar) = self
            .user
            .iter()
            .filter(|entry| entry.name().to_lowercase() == lower)
            .find_map(UserDefinition::grammar)
        {
            return Some(grammar);
        }
        let idx = BUNDLED
            .iter()
            .position(|b| b.name.to_lowercase() == lower)?;
        Some(self.load(idx))
    }

    fn by_short_name(&self, lower: &str) -> Option<Rc<Grammar>> {
        // A file stem answers without reading anything; a display-name match needs the headers.
        if let Some(grammar) = self
            .user
            .iter()
            .filter(|entry| entry.short == lower)
            .find_map(UserDefinition::grammar)
        {
            return Some(grammar);
        }
        self.read_headers();
        if let Some(grammar) = self
            .user
            .iter()
            .filter(|entry| entry.short == lower || entry.name().to_lowercase() == lower)
            .find_map(UserDefinition::grammar)
        {
            return Some(grammar);
        }
        let idx = BUNDLED.iter().position(|b| b.short == lower)?;
        Some(self.load(idx))
    }

    fn by_extension(&self, lower: &str) -> Option<Rc<Grammar>> {
        // When several definitions claim an extension, the highest priority wins; hidden helpers are
        // not selected by extension. User definitions take precedence over the bundled catalog.
        self.read_headers();
        let mut claimants: Vec<&UserDefinition> = self
            .user
            .iter()
            .filter(|entry| {
                !entry.hidden()
                    && entry
                        .extensions()
                        .iter()
                        .any(|glob| match_glob(glob, lower))
            })
            .collect();
        // A later registration wins a priority tie, so equal priorities are ordered latest-first.
        claimants.reverse();
        claimants.sort_by_key(|entry| -entry.priority());
        if let Some(grammar) = claimants.into_iter().find_map(UserDefinition::grammar) {
            return Some(grammar);
        }
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

    /// Read a definition from the runtime grammar pack and add it under its file stem, the way the
    /// CLI's grammar-directory loading does.
    fn add_from_pack(registry: &mut Registry, stem: &str) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("data/syntax-copyleft")
            .join(format!("{stem}.xml"));
        let xml = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        registry
            .add_definition_with_stem(&xml, stem)
            .unwrap_or_else(|error| panic!("parse {stem}: {error}"));
    }

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
        assert_eq!(
            reg.resolve("rust").map(|g| g.name.clone()).as_deref(),
            Some("Rust")
        );
        // Full display name (case-insensitive).
        assert!(reg.resolve("Rust").is_some());
        // Hyphenated stems only resolve without the hyphen.
        assert!(reg.resolve("fortranfree").is_some());
        assert!(reg.resolve("fortran-free").is_none());
    }

    #[test]
    fn pack_definitions_resolve_like_bundled_ones() {
        let mut reg = Registry::new();
        add_from_pack(&mut reg, "cpp");
        add_from_pack(&mut reg, "python");
        add_from_pack(&mut reg, "makefile");
        // Stem-derived short name, even though the language name is `C++`.
        assert_eq!(
            reg.resolve("cpp").map(|g| g.name.clone()).as_deref(),
            Some("C++")
        );
        // Full display name (case-insensitive).
        assert!(reg.resolve("C++").is_some());
        assert_eq!(
            reg.resolve("python").map(|g| g.name.clone()).as_deref(),
            Some("Python")
        );
        // The file-extension fallback consults user definitions (`makefile.*` is a Makefile glob).
        assert_eq!(
            reg.resolve("makefile.inc")
                .map(|g| g.name.clone())
                .as_deref(),
            Some("Makefile")
        );
    }

    #[test]
    fn honors_fixed_aliases() {
        let mut reg = Registry::new();
        add_from_pack(&mut reg, "cs");
        assert_eq!(
            reg.resolve("csharp").map(|g| g.name.clone()),
            reg.resolve("cs").map(|g| g.name.clone())
        );
        assert!(reg.resolve("csharp").is_some());
    }

    #[cfg(not(feature = "embed-copyleft-grammars"))]
    #[test]
    fn copyleft_grammars_are_not_embedded_by_default() {
        let reg = Registry::new();
        assert!(reg.resolve("cpp").is_none());
        assert!(reg.resolve("json").is_none());
        assert!(reg.resolve("rust").is_some());
    }

    #[cfg(feature = "embed-copyleft-grammars")]
    #[test]
    fn copyleft_grammars_are_embedded_with_the_feature() {
        let reg = Registry::new();
        assert!(reg.resolve("cpp").is_some());
        assert!(reg.resolve("json").is_some());
    }

    #[test]
    fn unknown_language_is_none() {
        let reg = Registry::new();
        assert!(reg.resolve("not-a-language").is_none());
        assert!(!reg.is_known("not-a-language"));
    }

    /// The runtime grammar pack, registered by listing alone.
    fn pack_directory() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data/syntax-copyleft")
    }

    #[test]
    fn directory_definitions_resolve_by_stem_name_and_extension() {
        let mut reg = Registry::new();
        let added = reg.add_directory(&pack_directory()).expect("list pack");
        assert!(added > 100, "expected the whole pack, saw {added}");

        // A file stem, answered without reading any other definition.
        assert_eq!(
            reg.resolve("cpp").map(|g| g.name.clone()).as_deref(),
            Some("C++")
        );
        // The display name, which is only known once headers are read.
        assert_eq!(
            reg.resolve("C++").map(|g| g.name.clone()).as_deref(),
            Some("C++")
        );
        // The file-extension fallback, which needs the extension lists.
        assert_eq!(
            reg.resolve("makefile.inc")
                .map(|g| g.name.clone())
                .as_deref(),
            Some("Makefile")
        );
        // A fixed alias still routes through the pack.
        assert_eq!(
            reg.resolve("csharp").map(|g| g.name.clone()),
            reg.resolve("cs").map(|g| g.name.clone())
        );
        assert!(reg.resolve("not-a-language").is_none());
    }

    #[test]
    fn a_directory_definition_that_does_not_parse_falls_back_to_the_bundled_one() {
        let directory = std::env::temp_dir().join(format!(
            "carta-registry-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&directory).expect("create temp directory");
        // `rust` is bundled, so a broken same-named definition must not hide it.
        std::fs::write(directory.join("rust.xml"), b"not xml at all").expect("write definition");

        let mut reg = Registry::new();
        reg.add_directory(&directory).expect("list directory");
        assert_eq!(
            reg.resolve("rust").map(|g| g.name.clone()).as_deref(),
            Some("Rust")
        );

        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn adding_a_missing_directory_reports_the_listing_error() {
        let mut reg = Registry::new();
        assert!(
            reg.add_directory(std::path::Path::new("/no/such/syntax/dir"))
                .is_err()
        );
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
