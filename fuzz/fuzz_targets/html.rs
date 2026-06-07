#![no_main]

use libfuzzer_sys::fuzz_target;
use oxidoc_core::{Reader, ReaderOptions};
use oxidoc_readers::HtmlReader;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = HtmlReader.read(text, &ReaderOptions::default());
    }
});
