#![no_main]

use carta_highlight::Highlighter;
use libfuzzer_sys::fuzz_target;

// The tokenizer must terminate and never panic on untrusted code; these grammars exercise distinct
// interpreter paths (indentation, nested contexts, line continuations, embedded regexes).
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
