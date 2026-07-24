//! The tokenizer: walks a stack of contexts over source text, emitting classified [`Token`]s.
//!
//! For each line the current context's rules are tried in order; the first to match consumes text,
//! emits a token, and may switch contexts. When nothing matches, the context either falls through to
//! another context or consumes a run of ordinary text. Regular-expression rules match anchored at the
//! current position; compiled patterns and resolved keyword sets are cached across lines.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use fancy_regex::Regex;

use crate::grammar::Grammar;
use crate::registry::Registry;
use crate::token::SourceLine;

mod helpers;
mod tokenizer;

use helpers::{build_regex, split_lines};
use tokenizer::Tokenizer;

/// Tokenizes source code using a catalog of syntax definitions.
#[derive(Debug, Default)]
pub struct Highlighter {
    registry: Registry,
    regexes: RefCell<BTreeMap<RegexKey, Option<Rc<Regex>>>>,
    keyword_sets: RefCell<BTreeMap<String, BTreeMap<String, Rc<KeywordSet>>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RegexKey {
    pattern: String,
    insensitive: bool,
    minimal: bool,
}

#[derive(Debug)]
pub(crate) struct KeywordSet {
    words: BTreeSet<String>,
    case_sensitive: bool,
}

impl KeywordSet {
    fn contains(&self, word: &str) -> bool {
        if self.case_sensitive {
            return self.words.contains(word);
        }
        // An ASCII word with no uppercase letters is already its own lowercase form.
        if word
            .bytes()
            .all(|b| b.is_ascii() && !b.is_ascii_uppercase())
        {
            return self.words.contains(word);
        }
        self.words.contains(&word.to_lowercase())
    }
}

/// One entry on the context stack: the definition and context it names, and any captures carried in
/// from the rule that entered it.
#[derive(Debug, Clone)]
struct Frame {
    grammar: Rc<Grammar>,
    context: usize,
    captures: Vec<String>,
}

impl Highlighter {
    /// A highlighter over the bundled syntax definitions.
    #[must_use]
    pub fn new() -> Self {
        Highlighter::default()
    }

    /// The catalog backing this highlighter.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The catalog backing this highlighter, mutably (to register user definitions).
    pub fn registry_mut(&mut self) -> &mut Registry {
        &mut self.registry
    }

    /// Tokenize `code` as the given language, returning one [`SourceLine`] per line, or `None` if the
    /// language is unknown.
    pub fn highlight(&self, language: &str, code: &str) -> Option<Vec<SourceLine>> {
        let grammar = self.registry.resolve(language)?;
        Some(self.tokenize(grammar, code))
    }

    fn tokenize(&self, start: Rc<Grammar>, code: &str) -> Vec<SourceLine> {
        let mut state = Tokenizer::new(self, start);
        split_lines(code)
            .into_iter()
            .map(|line| state.tokenize_line(line))
            .collect()
    }

    fn compiled_regex(&self, key: &RegexKey) -> Option<Rc<Regex>> {
        if let Some(entry) = self.regexes.borrow().get(key) {
            return entry.clone();
        }
        let compiled = build_regex(key);
        self.regexes
            .borrow_mut()
            .insert(key.clone(), compiled.clone());
        compiled
    }

    fn keyword_set(&self, grammar: &Rc<Grammar>, list: &str) -> Rc<KeywordSet> {
        if let Some(set) = self
            .keyword_sets
            .borrow()
            .get(grammar.name.as_str())
            .and_then(|lists| lists.get(list))
        {
            return Rc::clone(set);
        }
        let mut words = BTreeSet::new();
        let case_sensitive = grammar.keywords.case_sensitive;
        self.collect_words(
            grammar,
            list,
            case_sensitive,
            &mut words,
            &mut BTreeSet::new(),
        );
        let set = Rc::new(KeywordSet {
            words,
            case_sensitive,
        });
        self.keyword_sets
            .borrow_mut()
            .entry(grammar.name.clone())
            .or_default()
            .insert(list.to_string(), Rc::clone(&set));
        set
    }

    fn collect_words(
        &self,
        grammar: &Rc<Grammar>,
        list: &str,
        case_sensitive: bool,
        out: &mut BTreeSet<String>,
        visited: &mut BTreeSet<(String, String)>,
    ) {
        let key = (grammar.name.clone(), list.to_string());
        if !visited.insert(key) {
            return;
        }
        if let Some(words) = grammar.keyword_lists.get(list) {
            for word in words {
                if word.is_empty() {
                    continue;
                }
                out.insert(if case_sensitive {
                    word.clone()
                } else {
                    word.to_lowercase()
                });
            }
        }
        for include in &grammar.keyword_includes {
            if include.target_list != list {
                continue;
            }
            if include.source_language == grammar.name {
                self.collect_words(grammar, &include.source_list, case_sensitive, out, visited);
            } else if let Some(source) = self.registry.resolve_reference(&include.source_language) {
                self.collect_words(&source, &include.source_list, case_sensitive, out, visited);
            }
        }
    }
}

#[cfg(test)]
mod tests;
