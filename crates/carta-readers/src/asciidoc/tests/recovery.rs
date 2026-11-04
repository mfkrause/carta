use super::super::AsciidocReader;
use carta_ast::{Block, Inline};
use carta_core::{Reader, ReaderOptions};

#[test]
fn physical_blank_lines_only_match_spaces_and_tabs() {
    let options = ReaderOptions::default();
    let reader = AsciidocReader;

    assert!(reader.read(" \t\n\t", &options).unwrap().blocks.is_empty());
    assert_eq!(
        reader.read("\u{c}", &options).unwrap().blocks,
        vec![Block::Para(vec![])]
    );

    for character in ['\u{b}', '\u{c}'] {
        assert_eq!(
            reader
                .read(&format!("before\n{character}\nafter\n"), &options)
                .unwrap()
                .blocks,
            vec![Block::Para(vec![
                Inline::Str("before".into()),
                Inline::SoftBreak,
                Inline::Str(character.to_string().into()),
                Inline::SoftBreak,
                Inline::Str("after".into()),
            ])]
        );
    }

    let document = reader.read("Term::Text\n\n\u{c}\n", &options).unwrap();
    assert!(matches!(
        document.blocks.as_slice(),
        [Block::DefinitionList(_), Block::Para(inlines)] if inlines.is_empty()
    ));

    let document = reader
        .read("[cols=1d]\n|===\n|before\n\u{c}\nafter\n|===\n", &options)
        .unwrap();
    let Some(Block::Table(table)) = document.blocks.first() else {
        panic!("expected a table");
    };
    assert_eq!(
        table.head.rows[0].cells[0].content,
        vec![Block::Para(vec![
            Inline::Str("before".into()),
            Inline::SoftBreak,
            Inline::Str("\u{c}".into()),
            Inline::SoftBreak,
            Inline::Str("after".into()),
        ])]
    );
}

#[test]
fn semantic_space_matches_asciidoc_space_characters() {
    let options = ReaderOptions::default();
    let reader = AsciidocReader;

    for character in ['\u{85}', '\u{2028}', '\u{2029}'] {
        assert_eq!(
            reader
                .read(&format!("before\n{character}\nafter\n"), &options)
                .unwrap()
                .blocks,
            vec![Block::Para(vec![
                Inline::Str("before".into()),
                Inline::SoftBreak,
                Inline::Str(character.to_string().into()),
                Inline::SoftBreak,
                Inline::Str("after".into()),
            ])]
        );
    }

    for character in ['\u{b}', '\u{c}', '\u{a0}', '\u{2003}'] {
        let source = format!("Term::{character}\n");
        assert_eq!(
            reader.read(&source, &options).unwrap().blocks,
            vec![Block::DefinitionList(vec![(
                vec![Inline::Str("Term".into())],
                vec![vec![Block::Para(vec![])]],
            )])]
        );
    }
}

#[test]
fn failed_example_falls_back_before_resolving_includes() {
    let directory = super::Directory::new();
    std::fs::write(directory.0.join("existing.adoc"), "included\n").unwrap();
    let mut options = ReaderOptions::default();
    options.source_dir = Some(directory.0.clone());

    for target in ["missing.adoc", "existing.adoc"] {
        let source = format!("====\ninclude::{target}[]\n[NOTE]\n====\n");
        assert_eq!(
            AsciidocReader.read(&source, &options).unwrap().blocks,
            vec![
                Block::Para(vec![
                    Inline::Str("====".into()),
                    Inline::SoftBreak,
                    Inline::Str(format!("include::{target}[]").into()),
                ]),
                Block::Para(vec![Inline::Str("====".into())]),
            ]
        );
    }
}

#[test]
fn selected_example_preserves_include_errors() {
    let directory = super::Directory::new();
    let mut options = ReaderOptions::default();
    options.source_dir = Some(directory.0.clone());

    for source in [
        "====\ninclude::missing.adoc[]\nText\n====\n",
        "====\n----\ninclude::missing.adoc[]\n----\n====\n",
    ] {
        assert!(
            AsciidocReader.read(source, &options).is_err(),
            "expected include failure for {source:?}"
        );
    }
}
