use std::path::PathBuf;

use super::super::AsciidocReader;
use carta_ast::{Attr, Block, Inline, Target};
use carta_core::{Reader, ReaderOptions};

#[test]
fn included_terms_use_the_same_late_reference_boundary() {
    let directory = super::Directory::new();
    std::fs::write(
        directory.0.join("terms.adoc"),
        "{value}:: Included {value}\nxref:target[]:: Link\nicon:heart[]:: Icon\n",
    )
    .unwrap();
    let mut options = ReaderOptions::default();
    options.source_dir = Some(directory.0.clone());
    let mut document = AsciidocReader
        .read(
            ":value: Cedar\n:icons: font\n\ninclude::terms.adoc[]\n\n[[target]]\n== Target\n",
            &options,
        )
        .unwrap();

    let mut terms = Vec::new();
    carta_core::walk::for_each_document_block(&mut document, &mut |block| {
        if let Block::DefinitionList(entries) = block {
            terms.extend(entries.iter().map(|(term, _)| term.clone()));
        }
    });
    assert_eq!(
        terms,
        [
            vec![Inline::Str("{value}".into())],
            vec![Inline::Link(
                Box::new(Attr {
                    classes: vec!["cross-reference".into()],
                    ..Default::default()
                }),
                vec![Inline::Str("target".into())],
                Box::new(Target {
                    url: "#target".into(),
                    title: "".into(),
                }),
            )],
            vec![Inline::Span(
                Box::new(Attr {
                    classes: vec!["icon".into()],
                    attributes: vec![("name".into(), "heart".into())],
                    ..Default::default()
                }),
                vec![Inline::Str("[heart]".into())],
            )],
        ]
    );
}

#[test]
fn untyped_stem_math_in_a_definition_term_returns_the_converter_error() {
    let error = AsciidocReader
        .read("stem:[x+y]:: definition\n", &ReaderOptions::default())
        .unwrap_err();
    assert!(error.to_string().contains("Encountered math type Nothing"));
}

#[test]
fn source_boundaries_do_not_resolve_term_references() {
    let first = ":value: Cedar\n\n";
    let second = "{value}:: second source\n";
    let mut options = ReaderOptions::default();
    options.source_dirs.insert(0, PathBuf::new());
    options.source_dirs.insert(first.len(), PathBuf::new());
    let document = AsciidocReader
        .read(&format!("{first}{second}"), &options)
        .unwrap();
    assert!(matches!(
        document.blocks.as_slice(),
        [Block::DefinitionList(entries)] if entries[0].0 == vec![Inline::Str("{value}".into())]
    ));
}
