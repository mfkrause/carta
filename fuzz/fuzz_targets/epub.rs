#![no_main]

use carta_core::{BytesReader, ReaderOptions};
use carta_readers::EpubReader;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = EpubReader.read(data, &ReaderOptions::default());
});
