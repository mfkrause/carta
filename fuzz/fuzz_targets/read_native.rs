#![no_main]

use libfuzzer_sys::fuzz_target;
use carta_core::{Reader, ReaderOptions};
use carta_readers::NativeReader;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = NativeReader.read(text, &ReaderOptions::default());
    }
});
