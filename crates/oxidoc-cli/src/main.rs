//! `oxidoc` — command-line interface.
//!
//! Parses `--from`/`--to` and pipes the input through the `oxidoc` library's [`convert`]. Format
//! selection, aliases, and the recognized-but-unsupported error all live in the library; this binary
//! only handles argument parsing and stdin/file I/O.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use oxidoc::{Error, ReaderOptions, Result, WriterOptions, convert};

#[derive(Parser, Debug)]
#[command(name = "oxidoc", version, about = "Document converter")]
struct Cli {
    /// Input format (e.g. `commonmark`, `json`).
    #[arg(short = 'f', long = "from")]
    from: Option<String>,
    /// Output format (e.g. `html`, `json`).
    #[arg(short = 't', long = "to")]
    to: Option<String>,
    /// Write output to this file instead of stdout.
    #[arg(short = 'o', long = "output")]
    output: Option<PathBuf>,
    /// Read input from this file instead of stdin.
    input: Option<PathBuf>,
}

fn main() -> ExitCode {
    match run(&Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("oxidoc: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<()> {
    let from = require_flag(cli.from.as_deref(), "--from")?;
    let to = require_flag(cli.to.as_deref(), "--to")?;

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

fn require_flag<'a>(value: Option<&'a str>, flag: &str) -> Result<&'a str> {
    value.ok_or_else(|| Error::UnsupportedFormat(format!("{flag} is required")))
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
