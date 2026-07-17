#![no_main]

use libfuzzer_sys::fuzz_target;
use carta_core::{BytesReader, ReaderOptions};
use carta_readers::OdtReader;

fuzz_target!(|data: &[u8]| {
    let _ = OdtReader.read(data, &ReaderOptions::default());
});
