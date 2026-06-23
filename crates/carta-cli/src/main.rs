//! `carta` — command-line interface.
//!
//! Parses `--from`/`--to` and pipes the input through the `carta` library's [`convert`], or — when
//! a `--list-*`/`-D` flag is given — reports what this build supports. Format selection, aliases, the
//! recognized-but-unsupported error, and the introspection data all live in the library; this binary
//! only handles argument parsing, metadata/variable inputs, and stdin/file I/O.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use carta::ast::MetaValue;
use carta::{Error, MathMethod, ReaderOptions, Result, WrapMode, WriterOptions, convert};
use clap::{ArgAction, Parser};

const LIST_FLAGS: [&str; 4] = [
    "list_input_formats",
    "list_output_formats",
    "list_extensions",
    "print_default_template",
];

#[derive(Parser, Debug)]
#[command(
    name = "carta",
    version,
    about = "Document converter",
    disable_version_flag = true
)]
// A command line surfaces each flag as its own field; grouping the boolean toggles into a sub-struct
// would only obscure the one-flag-one-field mapping clap relies on.
#[allow(clippy::struct_excessive_bools)]
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
    /// Produce a standalone document, wrapping the body in the format's template.
    #[arg(short = 's', long = "standalone")]
    standalone: bool,
    /// Render with this template file instead of the format's built-in default; implies `-s`.
    #[arg(long = "template", value_name = "FILE")]
    template: Option<PathBuf>,
    /// Set a template variable: `KEY:VAL` (or `KEY=VAL`), or bare `KEY` for `true`. Repeatable; a
    /// repeated key accumulates into a list.
    #[arg(short = 'V', long = "variable", value_name = "KEY[:VAL]")]
    variable: Vec<String>,
    /// Text wrapping in the output: `auto` reflows to the column width, `none` keeps each block on a
    /// single line, `preserve` keeps the input's own line breaks.
    #[arg(long = "wrap", value_name = "auto|none|preserve", default_value = "auto", value_parser = parse_wrap)]
    wrap: WrapMode,
    /// Column at which `--wrap=auto` reflows text. Defaults to the writer's built-in width.
    #[arg(long = "columns", value_name = "N")]
    columns: Option<usize>,
    /// Number section headings (`1`, `1.1`, …).
    #[arg(short = 'N', long = "number-sections")]
    number_sections: bool,
    /// Include an automatically generated table of contents in the standalone output.
    #[arg(long = "toc", visible_alias = "table-of-contents")]
    toc: bool,
    /// The deepest heading level the table of contents includes (default 3).
    #[arg(long = "toc-depth", value_name = "N")]
    toc_depth: Option<usize>,
    /// Use MathJax to display embedded TeX math in HTML output. An optional URL overrides the script
    /// location.
    #[allow(clippy::option_option)]
    #[arg(long = "mathjax", value_name = "URL", num_args = 0..=1, require_equals = true)]
    mathjax: Option<Option<String>>,
    /// Use KaTeX to display embedded TeX math in HTML output. An optional URL overrides the asset
    /// base location.
    #[allow(clippy::option_option)]
    #[arg(long = "katex", value_name = "URL", num_args = 0..=1, require_equals = true)]
    katex: Option<Option<String>>,
    /// Set a metadata field: `KEY:VAL` (or `KEY=VAL`; `true`/`false` become booleans), or bare `KEY`
    /// for `true`. Repeatable.
    #[arg(short = 'M', long = "metadata", value_name = "KEY[:VAL]")]
    metadata: Vec<String>,
    /// Read metadata from a YAML or JSON file. Repeatable; later files override earlier ones, and all
    /// sit below the document's own metadata.
    #[arg(long = "metadata-file", value_name = "FILE")]
    metadata_file: Vec<PathBuf>,
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
    /// Print the built-in default template for FORMAT and exit.
    #[arg(short = 'D', long = "print-default-template", value_name = "FORMAT")]
    print_default_template: Option<String>,
    /// Print version information and exit.
    #[arg(long = "version", action = ArgAction::Version)]
    version: Option<bool>,
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
    if let Some(format) = &cli.print_default_template {
        return print_default_template(format);
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

    let mut writer_options = WriterOptions::default();
    writer_options.standalone = cli.standalone;
    if let Some(path) = &cli.template {
        writer_options.template = Some(fs::read_to_string(path)?);
        writer_options.template_dir = Some(template_dir(path));
        writer_options.template_ext = Some(
            path.extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("")
                .to_owned(),
        );
    }
    writer_options.wrap = cli.wrap;
    writer_options.columns = cli.columns;
    writer_options.number_sections = cli.number_sections;
    writer_options.toc = cli.toc;
    writer_options.toc_depth = cli.toc_depth;
    writer_options.math_method = math_method(cli);
    writer_options.variables = parse_variables(&cli.variable);
    writer_options.metadata = parse_metadata(&cli.metadata);
    writer_options.metadata_defaults = read_metadata_files(&cli.metadata_file)?;
    writer_options.source_name = Some(source_name(cli.input.as_deref()));

    // A template (default or `--template`) emits verbatim; a bare fragment gets one trailing newline.
    let verbatim = cli.standalone || cli.template.is_some();

    let output = convert(from, to, &text, &ReaderOptions::default(), &writer_options)?;
    write_output(cli.output.as_deref(), &output, verbatim)
}

/// The title an HTML-family standalone document falls back to when no `title` metadata is present:
/// the input file's stem (its name without the final extension), or `-` for standard input.
fn source_name(input: Option<&Path>) -> String {
    match input {
        None => "-".to_owned(),
        Some(path) => path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("-")
            .to_owned(),
    }
}

/// The directory a template's partials resolve against: the template file's own parent (the current
/// directory when the path has no parent component).
fn template_dir(path: &Path) -> PathBuf {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

/// The script URL MathJax loads from when `--mathjax` is given no explicit location.
const DEFAULT_MATHJAX_URL: &str = "https://cdn.jsdelivr.net/npm/mathjax@4/tex-chtml.js";
/// The asset base URL KaTeX loads from when `--katex` is given no explicit location.
const DEFAULT_KATEX_URL: &str = "https://cdn.jsdelivr.net/npm/katex@latest/dist/";

/// Resolve the math renderer from the `--mathjax`/`--katex` flags. Each flag's optional value
/// overrides its default URL; when both are given, MathJax wins. Absent both, math is left as
/// `\(…\)` / `\[…\]` source.
fn math_method(cli: &Cli) -> MathMethod {
    if let Some(url) = &cli.mathjax {
        MathMethod::MathJax(
            url.clone()
                .unwrap_or_else(|| DEFAULT_MATHJAX_URL.to_owned()),
        )
    } else if let Some(url) = &cli.katex {
        MathMethod::Katex(url.clone().unwrap_or_else(|| DEFAULT_KATEX_URL.to_owned()))
    } else {
        MathMethod::Plain
    }
}

/// Parse the `--wrap` argument into a [`WrapMode`].
fn parse_wrap(value: &str) -> std::result::Result<WrapMode, String> {
    match value {
        "auto" => Ok(WrapMode::Auto),
        "none" => Ok(WrapMode::None),
        "preserve" => Ok(WrapMode::Preserve),
        other => Err(format!(
            "invalid wrap mode '{other}' (expected auto, none, or preserve)"
        )),
    }
}

/// Parse `-V` specifiers into raw key/value pairs, defaulting a bare `KEY` to `"true"`. A specifier
/// splits on its first `:` or `=`, whichever comes first, so the value may contain the other.
fn parse_variables(specs: &[String]) -> Vec<(String, String)> {
    specs
        .iter()
        .map(|spec| match spec.split_once([':', '=']) {
            Some((key, value)) => (key.to_owned(), value.to_owned()),
            None => (spec.clone(), "true".to_owned()),
        })
        .collect()
}

/// Parse `-M` specifiers into metadata values: `true`/`false` become booleans, a bare `KEY` becomes
/// `true`, and anything else is a string. A repeated key accumulates its values into a list in order.
/// A specifier splits on its first `:` or `=`, whichever comes first.
fn parse_metadata(specs: &[String]) -> BTreeMap<String, MetaValue> {
    let mut map: BTreeMap<String, MetaValue> = BTreeMap::new();
    for spec in specs {
        let (key, value) = match spec.split_once([':', '=']) {
            Some((key, "true")) => (key, MetaValue::MetaBool(true)),
            Some((key, "false")) => (key, MetaValue::MetaBool(false)),
            Some((key, value)) => (key, MetaValue::MetaString(value.to_owned())),
            None => (spec.as_str(), MetaValue::MetaBool(true)),
        };
        let next = match map.remove(key) {
            None => value,
            Some(MetaValue::MetaList(mut items)) => {
                items.push(value);
                MetaValue::MetaList(items)
            }
            Some(first) => MetaValue::MetaList(vec![first, value]),
        };
        map.insert(key.to_owned(), next);
    }
    map
}

/// Read and merge every `--metadata-file`, later files overriding earlier ones at the key level. A
/// `.json` extension selects the JSON parser; everything else is read as YAML.
fn read_metadata_files(paths: &[PathBuf]) -> Result<BTreeMap<String, MetaValue>> {
    let mut defaults = BTreeMap::new();
    for path in paths {
        let content = fs::read_to_string(path)?;
        let json = path.extension().and_then(|ext| ext.to_str()) == Some("json");
        for (key, value) in carta::parse_metadata_file(&content, json)? {
            defaults.insert(key, value);
        }
    }
    Ok(defaults)
}

fn print_default_template(spec: &str) -> Result<()> {
    let (base, _) = carta::parse_format_spec(spec)?;
    let writer = carta::writer_for(&base)?;
    match writer.default_template() {
        Some(template) => {
            io::stdout().lock().write_all(template.as_bytes())?;
            Ok(())
        }
        None => Err(Error::Template(format!(
            "format '{base}' has no default template"
        ))),
    }
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

fn write_output(path: Option<&Path>, output: &str, verbatim: bool) -> Result<()> {
    let mut writer: Box<dyn Write> = match path {
        Some(path) => Box::new(fs::File::create(path)?),
        None => Box::new(io::stdout().lock()),
    };
    writer.write_all(output.as_bytes())?;
    // A fragment gets exactly one trailing newline; a template's output is emitted byte-for-byte.
    if !verbatim {
        writer.write_all(b"\n")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    // Indexing a metadata map by a key the case has just inserted is the idiomatic assertion here; a
    // missing key should fail the test loudly.
    #![allow(clippy::indexing_slicing)]

    use super::{Cli, parse_metadata, parse_variables, parse_wrap, template_dir};
    use carta::WrapMode;
    use carta::ast::MetaValue;
    use clap::CommandFactory;
    use std::path::{Path, PathBuf};

    fn vars(args: &[&str]) -> Vec<(String, String)> {
        parse_variables(&args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn bare_variable_defaults_to_true() {
        assert_eq!(
            vars(&["flag", "k=v", "eq=a=b"]),
            vec![
                ("flag".to_owned(), "true".to_owned()),
                ("k".to_owned(), "v".to_owned()),
                // Only the first `=` splits, so a value may itself contain `=`.
                ("eq".to_owned(), "a=b".to_owned()),
            ]
        );
    }

    #[test]
    fn variable_splits_on_the_first_colon_or_equals() {
        assert_eq!(
            vars(&["k:v", "colon:a=b", "equals=a:b"]),
            vec![
                ("k".to_owned(), "v".to_owned()),
                // The first separator wins: a `:` before an `=` keeps the `=` in the value.
                ("colon".to_owned(), "a=b".to_owned()),
                ("equals".to_owned(), "a:b".to_owned()),
            ]
        );
    }

    #[test]
    fn metadata_splits_on_the_first_colon_or_equals() {
        let map = parse_metadata(
            &["a:val", "b:true", "c:x=y"]
                .iter()
                .map(|s| (*s).to_owned())
                .collect::<Vec<_>>(),
        );
        assert_eq!(map["a"], MetaValue::MetaString("val".to_owned()));
        assert_eq!(map["b"], MetaValue::MetaBool(true));
        assert_eq!(map["c"], MetaValue::MetaString("x=y".to_owned()));
    }

    #[test]
    fn metadata_typing_distinguishes_booleans_from_strings() {
        let map = parse_metadata(
            &["a=true", "b=false", "c=text", "d", "e=True"]
                .iter()
                .map(|s| (*s).to_owned())
                .collect::<Vec<_>>(),
        );
        assert_eq!(map["a"], MetaValue::MetaBool(true));
        assert_eq!(map["b"], MetaValue::MetaBool(false));
        assert_eq!(map["c"], MetaValue::MetaString("text".to_owned()));
        assert_eq!(map["d"], MetaValue::MetaBool(true));
        // Only lowercase `true`/`false` are booleans; anything else stays a string.
        assert_eq!(map["e"], MetaValue::MetaString("True".to_owned()));
    }

    #[test]
    fn repeated_metadata_key_accumulates_into_a_list() {
        // Two occurrences promote the key to a two-element list, in order.
        let two = parse_metadata(&["k=first".to_owned(), "k=second".to_owned()]);
        assert_eq!(
            two["k"],
            MetaValue::MetaList(vec![
                MetaValue::MetaString("first".to_owned()),
                MetaValue::MetaString("second".to_owned()),
            ])
        );
        // Further occurrences append; a bare first occurrence keeps its boolean element.
        let mixed = parse_metadata(&["k".to_owned(), "k=a".to_owned(), "k=b".to_owned()]);
        assert_eq!(
            mixed["k"],
            MetaValue::MetaList(vec![
                MetaValue::MetaBool(true),
                MetaValue::MetaString("a".to_owned()),
                MetaValue::MetaString("b".to_owned()),
            ])
        );
    }

    #[test]
    fn template_dir_is_the_file_parent_or_current_dir() {
        assert_eq!(template_dir(Path::new("bare.html")), PathBuf::from("."));
        assert_eq!(
            template_dir(Path::new("sub/dir/t.html")),
            PathBuf::from("sub/dir")
        );
        assert_eq!(
            template_dir(Path::new("/abs/t.html")),
            PathBuf::from("/abs")
        );
    }

    #[test]
    fn cli_definition_is_valid() {
        // Catches a clap configuration error (e.g. a duplicate short flag) at test time.
        Cli::command().debug_assert();
    }

    #[test]
    fn wrap_mode_parses_the_three_names_and_rejects_others() {
        assert_eq!(parse_wrap("auto"), Ok(WrapMode::Auto));
        assert_eq!(parse_wrap("none"), Ok(WrapMode::None));
        assert_eq!(parse_wrap("preserve"), Ok(WrapMode::Preserve));
        assert!(parse_wrap("soft").is_err());
    }
}
