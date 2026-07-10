#![no_main]

use carta_core::{Reader, ReaderOptions};
use carta_readers::RtfReader;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = RtfReader.read(text, &ReaderOptions::default());
    }
});
