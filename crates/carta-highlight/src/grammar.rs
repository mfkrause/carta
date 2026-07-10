//! The in-memory model of a parsed syntax definition: its metadata, keyword lists, contexts, and the
//! ordered rules that drive tokenization.
//!
//! A definition is a small state machine. Tokenizing walks a stack of [`Context`]s; within the
//! current context each [`Rule`] is tried in order, and the first to match consumes text, emits a
//! token, and may switch contexts. Cross-definition references (embedded languages, shared keyword
//! lists) are left symbolic here and resolved against the registry while tokenizing.

use std::collections::BTreeMap;

use crate::token::TokenKind;

/// The characters that separate words by default, before a definition's `weakDeliminator` /
/// `additionalDeliminator` adjustments. Fixed by the syntax-definition format.
pub(crate) const STANDARD_DELIMITERS: &str = " \n\t.():!+,-<=>%&*/;?[]^{|}~\\";

/// A fully parsed syntax definition.
#[derive(Debug, Clone)]
pub struct Grammar {
    /// Display name, e.g. `C++`. Cross-definition references address a definition by this name.
    pub name: String,
    /// The menu section the definition belongs to, e.g. `Sources`.
    pub section: String,
    /// File-name globs the definition claims, e.g. `*.c`.
    pub extensions: Vec<String>,
    /// Additional names the definition answers to.
    pub alternative_names: Vec<String>,
    /// Selection priority when several definitions claim the same extension; higher wins.
    pub priority: i64,
    /// Whether the definition is a helper not offered in language listings.
    pub hidden: bool,
    /// Named keyword lists, each a set of literal words.
    pub keyword_lists: BTreeMap<String, Vec<String>>,
    /// Keyword lists this definition pulls in from other definitions (`list##Language`), applied
    /// after parsing all definitions.
    pub keyword_includes: Vec<KeywordInclude>,
    /// The contexts, in declaration order. The first is the entry context.
    pub contexts: Vec<Context>,
    /// Default word-delimiter behavior for `keyword`/`WordDetect` matching.
    pub keywords: KeywordSettings,
    /// Maps an `itemData` name to the token kind it paints, for resolving rule attributes.
    pub item_styles: BTreeMap<String, TokenKind>,
}

/// A keyword list pulled from another definition's list.
#[derive(Debug, Clone)]
pub struct KeywordInclude {
    /// The list in this definition that receives the imported words.
    pub target_list: String,
    /// The name of the list to import.
    pub source_list: String,
    /// The definition to import from.
    pub source_language: String,
}

/// Word-boundary configuration for keyword matching.
#[derive(Debug, Clone)]
pub struct KeywordSettings {
    /// Whether keyword matching is case-sensitive.
    pub case_sensitive: bool,
    /// Delimiter characters to remove from the standard set.
    pub weak_deliminators: String,
    /// Delimiter characters to add to the standard set.
    pub additional_deliminators: String,
}

impl Default for KeywordSettings {
    fn default() -> Self {
        KeywordSettings {
            case_sensitive: true,
            weak_deliminators: String::new(),
            additional_deliminators: String::new(),
        }
    }
}

impl KeywordSettings {
    /// Whether `c` acts as a word delimiter under these settings.
    pub fn is_delimiter(&self, c: char) -> bool {
        if self.weak_deliminators.contains(c) {
            return false;
        }
        if self.additional_deliminators.contains(c) {
            return true;
        }
        STANDARD_DELIMITERS.contains(c)
    }
}

/// One state of the tokenizer: a default token attribute, an ordered rule list, and the transitions
/// taken when a line begins, ends, or is empty.
#[derive(Debug, Clone)]
#[allow(clippy::struct_field_names)]
pub struct Context {
    /// The context's name, addressed by rule transitions.
    pub name: String,
    /// The `itemData` name painting text this context consumes without a more specific rule.
    pub attribute: String,
    /// The transition applied at end of line.
    pub line_end_context: ContextSwitch,
    /// The transition applied at start of line.
    pub line_begin_context: ContextSwitch,
    /// The transition applied for an empty line, if distinct from `line_end_context`.
    pub line_empty_context: Option<ContextSwitch>,
    /// When set, if no rule matches, take `fallthrough_context` instead of consuming a character.
    pub fallthrough: bool,
    /// The transition taken on fallthrough.
    pub fallthrough_context: ContextSwitch,
    /// Whether this context substitutes captures from the rule that entered it.
    pub dynamic: bool,
    /// The rules tried, in order.
    pub rules: Vec<Rule>,
}

/// One matching rule within a context.
#[derive(Debug, Clone)]
pub struct Rule {
    /// What text the rule matches.
    pub matcher: Matcher,
    /// The `itemData` name painting the matched text; falls back to the context attribute when absent.
    pub attribute: Option<String>,
    /// The context transition applied after a match.
    pub context: ContextSwitch,
    /// When set, the rule classifies without consuming: the match is examined but position does not
    /// advance past it (used to steer context switches).
    pub look_ahead: bool,
    /// When set, the rule only matches if the position is at the first non-space of the line.
    pub first_non_space: bool,
    /// When set, the rule only matches at this zero-based column.
    pub column: Option<usize>,
    /// When set, `%N` placeholders in the matcher are filled from the entering context's captures.
    pub dynamic: bool,
    /// Child rules tried immediately after this rule matches, extending the match.
    pub children: Vec<Rule>,
}

/// The text a rule matches.
#[derive(Debug, Clone)]
pub enum Matcher {
    /// A single literal character.
    DetectChar(char),
    /// Two literal characters in sequence.
    Detect2Chars(char, char),
    /// Any one character from a set.
    AnyChar(String),
    /// A literal string.
    StringDetect { text: String, insensitive: bool },
    /// A literal string bounded by word delimiters.
    WordDetect { text: String, insensitive: bool },
    /// A regular expression anchored at the current position.
    RegExpr {
        pattern: String,
        insensitive: bool,
        minimal: bool,
    },
    /// A word from a named keyword list.
    Keyword(String),
    /// A decimal integer.
    Int,
    /// A floating-point number.
    Float,
    /// A C-style octal integer.
    HlCOct,
    /// A C-style hexadecimal integer.
    HlCHex,
    /// A C-style character escape inside a string.
    HlCStringChar,
    /// A C-style character literal.
    HlCChar,
    /// A region opened and closed by single characters.
    RangeDetect { start: char, end: char },
    /// A run of whitespace.
    DetectSpaces,
    /// An identifier.
    DetectIdentifier,
    /// A line-continuation character at end of line.
    LineContinue(char),
    /// The rules of another context, spliced in place.
    IncludeRules {
        target: ContextTarget,
        include_attribute: bool,
    },
    /// An element the tokenizer does not act on.
    Unsupported,
}

/// A context transition: how many contexts to leave, then an optional one to enter.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContextSwitch {
    /// Number of contexts to pop off the stack.
    pub pops: usize,
    /// A context to push after popping, if any.
    pub push: Option<ContextTarget>,
}

/// A context named for a transition or inclusion, in this definition or another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextTarget {
    /// A context in the current definition.
    Local(String),
    /// A context in another definition; `None` names that definition's entry context.
    Foreign {
        language: String,
        context: Option<String>,
    },
}

impl ContextSwitch {
    /// Parse a context-switch attribute such as `#stay`, `#pop`, `#pop#pop!Name`, `Name`, or
    /// `Name##Language`.
    pub fn parse(raw: &str) -> Self {
        let mut rest = raw.trim();
        if rest.is_empty() || rest == "#stay" {
            return ContextSwitch::default();
        }
        let mut pops = 0usize;
        while let Some(after) = rest.strip_prefix("#pop") {
            pops = pops.saturating_add(1);
            rest = after;
        }
        rest = rest.strip_prefix('!').unwrap_or(rest);
        let push = if rest.is_empty() || rest == "#stay" {
            None
        } else {
            Some(ContextTarget::parse(rest))
        };
        ContextSwitch { pops, push }
    }
}

impl ContextTarget {
    /// Parse a context reference, recognizing the `Context##Language` cross-definition form.
    pub fn parse(raw: &str) -> Self {
        if let Some(idx) = raw.find("##") {
            let context = &raw[..idx];
            let language = &raw[idx + 2..];
            ContextTarget::Foreign {
                language: language.to_string(),
                context: if context.is_empty() {
                    None
                } else {
                    Some(context.to_string())
                },
            }
        } else {
            ContextTarget::Local(raw.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stay_and_empty() {
        assert_eq!(ContextSwitch::parse("#stay"), ContextSwitch::default());
        assert_eq!(ContextSwitch::parse(""), ContextSwitch::default());
    }

    #[test]
    fn parses_pops() {
        assert_eq!(
            ContextSwitch::parse("#pop"),
            ContextSwitch {
                pops: 1,
                push: None
            }
        );
        assert_eq!(
            ContextSwitch::parse("#pop#pop#pop"),
            ContextSwitch {
                pops: 3,
                push: None
            }
        );
    }

    #[test]
    fn parses_pop_then_push() {
        assert_eq!(
            ContextSwitch::parse("#pop#pop!Normal"),
            ContextSwitch {
                pops: 2,
                push: Some(ContextTarget::Local("Normal".to_string()))
            }
        );
    }

    #[test]
    fn parses_local_and_foreign() {
        assert_eq!(
            ContextSwitch::parse("Normal"),
            ContextSwitch {
                pops: 0,
                push: Some(ContextTarget::Local("Normal".to_string()))
            }
        );
        assert_eq!(
            ContextTarget::parse("Comment##Bash"),
            ContextTarget::Foreign {
                language: "Bash".to_string(),
                context: Some("Comment".to_string())
            }
        );
        assert_eq!(
            ContextTarget::parse("##CSS"),
            ContextTarget::Foreign {
                language: "CSS".to_string(),
                context: None
            }
        );
    }

    #[test]
    fn delimiter_adjustments_apply() {
        let mut k = KeywordSettings::default();
        assert!(k.is_delimiter(' '));
        assert!(!k.is_delimiter('a'));
        k.additional_deliminators = "$".to_string();
        assert!(k.is_delimiter('$'));
        k.weak_deliminators = ".".to_string();
        assert!(!k.is_delimiter('.'));
    }
}
