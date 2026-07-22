#![no_main]

use carta_ast::Document;
use carta_fuzz::check_text_writer;
use carta_writers::{CommonmarkXWriter, GfmWriter, MarkdownGithubWriter, MarkdownMmdWriter, MarkdownPhpextraWriter, MarkdownStrictWriter, MarkdownWriter};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|document: Document| {
    check_text_writer(&MarkdownWriter, &document);
    check_text_writer(&MarkdownStrictWriter, &document);
    check_text_writer(&MarkdownPhpextraWriter, &document);
    check_text_writer(&MarkdownMmdWriter, &document);
    check_text_writer(&MarkdownGithubWriter, &document);
    check_text_writer(&CommonmarkXWriter, &document);
    check_text_writer(&GfmWriter, &document);
});
