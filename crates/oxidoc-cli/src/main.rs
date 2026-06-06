//! `oxidoc` — command-line interface.
//!
//! Parses `--from`/`--to`, selects a [`Reader`] and [`Writer`] by format name, and pipes input
//! text through the conversion. Recognized formats are listed in [`InputFormat`]/[`OutputFormat`];
//! anything else is a recognized-but-unsupported error rather than a panic.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;

use clap::Parser;
use oxidoc_core::{Error, Reader, ReaderOptions, Result, Writer, WriterOptions};
use oxidoc_readers::{CommonmarkReader, JsonReader};
use oxidoc_writers::{HtmlWriter, JsonWriter};

#[derive(Parser, Debug)]
#[command(name = "oxidoc", version, about = "Document converter")]
struct Cli {
    /// Input format: `json` or `commonmark`.
    #[arg(short = 'f', long = "from")]
    from: Option<String>,
    /// Output format: `json` or `html`.
    #[arg(short = 't', long = "to")]
    to: Option<String>,
    /// Write output to this file instead of stdout.
    #[arg(short = 'o', long = "output")]
    output: Option<PathBuf>,
    /// Read input from this file instead of stdin.
    input: Option<PathBuf>,
}

enum InputFormat {
    Json,
    Commonmark,
}

enum OutputFormat {
    Json,
    Html,
}

impl FromStr for InputFormat {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "json" => Ok(Self::Json),
            "commonmark" | "markdown" => Ok(Self::Commonmark),
            other => Err(unsupported(other)),
        }
    }
}

impl FromStr for OutputFormat {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "json" => Ok(Self::Json),
            "html" | "html5" => Ok(Self::Html),
            other => Err(unsupported(other)),
        }
    }
}

impl InputFormat {
    fn reader(&self) -> Box<dyn Reader> {
        match self {
            Self::Json => Box::new(JsonReader),
            Self::Commonmark => Box::new(CommonmarkReader),
        }
    }
}

impl OutputFormat {
    fn writer(&self) -> Box<dyn Writer> {
        match self {
            Self::Json => Box::new(JsonWriter),
            Self::Html => Box::new(HtmlWriter),
        }
    }
}

fn unsupported(format: &str) -> Error {
    Error::UnsupportedFormat(format.to_owned())
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
    let input_format = require_format::<InputFormat>(cli.from.as_deref(), "--from")?;
    let output_format = require_format::<OutputFormat>(cli.to.as_deref(), "--to")?;

    let input = read_input(cli.input.as_deref())?;
    let text = String::from_utf8(input)?;

    let document = input_format
        .reader()
        .read(&text, &ReaderOptions::default())?;
    let output = output_format
        .writer()
        .write(&document, &WriterOptions::default())?;

    write_output(cli.output.as_deref(), &output)
}

fn require_format<T: FromStr<Err = Error>>(format: Option<&str>, flag: &str) -> Result<T> {
    match format {
        Some(value) => value.parse(),
        None => Err(Error::UnsupportedFormat(format!("{flag} is required"))),
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

fn write_output(path: Option<&Path>, output: &str) -> Result<()> {
    let mut writer: Box<dyn Write> = match path {
        Some(path) => Box::new(fs::File::create(path)?),
        None => Box::new(io::stdout().lock()),
    };
    writer.write_all(output.as_bytes())?;
    writer.write_all(b"\n")?;
    Ok(())
}
