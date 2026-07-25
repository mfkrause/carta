use super::FirstBytes;

fn admits_only(pattern: &str, allowed: &str) -> bool {
    let set = FirstBytes::of_pattern(pattern, false);
    (0u8..=127).all(|byte| set.admits(byte) == allowed.as_bytes().contains(&byte))
}

#[test]
fn literal_prefix_admits_one_byte() {
    assert!(admits_only("abc", "a"));
    assert!(admits_only(r"\.foo", "."));
}

#[test]
fn character_class_is_read_exactly() {
    assert!(admits_only("[abc]x", "abc"));
    assert!(admits_only("[a-c]x", "abc"));
    assert!(admits_only(r"[0-9]+", "0123456789"));
    assert!(admits_only("[[:digit:]]", "0123456789"));
}

#[test]
fn alternation_unions_its_branches() {
    assert!(admits_only("foo|bar", "fb"));
    assert!(admits_only("(?:a|b)|c", "abc"));
}

#[test]
fn optional_and_starred_elements_fall_through() {
    assert!(admits_only("a?b", "ab"));
    assert!(admits_only("a*b", "ab"));
    assert!(admits_only("[xy]{0,3}z", "xyz"));
    // A `+` binds the atom, so only the atom can lead.
    assert!(admits_only("a+b", "a"));
    assert!(admits_only("a{2,4}b", "a"));
}

#[test]
fn word_boundaries_and_anchors_are_transparent() {
    assert!(admits_only(r"\bfoo", "f"));
    assert!(admits_only(r"^\Aq", "q"));
}

#[test]
fn look_around_does_not_constrain_the_first_byte() {
    // The assertion consumes nothing, so the element after it leads.
    assert!(admits_only("(?=x)y", "y"));
    assert!(admits_only("(?!x)y", "y"));
}

#[test]
fn negated_class_admits_every_non_ascii_lead() {
    let set = FirstBytes::of_pattern("[^abc]", false);
    assert!(!set.admits(b'a'));
    assert!(set.admits(b'z'));
    assert!((0x80u8..=0xFF).all(|byte| set.admits(byte)));
}

#[test]
fn non_ascii_literal_admits_its_lead_byte() {
    let set = FirstBytes::of_pattern("ü", false);
    assert!(set.admits(0xC3));
    assert!(!set.admits(b'u'));
}

#[test]
fn case_insensitive_admits_both_cases() {
    let set = FirstBytes::of_pattern("foo", true);
    assert!(set.admits(b'f'));
    assert!(set.admits(b'F'));
}

#[test]
fn inline_flag_group_enables_insensitivity() {
    let set = FirstBytes::of_pattern("(?i)foo", false);
    assert!(set.admits(b'f'));
    assert!(set.admits(b'F'));
}

#[test]
fn unmodeled_constructs_widen_to_any() {
    // Back-references and Unicode properties are not modeled.
    for pattern in [r"(a)\1", r"\p{L}+", r"[[:^alpha:]]", r"\k<n>"] {
        let set = FirstBytes::of_pattern(pattern, false);
        assert!(
            (0u8..=255).all(|byte| set.admits(byte)),
            "expected {pattern} to widen to any"
        );
    }
}

#[test]
fn dot_admits_everything() {
    let set = FirstBytes::of_pattern(".", false);
    assert!((0u8..=255).all(|byte| set.admits(byte)));
}

/// The soundness property, checked against every regular-expression rule in every definition that
/// ships with the crate: whenever the analysis rejects a leading byte, no string beginning with that
/// byte may actually match. A rejection that is wrong would silently mis-highlight code.
#[test]
fn never_rejects_a_byte_a_pattern_can_start_with() {
    use crate::grammar::{Matcher, Rule};

    // Suffixes chosen to reach past a first character into the shapes definition patterns look for.
    const SUFFIXES: [&str; 24] = [
        "", "a", "Z", "0", "9", "_", " ", "\t", "'", "\"", "\\", "\\n", "x41", "{1}", "abc_123",
        "0x1F", "::name", "!", "#\"", "e'", "ü", "()", "a'b'c", "0b1_",
    ];

    fn check(rules: &[Rule], language: &str, context: &str, checked: &mut usize) {
        for rule in rules {
            if let Matcher::RegExpr {
                pattern,
                insensitive,
                minimal,
            } = &rule.matcher
            {
                let set = FirstBytes::of_pattern(pattern, *insensitive);
                // Built exactly as the tokenizer builds it, so the property is checked on the real
                // regex the rule matches with.
                let mut source = String::new();
                if *insensitive {
                    source.push_str("(?i)");
                }
                if *minimal {
                    source.push_str("(?U)");
                }
                source.push_str("\\A(?:");
                source.push_str(pattern);
                source.push(')');
                if let Ok(regex) = fancy_regex::Regex::new(&source) {
                    *checked += 1;
                    for byte in 0u8..=127 {
                        if set.admits(byte) {
                            continue;
                        }
                        for suffix in SUFFIXES {
                            let mut candidate = String::new();
                            candidate.push(char::from(byte));
                            candidate.push_str(suffix);
                            assert!(
                                !matches!(regex.find(&candidate), Ok(Some(m)) if m.start() == 0),
                                "{language}/{context}: /{pattern}/ matches {candidate:?} \
                                 but byte {byte:#04x} was rejected"
                            );
                        }
                    }
                }
            }
            check(&rule.children, language, context, checked);
        }
    }

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data");
    let mut checked = 0usize;
    let mut grammars = 0usize;
    for directory in ["syntax", "syntax-copyleft"] {
        let entries = std::fs::read_dir(root.join(directory)).expect("read definition directory");
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "xml") {
                continue;
            }
            let Ok(xml) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(grammar) = crate::parse::parse_grammar(&xml) else {
                continue;
            };
            grammars += 1;
            let language = path.file_stem().unwrap_or_default().to_string_lossy();
            for context in &grammar.contexts {
                check(&context.rules, &language, &context.name, &mut checked);
            }
        }
    }
    assert!(
        grammars > 40,
        "expected the bundled definitions, saw {grammars}"
    );
    assert!(
        checked > 2000,
        "expected many regex rules, checked {checked}"
    );
}
