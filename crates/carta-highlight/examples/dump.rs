//! Tokenize stdin as a given language and print each line's tokens as `class:"text"`, for
//! inspecting how a definition classifies source. Usage: `dump <language> < file`.

use carta_highlight::Highlighter;

fn main() -> std::process::ExitCode {
    let Some(lang) = std::env::args().nth(1) else {
        eprintln!("usage: dump <language> < file");
        return std::process::ExitCode::FAILURE;
    };
    let code = match std::io::read_to_string(std::io::stdin()) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("read stdin: {err}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let Some(lines) = Highlighter::new().highlight(&lang, &code) else {
        eprintln!("unknown language: {lang}");
        return std::process::ExitCode::FAILURE;
    };
    for line in &lines {
        let parts: Vec<String> = line
            .iter()
            .map(|t| format!("{}:{:?}", t.kind.html_class(), t.text))
            .collect();
        println!("{}", parts.join(" | "));
    }
    std::process::ExitCode::SUCCESS
}
