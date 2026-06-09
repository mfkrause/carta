//! `carta` — command-line interface.
//!
//! Parses `--from`/`--to` and pipes the input through the `carta` library's [`convert`]. Format
//! selection, aliases, and the recognized-but-unsupported error all live in the library; this binary
//! only handles argument parsing and stdin/file I/O.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use carta::{ReaderOptions, Result, WriterOptions, convert};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "carta", version, about = "Document converter")]
struct Cli {
    /// Input format (e.g. `commonmark`, `json`).
    #[arg(short = 'f', long = "from")]
    from: String,
    /// Output format (e.g. `html`, `json`).
    #[arg(short = 't', long = "to")]
    to: String,
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
            eprintln!("carta: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<()> {
    let input = read_input(cli.input.as_deref())?;
    let text = String::from_utf8(input)?;

    let output = convert(
        &cli.from,
        &cli.to,
        &text,
        &ReaderOptions::default(),
        &WriterOptions::default(),
    )?;

    write_output(cli.output.as_deref(), &output)
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
