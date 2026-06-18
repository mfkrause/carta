#![no_main]

use libfuzzer_sys::fuzz_target;
use carta_core::{Reader, ReaderOptions};
use carta_readers::TsvReader;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = TsvReader.read(text, &ReaderOptions::default());
    }
});
