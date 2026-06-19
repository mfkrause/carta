//! `carta` — command-line interface.
//!
//! Parses `--from`/`--to` and pipes the input through the `carta` library's [`convert`], or — when
//! a `--list-*` flag is given — reports what this build supports. Format selection, aliases, the
//! recognized-but-unsupported error, and the introspection data all live in the library; this binary
//! only handles argument parsing and stdin/file I/O.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use carta::{ReaderOptions, Result, WriterOptions, convert};
use clap::Parser;

const LIST_FLAGS: [&str; 3] = [
    "list_input_formats",
    "list_output_formats",
    "list_extensions",
];

#[derive(Parser, Debug)]
#[command(name = "carta", version, about = "Document converter")]
struct Cli {
    /// Input format (e.g. `commonmark`, `json`).
    #[arg(short = 'f', long = "from", required_unless_present_any = LIST_FLAGS)]
    from: Option<String>,
    /// Output format (e.g. `html`, `json`).
    #[arg(short = 't', long = "to", required_unless_present_any = LIST_FLAGS)]
    to: Option<String>,
    /// Write output to this file instead of stdout.
    #[arg(short = 'o', long = "output")]
    output: Option<PathBuf>,
    /// List the input formats this build supports and exit.
    #[arg(long = "list-input-formats")]
    list_input_formats: bool,
    /// List the output formats this build supports and exit.
    #[arg(long = "list-output-formats")]
    list_output_formats: bool,
    /// List extensions and their default state for FORMAT (the Markdown dialect if omitted) and exit.
    // The outer `Option` distinguishes a missing flag from a present one; the inner distinguishes a
    // bare `--list-extensions` from `--list-extensions=FORMAT`. This is clap's optional-value shape.
    #[allow(clippy::option_option)]
    #[arg(long = "list-extensions", value_name = "FORMAT", num_args = 0..=1, require_equals = true)]
    list_extensions: Option<Option<String>>,
    /// Read input from this file instead of stdin.
    input: Option<PathBuf>,
}

fn main() -> ExitCode {
    match run(&Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("carta: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<()> {
    if cli.list_input_formats {
        return print_lines(&carta::input_format_names());
    }
    if cli.list_output_formats {
        return print_lines(&carta::output_format_names());
    }
    if let Some(format) = &cli.list_extensions {
        return list_extensions(format.as_deref());
    }

    match (cli.from.as_deref(), cli.to.as_deref()) {
        (Some(from), Some(to)) => convert_document(from, to, cli),
        // `required_unless_present_any` makes clap reject a conversion missing `--from`/`--to`
        // before `run` is reached.
        _ => Ok(()),
    }
}

fn convert_document(from: &str, to: &str, cli: &Cli) -> Result<()> {
    let input = read_input(cli.input.as_deref())?;
    let text = String::from_utf8(input)?;

    let output = convert(
        from,
        to,
        &text,
        &ReaderOptions::default(),
        &WriterOptions::default(),
    )?;

    write_output(cli.output.as_deref(), &output)
}

fn print_lines(lines: &[&str]) -> Result<()> {
    let mut out = io::stdout().lock();
    for line in lines {
        writeln!(out, "{line}")?;
    }
    Ok(())
}

fn list_extensions(format: Option<&str>) -> Result<()> {
    let mut out = io::stdout().lock();
    for (extension, enabled) in carta::format_extensions(format)? {
        let sign = if enabled { '+' } else { '-' };
        writeln!(out, "{sign}{}", extension.name())?;
    }
    Ok(())
}

fn read_input(path: Option<&Path>) -> Result<Vec<u8>> {
    if let Some(path) = path {
        Ok(fs::read(path)?)
    } else {
        let mut buffer = Vec::new();
        io::stdin().read_to_end(&mut buffer)?;
        Ok(buffer)
    }
}

fn write_output(path: Option<&Path>, output: &str) -> Result<()> {
    let mut writer: Box<dyn Write> = match path {
        Some(path) => Box::new(fs::File::create(path)?),
        None => Box::new(io::stdout().lock()),
    };
    writer.write_all(output.as_bytes())?;
    writer.write_all(b"\n")?;
    Ok(())
}
