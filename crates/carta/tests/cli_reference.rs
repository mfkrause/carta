//! Keep the website's CLI reference page in step with the binary's man page.
//!
//! Set `UPDATE_CLI_REFERENCE=1` to rewrite the page.

#![cfg(all(feature = "cli", feature = "full"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::process::Command;

use carta::{ReaderOptions, WrapMode, WriterOptions, convert_text};

const PAGE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../website/src/content/docs/cli/reference.md"
);

const FRONT_MATTER: &str = "---\ntitle: CLI reference\ndescription: Every carta command-line flag, generated from the binary's own help text.\nsidebar:\n  order: 2\n---\n\n<!-- Generated from `carta --man` by crates/carta/tests/cli_reference.rs. Do not edit. -->\n\n:::note[Converted by carta]\nThis page is `carta --man` read back in and converted to Markdown by carta itself. Pretty neat!\n:::\n\n";

#[test]
fn reference_page_matches_the_man_page() {
    let page = reference_page();
    if std::env::var_os("UPDATE_CLI_REFERENCE").is_some() {
        fs::write(PAGE, &page).expect("write the CLI reference page");
    }
    let committed = fs::read_to_string(PAGE).expect("read the CLI reference page");
    assert!(
        committed == page,
        "website/src/content/docs/cli/reference.md is out of date; rewrite it with\n  \
         UPDATE_CLI_REFERENCE=1 cargo nextest run -p carta --test cli_reference"
    );
}

fn reference_page() -> String {
    let mut writer_options = WriterOptions::default();
    writer_options.wrap = WrapMode::None;
    let markdown = convert_text(
        "man",
        "gfm",
        &man_page(),
        &ReaderOptions::default(),
        &writer_options,
    )
    .expect("convert the man page");

    let body = drop_version_section(&demote_headings(&code_spans(&markdown)));
    format!("{FRONT_MATTER}{}\n", body.trim_end())
}

fn man_page() -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_carta"))
        .arg("--man")
        .output()
        .expect("run carta --man");
    assert!(output.status.success(), "carta --man failed");
    String::from_utf8(output.stdout).expect("utf-8 man page")
}

/// Drops the version section and everything under it.
///
/// The page is committed, so a version in it would go stale on every release commit.
fn drop_version_section(markdown: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut dropping = false;

    for line in markdown.lines() {
        if line.starts_with("## ") {
            dropping = line == "## Version";
        }
        if !dropping {
            kept.push(line);
        }
    }

    kept.join("\n")
}

/// Restores the backticks the help text writes around code as Markdown code spans.
///
/// roff has no code spans, so the Markdown writer escapes each backtick. Each balanced pair becomes
/// a span with its inner escapes undone; an unpaired backtick stays as written.
fn code_spans(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut rest = markdown;

    while let Some(open) = rest.find("\\`") {
        let (before, from_open) = rest.split_at(open);
        out.push_str(before);
        let Some(inner) = from_open.strip_prefix("\\`") else {
            break;
        };
        if let Some(close) = inner.find("\\`") {
            let (span, tail) = inner.split_at(close);
            out.push('`');
            out.push_str(&unescape(span));
            out.push('`');
            rest = tail.strip_prefix("\\`").unwrap_or(tail);
        } else {
            out.push_str("\\`");
            rest = inner;
        }
    }

    out.push_str(rest);
    out
}

/// Drops the backslash from every escaped ASCII punctuation character.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut characters = text.chars();

    while let Some(character) = characters.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        match characters.next() {
            Some(escaped) if escaped.is_ascii_punctuation() => out.push(escaped),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }

    out
}

/// Shifts every heading down one level, below the page title, and sets all-capital section names
/// in sentence case.
fn demote_headings(markdown: &str) -> String {
    markdown
        .lines()
        .map(|line| match line.split_once(' ') {
            Some((hashes, title))
                if hashes.starts_with('#') && hashes.bytes().all(|byte| byte == b'#') =>
            {
                format!("#{hashes} {}", sentence_case(title))
            }
            _ => line.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Lowercases a heading that is set entirely in capitals, keeping its first letter.
fn sentence_case(title: &str) -> String {
    if title.chars().any(char::is_lowercase) {
        return title.to_owned();
    }
    let mut characters = title.chars();
    match characters.next() {
        Some(first) => {
            first.to_uppercase().collect::<String>() + &characters.as_str().to_lowercase()
        }
        None => String::new(),
    }
}
