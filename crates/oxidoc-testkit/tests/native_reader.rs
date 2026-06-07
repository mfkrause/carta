//! Reader-parity tests for the native reader: each case is a small native-format source exercising
//! one or more AST node kinds the reader handles. The reader's output (as JSON) is diffed against
//! `pandoc -f native -t json`, minted at run time; expected values are never committed. The oracle
//! is hard-required (its absence fails with provisioning instructions rather than skipping).

// This whole file is test code, where panicking on a known case is the idiomatic assertion.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use oxidoc_testkit::differential::{self, Diff};
use oxidoc_testkit::pandoc_bin;

/// A reader-parity case: a human label and the native-format source text.
struct Case {
    label: &'static str,
    input: &'static str,
}

const fn case(label: &'static str, input: &'static str) -> Case {
    Case { label, input }
}

/// Cases chosen to cover every node path the native reader parses: the four top-level document
/// forms, every block and inline constructor, the metadata value kinds, and the lexer's string
/// escapes and numeric literals.
#[allow(clippy::too_many_lines)]
fn cases() -> Vec<Case> {
    vec![
        // Top-level document forms.
        case("empty-block-list", "[]"),
        case("bare-block", r#"Para [ Str "hi" ]"#),
        case("bare-inline", r#"Str "lone""#),
        case("inline-list", r#"[ Str "a" , Space , Str "b" ]"#),
        case(
            "pandoc-wrapper-empty-meta",
            r#"Pandoc (Meta {unMeta = fromList []}) [ Para [ Str "hi" ] ]"#,
        ),
        // Metadata value kinds.
        case(
            "meta-all-kinds",
            r#"Pandoc (Meta {unMeta = fromList [("title", MetaInlines [Str "T"]), ("flag", MetaBool True), ("off", MetaBool False), ("s", MetaString "x"), ("lst", MetaList [MetaString "a", MetaString "b"]), ("m", MetaMap (fromList [("k", MetaString "v")])), ("blk", MetaBlocks [Para [Str "p"]])]}) [ Para [ Str "body" ] ]"#,
        ),
        // Inline constructors.
        case(
            "emphasis-family",
            r#"[ Para [ Emph [ Str "e" ] , Strong [ Str "s" ] , Underline [ Str "u" ] , Strikeout [ Str "k" ] , Superscript [ Str "2" ] , Subscript [ Str "3" ] , SmallCaps [ Str "c" ] ] ]"#,
        ),
        case(
            "quoted",
            r#"[ Para [ Quoted SingleQuote [ Str "a" ] , Quoted DoubleQuote [ Str "b" ] ] ]"#,
        ),
        case(
            "breaks-and-space",
            r#"[ Para [ Str "a" , Space , SoftBreak , LineBreak , Str "b" ] ]"#,
        ),
        case(
            "code-inline",
            r#"[ Para [ Code ( "i" , [ "lang" ] , [ ( "k" , "v" ) ] ) "x = 1" ] ]"#,
        ),
        case(
            "math",
            r#"[ Para [ Math InlineMath "a^2" , Math DisplayMath "b" ] ]"#,
        ),
        case(
            "raw-inline",
            r#"[ Para [ RawInline (Format "html") "<b>" ] ]"#,
        ),
        case(
            "link-image",
            r#"[ Para [ Link ( "l" , [ "c" ] , [] ) [ Str "t" ] ( "http://x" , "ti" ) , Image ( "" , [] , [] ) [ Str "alt" ] ( "p.png" , "" ) ] ]"#,
        ),
        case(
            "note",
            r#"[ Para [ Str "x" , Note [ Para [ Str "n" ] ] ] ]"#,
        ),
        case(
            "span",
            r#"[ Para [ Span ( "s" , [ "c" ] , [] ) [ Str "inner" ] ] ]"#,
        ),
        case(
            "cite",
            r#"[ Para [ Cite [ Citation { citationId = "k" , citationPrefix = [ Str "see" ] , citationSuffix = [ Str "p5" ] , citationMode = NormalCitation , citationNoteNum = 1 , citationHash = 0 } ] [ Str "[@k]" ] ] ]"#,
        ),
        case(
            "cite-modes",
            r#"[ Para [ Cite [ Citation { citationId = "a" , citationPrefix = [] , citationSuffix = [] , citationMode = AuthorInText , citationNoteNum = 0 , citationHash = 0 } , Citation { citationId = "b" , citationPrefix = [] , citationSuffix = [] , citationMode = SuppressAuthor , citationNoteNum = 0 , citationHash = 0 } ] [ Str "x" ] ] ]"#,
        ),
        // Block constructors.
        case("plain", r#"[ Plain [ Str "p" ] ]"#),
        case(
            "line-block",
            r#"[ LineBlock [ [ Str "one" ] , [ Str "two" ] ] ]"#,
        ),
        case(
            "code-block",
            r#"[ CodeBlock ( "" , [ "rust" ] , [] ) "let x = 1;" ]"#,
        ),
        case("raw-block", r#"[ RawBlock (Format "html") "<div>" ]"#),
        case("block-quote", r#"[ BlockQuote [ Para [ Str "q" ] ] ]"#),
        case(
            "ordered-list",
            r#"[ OrderedList ( 5 , Decimal , Period ) [ [ Plain [ Str "a" ] ] , [ Plain [ Str "b" ] ] ] ]"#,
        ),
        case(
            "ordered-list-styles",
            r#"[ OrderedList ( 1 , LowerRoman , OneParen ) [ [ Plain [ Str "a" ] ] ] , OrderedList ( 1 , UpperAlpha , TwoParens ) [ [ Plain [ Str "b" ] ] ] , OrderedList ( 1 , Example , DefaultDelim ) [ [ Plain [ Str "c" ] ] ] , OrderedList ( 1 , DefaultStyle , Period ) [ [ Plain [ Str "d" ] ] ] , OrderedList ( 1 , LowerAlpha , Period ) [ [ Plain [ Str "e" ] ] ] , OrderedList ( 1 , UpperRoman , Period ) [ [ Plain [ Str "f" ] ] ] ]"#,
        ),
        case(
            "bullet-list",
            r#"[ BulletList [ [ Plain [ Str "a" ] ] , [ Plain [ Str "b" ] ] ] ]"#,
        ),
        case(
            "definition-list",
            r#"[ DefinitionList [ ( [ Str "Term" ] , [ [ Plain [ Str "def" ] ] ] ) ] ]"#,
        ),
        case(
            "header",
            r#"[ Header 2 ( "id" , [ "c" ] , [ ( "k" , "v" ) ] ) [ Str "H" ] ]"#,
        ),
        case("horizontal-rule", "[ HorizontalRule ]"),
        case(
            "div",
            r#"[ Div ( "d" , [ "note" ] , [] ) [ Para [ Str "body" ] ] ]"#,
        ),
        case(
            "figure",
            r#"[ Figure ( "f" , [] , [] ) (Caption Nothing [ Plain [ Str "cap" ] ]) [ Plain [ Image ( "" , [] , [] ) [ Str "alt" ] ( "p.png" , "" ) ] ] ]"#,
        ),
        case(
            "figure-short-caption",
            r#"[ Figure ( "" , [] , [] ) (Caption (Just [ Str "short" ]) [ Plain [ Str "long" ] ]) [ Para [ Str "x" ] ] ]"#,
        ),
        // Tables: alignments, column widths, spans, head/body/foot.
        case(
            "table",
            r#"[ Table ( "" , [] , [] ) (Caption Nothing [ Plain [ Str "cap" ] ]) [ ( AlignLeft , ColWidth 0.5 ) , ( AlignRight , ColWidthDefault ) , ( AlignCenter , ColWidthDefault ) , ( AlignDefault , ColWidthDefault ) ] (TableHead ( "" , [] , [] ) [ Row ( "" , [] , [] ) [ Cell ( "" , [] , [] ) AlignDefault (RowSpan 1) (ColSpan 1) [ Plain [ Str "h" ] ] ] ]) [ TableBody ( "" , [] , [] ) (RowHeadColumns 0) [] [ Row ( "" , [] , [] ) [ Cell ( "" , [] , [] ) AlignDefault (RowSpan 2) (ColSpan 1) [ Plain [ Str "a" ] ] ] ] ] (TableFoot ( "" , [] , [] ) []) ]"#,
        ),
        // Lexer: string escapes (named control, decimal, hex, octal, gap, separator) and non-ASCII.
        case("escape-nonascii", r#"[ Para [ Str "caf\233" ] ]"#),
        case("escape-separator", r#"[ Para [ Str "\233\&1" ] ]"#),
        case("escape-named-control", r#"[ Para [ Str "a\SOHb\ESCc" ] ]"#),
        case("escape-radix", r#"[ Para [ Str "\x41\o101" ] ]"#),
        case("escape-c-style", r#"[ Para [ Str "tab\there\nnl" ] ]"#),
        case("escape-gap", "[ Para [ Str \"a\\   \\b\" ] ]"),
    ]
}

#[test]
fn reader_matches_oracle_native() {
    assert!(
        pandoc_bin().is_file(),
        "pinned pandoc binary not found at {}.\nRun tools/install-pandoc.sh.",
        pandoc_bin().display()
    );

    let mut failures = Vec::new();
    for case in cases() {
        match differential::reader_json("native", case.input).expect("run reader surface") {
            Diff::Match | Diff::OracleRejected { .. } => {}
            Diff::Mismatch { detail } => failures.push(format!("{}: {detail}", case.label)),
            Diff::OxidocError { detail } => {
                failures.push(format!("{}: error: {detail}", case.label));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{}/{} reader cases diverged:\n{}",
        failures.len(),
        cases().len(),
        failures.join("\n")
    );
}
