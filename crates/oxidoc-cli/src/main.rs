//! `oxidoc` — command-line interface.
//!
//! Slice 0 wires a single conversion path: JSON in, JSON out. Argument parsing and the
//! reader → writer dispatch grow in later slices; for now any format other than `json` is a
//! recognized-but-unsupported error rather than a panic.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use oxidoc_ast::Document;
use oxidoc_core::{Error, Result};

#[derive(Parser, Debug)]
#[command(name = "oxidoc", version, about = "Document converter")]
struct Cli {
    /// Input format (currently only `json`).
    #[arg(short = 'f', long = "from")]
    from: Option<String>,
    /// Output format (currently only `json`).
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
    require_json(cli.from.as_deref(), "--from")?;
    require_json(cli.to.as_deref(), "--to")?;

    let input = read_input(cli.input.as_deref())?;
    let document = oxidoc_ast::from_json(&input)?;
    write_output(cli.output.as_deref(), &document)
}

fn require_json(format: Option<&str>, flag: &str) -> Result<()> {
    match format {
        Some("json") => Ok(()),
        Some(other) => Err(Error::UnsupportedFormat(format!(
            "{other} (only json is supported)"
        ))),
        None => Err(Error::UnsupportedFormat(format!(
            "{flag} is required (only json is supported)"
        ))),
    }
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

fn write_output(path: Option<&Path>, document: &Document) -> Result<()> {
    let mut writer: Box<dyn Write> = match path {
        Some(path) => Box::new(fs::File::create(path)?),
        None => Box::new(io::stdout().lock()),
    };
    oxidoc_ast::to_json_writer(document, &mut writer)?;
    writer.write_all(b"\n")?;
    Ok(())
}
