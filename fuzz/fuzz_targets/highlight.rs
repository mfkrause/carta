#![no_main]

use carta_highlight::Highlighter;
use libfuzzer_sys::fuzz_target;

// These grammars cover indentation, contexts, continuations, and regex rules.
static GRAMMARS: [&str; 6] = ["python", "haskell", "cpp", "bash", "xml", "latex"];

// The single-threaded refcounted grammar cache is not `Sync`, so one instance per fuzzing thread.
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
