//! Report oxidoc's CommonMark-reader parity against the pinned binary, example by example.
//!
//! Runs every worked example from the vendored spec through the reader surface and prints a pass
//! count plus the first failures. A `--surface=e2e` flag switches to the full text→HTML pipeline.
//! Intended for fast iteration; the gated tests live in `tests/`.

use std::io::Write;

use oxidoc::ReaderOptions;
use oxidoc_testkit::commonmark_spec::examples;
use oxidoc_testkit::differential::{self, Diff};

fn main() {
    // Parse-only mode: feed each example to the reader, announcing the number first so a hang is
    // pinpointed by the last line printed. No oracle involved.
    if std::env::args().any(|a| a == "--parse-only") {
        let reader = match oxidoc::reader_for("commonmark") {
            Ok(reader) => reader,
            Err(error) => {
                eprintln!("commonmark reader unavailable: {error}");
                std::process::exit(2);
            }
        };
        for example in &examples() {
            print!("parsing {} ... ", example.number);
            let _ = std::io::stdout().flush();
            match reader.read(&example.markdown, &ReaderOptions::default()) {
                Ok(_) => println!("ok"),
                Err(error) => println!("error: {error}"),
            }
        }
        return;
    }

    if !differential::oracle_available() {
        eprintln!("oracle binary absent (.oracle/bin/pandoc); run tools/install-pandoc.sh");
        std::process::exit(2);
    }

    let args: Vec<String> = std::env::args().collect();
    let e2e = args.iter().any(|a| a == "--surface=e2e");
    let show = args
        .iter()
        .find_map(|a| a.strip_prefix("--show="))
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(30);

    let examples = examples();
    let total = examples.len();
    let mut passed: usize = 0;
    let mut failures: Vec<(usize, String, String)> = Vec::new();

    for example in &examples {
        let diff = if e2e {
            differential::e2e("commonmark", "html", &example.markdown)
        } else {
            differential::reader_json("commonmark", &example.markdown)
        };
        match diff {
            Ok(Diff::Match | Diff::OracleRejected { .. }) => passed += 1,
            Ok(other) => {
                failures.push((example.number, describe(&other), example.markdown.clone()));
            }
            Err(error) => failures.push((
                example.number,
                format!("io: {error}"),
                example.markdown.clone(),
            )),
        }
    }

    let surface = if e2e {
        "e2e text→html"
    } else {
        "reader →json"
    };
    // Counts are in the low thousands; the f64 conversion is exact at this scale.
    #[allow(clippy::cast_precision_loss)]
    let percent = 100.0 * passed as f64 / total as f64;
    println!("{surface}: {passed}/{total} examples match ({percent:.1}%)");
    println!("first {} failures:", show.min(failures.len()));
    for (number, detail, markdown) in failures.iter().take(show) {
        println!("  ex {number}: {detail}");
        println!("      input: {:?}", truncate(markdown, 80));
    }
}

fn describe(diff: &Diff) -> String {
    match diff {
        Diff::Mismatch { detail } => format!("mismatch {detail}"),
        Diff::OxidocError { detail } => format!("error {detail}"),
        Diff::Match | Diff::OracleRejected { .. } => "ok".to_owned(),
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let head: String = text.chars().take(max).collect();
        format!("{head}…")
    }
}
