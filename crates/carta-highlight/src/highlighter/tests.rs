#![allow(clippy::indexing_slicing)]
use super::*;

use super::helpers::{match_decimal, match_float, match_hex, normalize};
use crate::token::{Token, TokenKind};

const RUST_SNIPPET: &str = r#"use std::collections::HashMap;

/// A small example struct.
pub struct Point {
    x: f64,
    y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Self {
        Point { x, y }
    }

    fn magnitude(&self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
}

fn main() {
    let mut counts: HashMap<String, u32> = HashMap::new();
    let p = Point::new(3.0, 4.0);
    let label = "distance";
    let n = 0xFF;
    counts.insert(label.to_string(), n);
    // Print the magnitude.
    println!("{} = {}", label, p.magnitude());
    for i in 0..10 {
        counts.insert(format!("k{}", i), i as u32);
    }
}
"#;

fn kinds(line: &SourceLine) -> Vec<(TokenKind, &str)> {
    line.iter().map(|t| (t.kind, t.text.as_str())).collect()
}

#[test]
fn highlights_c_keyword_and_number() {
    // The C grammar ships in the runtime pack, so this also covers stem-registered loading.
    let mut hl = Highlighter::new();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data/syntax-copyleft/c.xml");
    let xml = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    hl.registry_mut()
        .add_definition_with_stem(&xml, "c")
        .expect("parse c grammar");
    let lines = hl.highlight("c", "int x = 42;").expect("c is known");
    assert_eq!(lines.len(), 1);
    let toks = kinds(&lines[0]);
    assert!(
        toks.iter()
            .any(|(k, t)| *k == TokenKind::DataType && *t == "int")
    );
    assert!(
        toks.iter()
            .any(|(k, t)| *k == TokenKind::DecVal && *t == "42")
    );
}

#[test]
fn unknown_language_returns_none() {
    let hl = Highlighter::new();
    assert!(hl.highlight("no-such-lang", "x").is_none());
}

#[test]
fn normalizes_adjacent_same_kind() {
    let merged = normalize(vec![
        Token::new(TokenKind::Normal, "a"),
        Token::new(TokenKind::Normal, "b"),
        Token::new(TokenKind::Keyword, ""),
        Token::new(TokenKind::Keyword, "if"),
    ]);
    assert_eq!(
        merged,
        vec![
            Token::new(TokenKind::Normal, "ab"),
            Token::new(TokenKind::Keyword, "if"),
        ]
    );
}

#[test]
fn splits_lines_like_the_spec() {
    assert_eq!(split_lines(""), Vec::<&str>::new());
    assert_eq!(split_lines("a\n"), vec!["a"]);
    assert_eq!(split_lines("a\nb"), vec!["a", "b"]);
    assert_eq!(split_lines("a\n\n"), vec!["a", ""]);
}

#[test]
fn float_matcher_matches_expected_forms() {
    assert_eq!(match_float("5e2"), Some(3));
    assert_eq!(match_float("5.2"), Some(3));
    assert_eq!(match_float(".23"), Some(3));
    assert_eq!(match_float("5"), None);
    assert_eq!(match_float("5.2.3"), None);
}

#[test]
#[allow(clippy::too_many_lines)]
fn rust_snippet_token_stream_is_stable() {
    use TokenKind::{
        Comment, ControlFlow, DataType, DecVal, Keyword, Normal, Operator, Preprocessor, String,
    };
    let hl = Highlighter::new();
    let lines = hl.highlight("rust", RUST_SNIPPET).expect("rust is known");
    let actual: Vec<Vec<(TokenKind, &str)>> = lines.iter().map(kinds).collect();
    let expected: Vec<Vec<(TokenKind, &str)>> = vec![
        vec![
            (Keyword, "use"),
            (Normal, " "),
            (Preprocessor, "std::collections::"),
            (Normal, "HashMap"),
            (Operator, ";"),
        ],
        vec![],
        vec![(Comment, "/// A small example struct.")],
        vec![
            (Keyword, "pub"),
            (Normal, " "),
            (Keyword, "struct"),
            (Normal, " Point "),
            (Operator, "{"),
        ],
        vec![
            (Normal, "    x"),
            (Operator, ":"),
            (Normal, " "),
            (DataType, "f64"),
            (Operator, ","),
        ],
        vec![
            (Normal, "    y"),
            (Operator, ":"),
            (Normal, " "),
            (DataType, "f64"),
            (Operator, ","),
        ],
        vec![(Operator, "}")],
        vec![],
        vec![(Keyword, "impl"), (Normal, " Point "), (Operator, "{")],
        vec![
            (Normal, "    "),
            (Keyword, "pub"),
            (Normal, " "),
            (Keyword, "fn"),
            (Normal, " new(x"),
            (Operator, ":"),
            (Normal, " "),
            (DataType, "f64"),
            (Operator, ","),
            (Normal, " y"),
            (Operator, ":"),
            (Normal, " "),
            (DataType, "f64"),
            (Normal, ") "),
            (Operator, "->"),
            (Normal, " "),
            (DataType, "Self"),
            (Normal, " "),
            (Operator, "{"),
        ],
        vec![
            (Normal, "        Point "),
            (Operator, "{"),
            (Normal, " x"),
            (Operator, ","),
            (Normal, " y "),
            (Operator, "}"),
        ],
        vec![(Normal, "    "), (Operator, "}")],
        vec![],
        vec![
            (Normal, "    "),
            (Keyword, "fn"),
            (Normal, " magnitude("),
            (Operator, "&"),
            (Keyword, "self"),
            (Normal, ") "),
            (Operator, "->"),
            (Normal, " "),
            (DataType, "f64"),
            (Normal, " "),
            (Operator, "{"),
        ],
        vec![
            (Normal, "        ("),
            (Keyword, "self"),
            (Operator, "."),
            (Normal, "x "),
            (Operator, "*"),
            (Normal, " "),
            (Keyword, "self"),
            (Operator, "."),
            (Normal, "x "),
            (Operator, "+"),
            (Normal, " "),
            (Keyword, "self"),
            (Operator, "."),
            (Normal, "y "),
            (Operator, "*"),
            (Normal, " "),
            (Keyword, "self"),
            (Operator, "."),
            (Normal, "y)"),
            (Operator, "."),
            (Normal, "sqrt()"),
        ],
        vec![(Normal, "    "), (Operator, "}")],
        vec![(Operator, "}")],
        vec![],
        vec![(Keyword, "fn"), (Normal, " main() "), (Operator, "{")],
        vec![
            (Normal, "    "),
            (Keyword, "let"),
            (Normal, " "),
            (Keyword, "mut"),
            (Normal, " counts"),
            (Operator, ":"),
            (Normal, " HashMap"),
            (Operator, "<"),
            (DataType, "String"),
            (Operator, ","),
            (Normal, " "),
            (DataType, "u32"),
            (Operator, ">"),
            (Normal, " "),
            (Operator, "="),
            (Normal, " "),
            (Preprocessor, "HashMap::"),
            (Normal, "new()"),
            (Operator, ";"),
        ],
        vec![
            (Normal, "    "),
            (Keyword, "let"),
            (Normal, " p "),
            (Operator, "="),
            (Normal, " "),
            (Preprocessor, "Point::"),
            (Normal, "new("),
            (DecVal, "3.0"),
            (Operator, ","),
            (Normal, " "),
            (DecVal, "4.0"),
            (Normal, ")"),
            (Operator, ";"),
        ],
        vec![
            (Normal, "    "),
            (Keyword, "let"),
            (Normal, " label "),
            (Operator, "="),
            (Normal, " "),
            (String, "\"distance\""),
            (Operator, ";"),
        ],
        vec![
            (Normal, "    "),
            (Keyword, "let"),
            (Normal, " n "),
            (Operator, "="),
            (Normal, " "),
            (DecVal, "0xFF"),
            (Operator, ";"),
        ],
        vec![
            (Normal, "    counts"),
            (Operator, "."),
            (Normal, "insert(label"),
            (Operator, "."),
            (Normal, "to_string()"),
            (Operator, ","),
            (Normal, " n)"),
            (Operator, ";"),
        ],
        vec![(Normal, "    "), (Comment, "// Print the magnitude.")],
        vec![
            (Normal, "    "),
            (Preprocessor, "println!"),
            (Normal, "("),
            (String, "\"{} = {}\""),
            (Operator, ","),
            (Normal, " label"),
            (Operator, ","),
            (Normal, " p"),
            (Operator, "."),
            (Normal, "magnitude())"),
            (Operator, ";"),
        ],
        vec![
            (Normal, "    "),
            (ControlFlow, "for"),
            (Normal, " i "),
            (Keyword, "in"),
            (Normal, " "),
            (DecVal, "0"),
            (Operator, ".."),
            (DecVal, "10"),
            (Normal, " "),
            (Operator, "{"),
        ],
        vec![
            (Normal, "        counts"),
            (Operator, "."),
            (Normal, "insert("),
            (Preprocessor, "format!"),
            (Normal, "("),
            (String, "\"k{}\""),
            (Operator, ","),
            (Normal, " i)"),
            (Operator, ","),
            (Normal, " i "),
            (Keyword, "as"),
            (Normal, " "),
            (DataType, "u32"),
            (Normal, ")"),
            (Operator, ";"),
        ],
        vec![(Normal, "    "), (Operator, "}")],
        vec![(Operator, "}")],
    ];
    assert_eq!(actual, expected);
}

#[test]
fn hex_and_decimal() {
    assert_eq!(match_hex("0xFF"), Some(4));
    assert_eq!(match_hex("0x"), None);
    assert_eq!(match_decimal("42abc"), Some(2));
    assert_eq!(match_decimal("abc"), None);
}
