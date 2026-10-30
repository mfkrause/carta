use super::*;
use std::path::PathBuf;

fn read_sources(first: &str, second: &str, macros: bool) -> Document {
    let input = format!("{first}\n\n{second}");
    let mut options = ReaderOptions::default();
    options.extensions = carta_core::extensions::presets::MARKDOWN;
    options.extensions.insert(Extension::RebaseRelativePaths);
    if macros {
        options.extensions.insert(Extension::LatexMacros);
    }
    options.greedy_paragraphs = true;
    options.source_dir = Some(PathBuf::from("a"));
    options.source_dirs.insert(0, PathBuf::from("a"));
    options
        .source_dirs
        .insert(first.len() + 2, PathBuf::from("b"));
    CommonmarkReader.read(&input, &options).unwrap()
}

fn targets(document: &mut Document) -> Vec<String> {
    let mut targets = Vec::new();
    carta_core::walk::for_each_document_inline(document, &mut |inline| {
        if let Inline::Link(_, _, target) | Inline::Image(_, _, target) = inline {
            targets.push(target.url.to_string());
        }
    });
    targets
}

#[test]
fn direct_links_and_images_use_their_own_input_directory() {
    let mut document = read_sources(
        "[one](page.md) ![image](images/a%20b.png)",
        "[two](../page.md?q=1#part) [query](?q=1) [fragment](#part) [root](/page.md) [network](//host/a) [scheme](custom:item) [empty]()",
        false,
    );
    assert_eq!(
        targets(&mut document),
        [
            "a/page.md",
            "a/images/a%20b.png",
            "b/../page.md?q=1#part",
            "b/?q=1",
            "#part",
            "/page.md",
            "//host/a",
            "custom:item",
            ""
        ]
    );
}

#[test]
fn references_use_the_definition_directory_in_both_directions() {
    let mut document = read_sources(
        "[later]\n\n[shared]: refs/item.md",
        "[later]: later.md\n\n[shared] [later]",
        false,
    );
    assert_eq!(
        targets(&mut document),
        ["b/later.md", "a/refs/item.md", "b/later.md"]
    );
}

#[test]
fn normalization_and_metadata_keep_the_original_source_directories() {
    let mut document = read_sources(
        "\u{feff}---\r\ntitle: '[meta](meta.md)'\r\n---\r\n\r\n> \t[one](one.md)\r\n\r\n[shared]: shared.md",
        "[two](two.md) [shared]",
        false,
    );
    let mut actual = targets(&mut document);
    actual.sort();
    assert_eq!(actual, ["a/meta.md", "a/one.md", "a/shared.md", "b/two.md"]);
}

#[test]
fn note_targets_keep_the_definition_location_with_and_without_expansion() {
    for macros in [false, true] {
        let first = "\\newcommand{\\word}{Hello}\n\nText[^n]. [later]\n\n$\\word$";
        let second = "[^n]: [note](note.md)\n\n[later]: later.md";
        let mut document = read_sources(first, second, macros);
        assert_eq!(targets(&mut document), ["b/note.md", "b/later.md"]);
    }
}

#[test]
fn a_caption_keeps_its_directory_when_attached_to_another_input_table() {
    let mut document = read_sources(
        "| [cell](cell.md) |\n|---|\n| value |",
        ": [caption](caption.md)",
        false,
    );
    assert_eq!(targets(&mut document), ["b/caption.md", "a/cell.md"]);
}

#[test]
fn definition_list_terms_and_definitions_keep_their_own_directories() {
    let mut document = read_sources(
        "[first](first.md)\n: [one](one.md)",
        "[second](second.md)\n: [two](two.md)",
        false,
    );
    assert_eq!(
        targets(&mut document),
        ["a/first.md", "a/one.md", "b/second.md", "b/two.md"]
    );
    let mut document = read_sources("[term](term.md)", ": [definition](definition.md)", false);
    assert_eq!(targets(&mut document), ["a/term.md", "b/definition.md"]);
}

#[test]
fn multiline_table_rows_keep_their_input_directories() {
    let mut document = read_sources(
        "--------------------\n Head\n--------------------\n ![one](one.png)\n",
        " ![two](two.png)\n--------------------\n",
        false,
    );
    assert_eq!(targets(&mut document), ["a/one.png", "b/two.png"]);
}

#[test]
fn source_directory_punctuation_does_not_change_uri_components() {
    let mut options = ReaderOptions::default();
    options.extensions = Extensions::from_list(&[Extension::RebaseRelativePaths]);
    options.source_dir = Some(PathBuf::from("folder #?%: name"));
    let mut document = CommonmarkReader
        .read("[file](file.md?q=1#part)", &options)
        .unwrap();
    assert_eq!(
        targets(&mut document),
        ["folder%20%23%3F%25%3A%20name/file.md?q=1#part"]
    );
}

#[test]
fn deep_containers_keep_directory_scopes_and_drop_safely() {
    let mut options = ReaderOptions::default();
    options.extensions = Extensions::from_list(&[Extension::RebaseRelativePaths]);
    options.source_dir = Some(PathBuf::from("a"));
    let input = format!("{}[file](file.md)", "> ".repeat(2000));
    let mut document = CommonmarkReader.read(&input, &options).unwrap();
    assert_eq!(targets(&mut document), ["a/file.md"]);
}

#[test]
fn continued_containers_keep_each_inputs_own_directory() {
    for marker in ["> ", "- ", "1. ", "> - "] {
        let first = format!("{marker}[one](one.md)");
        let second = format!("{marker}[two](two.md)");
        let mut document = read_sources(&first, &second, false);
        assert_eq!(targets(&mut document), ["a/one.md", "b/two.md"], "{marker}");
    }
}

#[test]
fn wiki_page_names_stay_in_their_own_namespace() {
    let mut options = ReaderOptions::default();
    options.extensions = Extensions::from_list(&[
        Extension::RebaseRelativePaths,
        Extension::WikilinksTitleAfterPipe,
    ]);
    options.source_dir = Some(PathBuf::from("a"));
    let mut document = CommonmarkReader.read("[[Page|label]]", &options).unwrap();
    assert_eq!(targets(&mut document), ["Page"]);
}
