#![no_main]

use carta_ast::Document;
use carta_fuzz::check_bytes_writer;
use carta_writers::{Epub2Writer, Epub3Writer};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|document: Document| {
    check_bytes_writer(&Epub2Writer, &document);
    check_bytes_writer(&Epub3Writer, &document);
});
