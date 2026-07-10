#![no_main]

use carta_highlight::Highlighter;
use libfuzzer_sys::fuzz_target;

// The tokenizer is a regex-driven state machine walking untrusted code; it must terminate and never
// panic, whatever the input. A handful of grammars with distinct context and rule shapes exercise
// separate interpreter paths (indentation, nested contexts, line continuations, embedded regexes).
static GRAMMARS: [&str; 6] = ["python", "haskell", "cpp", "bash", "xml", "latex"];

// The highlighter caches grammars with single-threaded reference counting, so it is not `Sync` and
// cannot be a plain `static`; one instance per fuzzing thread keeps setup out of the hot loop.
thread_local! {
    static HIGHLIGHTER: Highlighter = Highlighter::new();
}

fuzz_target!(|data: &[u8]| {
    if let Ok(code) = std::str::from_utf8(data) {
        HIGHLIGHTER.with(|highlighter| {
            for language in GRAMMARS {
                let _ = highlighter.highlight(language, code);
            }
        });
    }
});
