#![no_main]

use carta_ast::Document;
use carta_fuzz::check_text_writer;
use carta_writers::{ManWriter};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|document: Document| {
    check_text_writer(&ManWriter, &document);
});
