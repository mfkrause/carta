//! `carta` — command-line interface.
//!
//! Parses `--from`/`--to` and pipes the input through the `carta` library's [`convert`], or — when
//! a `--list-*`/`-D` flag is given — reports what this build supports. Format selection, aliases, the
//! recognized-but-unsupported error, and the introspection data all live in the library; this binary
//! only handles argument parsing, metadata/variable inputs, and stdin/file I/O.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use carta::ast::{Block, MetaValue};
use carta::{
    DocxOptions, EpubOptions, Error, MathMethod, MediaBag, Output, ReaderOptions, Result, WrapMode,
    WriterOptions, media, read_document, render_document,
};
#[cfg(feature = "write-html")]
use carta::{Resource, inline_resources};
use clap::{ArgAction, Parser};

mod datadir;
mod filters;

#[cfg(not(feature = "highlight"))]
const LIST_FLAGS: [&str; 4] = [
    "list_input_formats",
    "list_output_formats",
    "list_extensions",
    "print_default_template",
];
#[cfg(feature = "highlight")]
const LIST_FLAGS: [&str; 7] = [
    "list_input_formats",
    "list_output_formats",
    "list_extensions",
    "print_default_template",
    "list_highlight_languages",
    "list_highlight_styles",
    "print_highlight_style",
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
    /// Extract the input's embedded media to this directory, writing each resource as a file and
    /// rewriting the document's references to point at the extracted files.
    #[arg(long = "extract-media", value_name = "DIR")]
    extract_media: Option<PathBuf>,
    /// Search these directories, in order, for a document's images and other resources before falling
    /// back to the working directory, when a container format embeds them. Directories are separated
    /// by the platform's path separator (`:` on Unix, `;` on Windows); repeatable.
    #[arg(long = "resource-path", value_name = "SEARCHPATH")]
    resource_path: Vec<String>,
    /// Inline a document's referenced media into HTML output as `data:` URIs, producing a
    /// self-contained file with no external resource dependencies. Local resources are read from disk
    /// (honoring `--resource-path`); references that cannot be read are left as written. Ignored for
    /// non-HTML output.
    #[arg(long = "embed-resources")]
    embed_resources: bool,
    /// Deprecated alias for `--embed-resources --standalone`: produce a self-contained standalone
    /// HTML file.
    #[arg(long = "self-contained")]
    self_contained: bool,
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
    /// The deepest heading level the table of contents includes (default 3; 1–6).
    #[arg(long = "toc-depth", value_name = "N", value_parser = parse_toc_depth)]
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
    /// Transform the document through this JSON filter before writing: a program that reads the
    /// document as JSON on stdin, receives the output format name as its argument, and writes the
    /// transformed JSON on stdout. Repeatable; filters run in the order given.
    #[arg(short = 'F', long = "filter", value_name = "PROGRAM")]
    filter: Vec<String>,
    /// Search this directory for user data — filters in `filters/`, templates in `templates/` —
    /// before the built-in locations. Defaults to `$XDG_DATA_HOME/carta` (or `~/.local/share/carta`).
    #[arg(long = "data-dir", value_name = "DIR")]
    data_dir: Option<PathBuf>,
    /// Style an EPUB with this stylesheet, embedded in the book and linked from every page in place
    /// of the built-in one. Repeatable; several sheets are linked in order.
    #[arg(
        short = 'c',
        long = "css",
        visible_alias = "stylesheet",
        value_name = "FILE"
    )]
    css: Vec<PathBuf>,
    /// Use this image as an EPUB's cover, generating a dedicated cover page.
    #[arg(long = "epub-cover-image", value_name = "FILE")]
    epub_cover_image: Option<PathBuf>,
    /// Embed this font file in an EPUB. Repeatable.
    #[arg(long = "epub-embed-font", value_name = "FILE")]
    epub_embed_font: Vec<PathBuf>,
    /// Merge Dublin Core metadata from this XML file into an EPUB's package document.
    #[arg(long = "epub-metadata", value_name = "FILE")]
    epub_metadata: Option<PathBuf>,
    /// Hold an EPUB's content in this container subdirectory (default `EPUB`; empty for the archive
    /// root).
    #[arg(long = "epub-subdirectory", value_name = "DIRNAME")]
    epub_subdirectory: Option<String>,
    /// Split the document into separate files at this heading level (EPUB). `--epub-chapter-level` is
    /// an accepted alias.
    #[arg(long = "split-level", visible_alias = "epub-chapter-level", value_name = "N", value_parser = parse_split_level)]
    split_level: Option<usize>,
    /// Style a DOCX from this reference document: its styling parts and document template are reused,
    /// and the converted content is generated into it.
    #[arg(long = "reference-doc", value_name = "FILE")]
    reference_doc: Option<PathBuf>,
    /// Colorize code blocks with this style: a built-in name (`--list-highlight-styles`) or a JSON
    /// theme file. Overrides the style selected by `--syntax-highlighting`.
    #[cfg(feature = "highlight")]
    #[arg(long = "highlight-style", value_name = "STYLE|FILE")]
    highlight_style: Option<String>,
    /// Leave code blocks unhighlighted (equivalent to `--syntax-highlighting=none`).
    #[cfg(feature = "highlight")]
    #[arg(long = "no-highlight")]
    no_highlight: bool,
    /// How code blocks are presented: `default` colorizes them, `none` leaves them plain, `idiomatic`
    /// uses the target format's own listing construct, and any other value names a built-in style or a
    /// JSON theme file to colorize with.
    #[cfg(feature = "highlight")]
    #[arg(
        long = "syntax-highlighting",
        value_name = "none|default|idiomatic|STYLE|FILE"
    )]
    syntax_highlighting: Option<String>,
    /// Load an additional syntax definition (a KDE-syntax XML file) whose language joins the catalog,
    /// overriding a built-in of the same name. Repeatable.
    #[cfg(feature = "highlight")]
    #[arg(long = "syntax-definition", value_name = "FILE")]
    syntax_definition: Vec<PathBuf>,
    /// List the languages this build can highlight and exit.
    #[cfg(feature = "highlight")]
    #[arg(long = "list-highlight-languages")]
    list_highlight_languages: bool,
    /// List the built-in highlight styles and exit.
    #[cfg(feature = "highlight")]
    #[arg(long = "list-highlight-styles")]
    list_highlight_styles: bool,
    /// Print a highlight style as a JSON theme — a built-in name or a theme file — and exit.
    #[cfg(feature = "highlight")]
    #[arg(long = "print-highlight-style", value_name = "STYLE|FILE")]
    print_highlight_style: Option<String>,
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
        Err(error) if is_broken_pipe(&error) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("carta: {error}");
            exit_code(&error)
        }
    }
}

/// A closed downstream pipe (`carta … | head`) is not an error: a well-behaved filter terminates
/// silently when its consumer goes away.
fn is_broken_pipe(error: &Error) -> bool {
    matches!(error, Error::Io(io) if io.kind() == io::ErrorKind::BrokenPipe)
}

/// Maps a conversion error to a process exit status. A toggle naming an extension the format does
/// not accept, and a filter failure, each get their own status distinct from the generic failure
/// code, so callers can tell those requests apart from other errors.
fn exit_code(error: &Error) -> ExitCode {
    match error {
        Error::UnsupportedExtension { .. } => ExitCode::from(23),
        Error::Filter(_) => ExitCode::from(83),
        _ => ExitCode::FAILURE,
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
    #[cfg(feature = "highlight")]
    {
        if cli.list_highlight_languages {
            return print_owned_lines(&carta::languages());
        }
        if cli.list_highlight_styles {
            return print_owned_lines(&carta::styles());
        }
        if let Some(style) = &cli.print_highlight_style {
            return print_highlight_style(style);
        }
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
    // The base output format, without its `+ext`/`-ext` toggles: the name passed to each filter and
    // the format whose default template a data-directory override replaces.
    let to_base = carta::parse_format_spec(to)?.0;
    let data_dir = datadir::resolve(cli.data_dir.as_deref());

    // `--self-contained` is the deprecated spelling of `--embed-resources --standalone`.
    if cli.self_contained {
        eprintln!("carta: --self-contained is deprecated; use --embed-resources --standalone");
    }
    // Media embedding inlines resources as `data:` URIs, and only the HTML family (`html`, `html4`,
    // `html5`) renders them, so the request is dropped for any other target (as a container packs its
    // media a different way).
    #[cfg(feature = "write-html")]
    let embed_resources = (cli.embed_resources || cli.self_contained) && to.starts_with("html");

    let mut writer_options = WriterOptions::default();
    writer_options.standalone = cli.standalone || cli.self_contained;
    if let Some((source, dir, ext)) = resolve_template(cli, &to_base, data_dir.as_deref())? {
        writer_options.template = Some(source.into());
        writer_options.template_dir = Some(dir);
        writer_options.template_ext = Some(ext);
    }
    writer_options.template_datadir = data_dir.as_ref().map(|dir| dir.join("templates"));
    writer_options.wrap = cli.wrap;
    writer_options.columns = cli.columns;
    writer_options.number_sections = cli.number_sections;
    writer_options.toc = cli.toc;
    writer_options.toc_depth = cli.toc_depth;
    writer_options.math_method = math_method(cli);
    #[cfg(feature = "highlight")]
    {
        writer_options.highlight = highlight_options(cli)?;
    }
    writer_options.variables = parse_variables(&cli.variable);
    writer_options.metadata = parse_metadata(&cli.metadata);
    writer_options.metadata_defaults = read_metadata_files(&cli.metadata_file)?;
    writer_options.source_name = Some(source_name(cli.input.as_deref()));
    if is_docx(to) {
        writer_options.docx = docx_options(cli)?;
    } else if to.starts_with("epub") {
        writer_options.epub = Arc::new(epub_options(cli)?);
    }

    // A template (default or `--template`) emits verbatim; a bare fragment gets one trailing newline.
    let verbatim = writer_options.standalone || cli.template.is_some();

    let (mut document, resources) = read_document(from, &input, &ReaderOptions::default())?;

    // Fold the metadata layers into the document before any filter runs, so a filter observes the
    // same metadata the writer will, and can delete or rewrite it. The layers are then cleared so
    // rendering does not apply them a second time (which would resurrect a filter-deleted `-M` key).
    carta::merge_metadata(&mut document, &writer_options);
    writer_options.metadata.clear();
    writer_options.metadata_defaults.clear();

    let mut resources = match &cli.extract_media {
        // Extraction turns the embedded resources into external files the document points at, so the
        // writer no longer re-embeds them: it renders against an empty bag.
        Some(dir) => {
            extract_media(dir, &resources, &mut document.blocks)?;
            MediaBag::new()
        }
        None => resources,
    };

    // Filters run after extraction (so they see the rewritten resource references) and before a
    // container packs its media (so resources a filter introduces are still gathered).
    filters::run(&mut document, &cli.filter, &to_base, data_dir.as_deref())?;

    // A container format embeds the resources it references, so pull the local ones off disk here for
    // the writer to carry. HTML self-contained mode instead inlines resources after rendering (the
    // post-render pass below), which lets it reach raw-HTML and stylesheet references the tree omits.
    if cli.extract_media.is_none() && embeds_resources(to) {
        let search_path = resource_search_path(cli);
        media::embed_referenced_media(&mut document.blocks, &mut resources, |reference| {
            resolve_resource(reference, &search_path)
        });
    }

    // Self-contained HTML inlines resources after rendering, resolving each reference the finished
    // page carries; the bag travels alongside so a reader-carried resource resolves without disk I/O.
    #[cfg(feature = "write-html")]
    let embed_bag = if embed_resources && cli.extract_media.is_none() {
        resources.clone()
    } else {
        MediaBag::new()
    };

    let output = render_document(to, document, resources, &writer_options)?;

    #[cfg(feature = "write-html")]
    let output = match output {
        Output::Text(html) if embed_resources && cli.extract_media.is_none() => {
            let search_path = resource_search_path(cli);
            Output::Text(inline_resources(&html, |reference| {
                resolve_embed(reference, &embed_bag, &search_path)
            }))
        }
        output => output,
    };

    write_output(cli.output.as_deref(), &output, verbatim)
}

/// Read `path` to a string, treating its absence as a miss rather than an error: `Ok(None)` when the
/// file does not exist, any other I/O failure propagated. Templates are looked up across several
/// locations in turn, so a missing candidate is expected.
fn try_read(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(source) => Ok(Some(source)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Resolve the standalone template the writer options should carry.
///
/// A `--template` argument is a file path, or — failing that — a name looked up under the data
/// directory's `templates/`. With no `--template` but standalone output requested, a
/// `templates/default.<ext>` in the data directory overrides the format's built-in template, where
/// `<ext>` is the format's template extension. Returns the template source together with the
/// directory its partials resolve against and the extension they inherit; `None` leaves the built-in
/// default in place.
fn resolve_template(
    cli: &Cli,
    to_base: &str,
    data_dir: Option<&Path>,
) -> Result<Option<(String, PathBuf, String)>> {
    if let Some(name) = &cli.template {
        return resolve_named_template(name, data_dir).map(Some);
    }
    if cli.standalone
        && let Some(dir) = data_dir
    {
        let dir = dir.join("templates");
        let extension = default_template_extension(to_base);
        if let Some(source) = try_read(&dir.join(format!("default.{extension}")))? {
            return Ok(Some((source, dir, extension.to_owned())));
        }
    }
    Ok(None)
}

/// Resolve a `--template NAME`: read `NAME` as a file path, or — when that names nothing — a file of
/// that name under the data directory's `templates/`. The partial directory is the template's own
/// parent (or the data `templates/`), and partials inherit the template's file extension.
fn resolve_named_template(
    name: &Path,
    data_dir: Option<&Path>,
) -> Result<(String, PathBuf, String)> {
    let extension = name
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_owned();
    if let Some(source) = try_read(name)? {
        return Ok((source, template_dir(name), extension));
    }
    if let Some(dir) = data_dir {
        let dir = dir.join("templates");
        if let Some(source) = try_read(&dir.join(name))? {
            return Ok((source, dir, extension));
        }
    }
    Err(Error::Template(format!(
        "could not find template '{}'",
        name.display()
    )))
}

/// The file extension of a format's default template. Most formats name it after themselves; the
/// HTML family shares one template per major version, and `gfm` shares the `commonmark` template.
fn default_template_extension(to_base: &str) -> &str {
    match to_base {
        "html" | "html5" => "html5",
        "html4" => "html4",
        "gfm" => "commonmark",
        other => other,
    }
}

/// Whether the target format packs the resources a document references into its output, so those
/// resources must be resolved off disk before rendering.
fn embeds_resources(to: &str) -> bool {
    to.starts_with("epub") || is_docx(to) || is_rtf(to) || is_odt(to)
}

/// Whether the target format is DOCX (any extension toggles follow the base name).
fn is_docx(to: &str) -> bool {
    to.starts_with("docx")
}

/// Whether the target format is ODT (any extension toggles follow the base name). ODT embeds the
/// images it references, so their bytes are gathered off disk before rendering.
fn is_odt(to: &str) -> bool {
    to.starts_with("odt")
}

/// Whether the target format is RTF (any extension toggles follow the base name). RTF embeds the
/// images it references, so their bytes are gathered before rendering.
fn is_rtf(to: &str) -> bool {
    to.starts_with("rtf")
}

/// Assemble the EPUB writer options from the command line: stylesheets, cover image, embedded fonts,
/// the Dublin Core metadata fragment, the container subdirectory, the chapter split level, and the
/// reproducible-build timestamp. Each referenced file is read from disk here.
fn epub_options(cli: &Cli) -> Result<EpubOptions> {
    let mut epub = EpubOptions::default();
    for path in &cli.css {
        epub.stylesheets.push(fs::read_to_string(path)?);
    }
    if let Some(path) = &cli.epub_cover_image {
        epub.cover_image = Some((base_name(path), fs::read(path)?));
    }
    for path in &cli.epub_embed_font {
        epub.fonts.push((base_name(path), fs::read(path)?));
    }
    if let Some(path) = &cli.epub_metadata {
        epub.metadata_xml = Some(fs::read_to_string(path)?);
    }
    epub.subdirectory.clone_from(&cli.epub_subdirectory);
    epub.split_level = cli.split_level;
    epub.source_date_epoch = source_date_epoch();
    epub.locale = std::env::var("LANG").ok();
    Ok(epub)
}

/// Assemble the DOCX writer options from the command line: the reference document read from disk,
/// and the reproducible-build timestamp and locale fallback shared with the EPUB writer.
fn docx_options(cli: &Cli) -> Result<DocxOptions> {
    let mut docx = DocxOptions::default();
    if let Some(path) = &cli.reference_doc {
        docx.reference_doc = Some(fs::read(path)?);
    }
    docx.source_date_epoch = source_date_epoch();
    docx.locale = std::env::var("LANG").ok();
    Ok(docx)
}

/// The final component of a path, as an owned string; empty when the path has none.
fn base_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_owned()
}

/// The reproducible-build timestamp, in seconds since the Unix epoch, read from `SOURCE_DATE_EPOCH`.
/// An unset or unparsable value leaves the writer's fixed default in place.
fn source_date_epoch() -> Option<i64> {
    std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
}

/// The directories a referenced resource is looked for in, in search order: those named by
/// `--resource-path` (each entry split on the platform path separator), then the working directory as
/// a final fallback.
fn resource_search_path(cli: &Cli) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = cli
        .resource_path
        .iter()
        .flat_map(std::env::split_paths)
        .collect();
    dirs.push(PathBuf::from("."));
    dirs
}

/// Read the file a document references, trying each directory of `search_path` in turn; an absolute
/// reference is read directly. Returns the bytes, or `None` when no directory holds the file (or the
/// reference is not a readable local path), leaving the reference as written.
fn resolve_resource(reference: &str, search_path: &[PathBuf]) -> Option<Vec<u8>> {
    let reference = Path::new(reference);
    if reference.is_absolute() {
        return fs::read(reference).ok();
    }
    search_path
        .iter()
        .find_map(|dir| fs::read(dir.join(reference)).ok())
}

/// Resolve a reference the self-contained HTML pass encounters into its bytes and MIME type, in order:
/// a resource the reader carried in the bag, then a local file found along `search_path`, then — with
/// the `fetch` feature — a resource retrieved over HTTP(S). A reference that resolves nowhere is left
/// external (the pass keeps it as written).
#[cfg(feature = "write-html")]
fn resolve_embed(reference: &str, bag: &MediaBag, search_path: &[PathBuf]) -> Option<Resource> {
    if let Some(item) = bag.get(reference) {
        return Some(Resource {
            bytes: item.bytes.clone(),
            mime: item.mime.clone(),
        });
    }
    if is_remote_url(reference) {
        return fetch_remote(reference);
    }
    let bytes = resolve_resource(reference, search_path)?;
    Some(Resource {
        bytes,
        mime: mime_for_path(reference),
    })
}

/// Whether a reference is retrieved over the network rather than read from disk.
#[cfg(feature = "write-html")]
fn is_remote_url(reference: &str) -> bool {
    reference.starts_with("http://") || reference.starts_with("https://")
}

/// The MIME type a reference's file extension implies, for the `data:` URI that inlines it. Covers the
/// resource kinds a self-contained page embeds — images, fonts, media, and stylesheets; an
/// unrecognized extension yields `None`, leaving the generic binary type to stand in.
#[cfg(feature = "write-html")]
fn mime_for_path(reference: &str) -> Option<String> {
    let path = reference.split(['?', '#']).next().unwrap_or(reference);
    let extension = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    let mime = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "eot" => "application/vnd.ms-fontobject",
        "pdf" => "application/pdf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        _ => return None,
    };
    Some(mime.to_owned())
}

/// Retrieve a resource over HTTP(S) for inlining, returning its bytes and the MIME type its
/// `Content-Type` reports. A network failure, an error status, or an unreadable body is reported and
/// the reference is left external.
#[cfg(all(feature = "write-html", feature = "fetch"))]
fn fetch_remote(url: &str) -> Option<Resource> {
    // A self-contained page may legitimately embed large media, so lift the read ceiling well above
    // the client's conservative default rather than truncate a big resource mid-download.
    const LIMIT: u64 = 128 * 1024 * 1024;
    match ureq::get(url).call() {
        Ok(mut response) => {
            let mime = response.body().mime_type().map(str::to_owned);
            match response.body_mut().with_config().limit(LIMIT).read_to_vec() {
                Ok(bytes) => Some(Resource { bytes, mime }),
                Err(error) => {
                    eprintln!("carta: could not read {url}: {error}");
                    None
                }
            }
        }
        Err(error) => {
            eprintln!("carta: could not fetch {url}: {error}");
            None
        }
    }
}

/// Without the `fetch` feature the tool retrieves no remote resource; the reference is reported and
/// left external.
#[cfg(all(feature = "write-html", not(feature = "fetch")))]
fn fetch_remote(url: &str) -> Option<Resource> {
    eprintln!("carta: cannot fetch {url}: built without network support");
    None
}

/// Writes every resource in `media` to a file under `dir` (`<dir>/<name>`, creating parent
/// directories) and rewrites the document's references to those resources to point at the files.
fn extract_media(dir: &Path, media: &MediaBag, blocks: &mut [Block]) -> Result<()> {
    for (name, item) in media.iter() {
        let safe = media::extraction_target(name, item);
        let path = dir.join(&safe);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, &item.bytes)?;
    }
    media::rewrite_extracted_references(blocks, media, &dir.to_string_lossy());
    Ok(())
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

/// The built-in style code blocks are colorized with when no style is named.
#[cfg(feature = "highlight")]
const DEFAULT_HIGHLIGHT_STYLE: &str = "pygments";

/// Build the syntax-highlighting configuration from the highlight flags. `--no-highlight` and
/// `--syntax-highlighting=none` leave code plain; `idiomatic` selects the format's own listing
/// construct; otherwise a highlighter runs with the chosen (or default) style. `--highlight-style`
/// overrides the style, and each `--syntax-definition` adds — or replaces by name — a language.
// The highlighter caches tokenization with single-threaded reference counting; conversion never
// shares it across threads, so wrapping it for the writer options is sound despite the lint.
#[allow(clippy::arc_with_non_send_sync)]
#[cfg(feature = "highlight")]
fn highlight_options(cli: &Cli) -> Result<carta::HighlightOptions> {
    let mode = cli.syntax_highlighting.as_deref();
    if cli.no_highlight || mode == Some("none") {
        return Ok(carta::HighlightOptions::default());
    }
    if mode == Some("idiomatic") {
        return Ok(carta::HighlightOptions {
            idiomatic: true,
            ..carta::HighlightOptions::default()
        });
    }

    // An explicit `--highlight-style` wins; otherwise a style or theme path named by
    // `--syntax-highlighting` (anything but the bare `default`); otherwise the built-in default.
    let style = cli.highlight_style.as_deref().or(match mode {
        Some("default") | None => None,
        named => named,
    });
    let theme = resolve_theme(style.unwrap_or(DEFAULT_HIGHLIGHT_STYLE))?;

    let mut highlighter = carta::Highlighter::new();
    for path in &cli.syntax_definition {
        let xml = fs::read_to_string(path)?;
        highlighter
            .registry_mut()
            .add_definition(&xml)
            .map_err(|error| Error::Highlight(format!("{}: {error}", path.display())))?;
    }

    Ok(carta::HighlightOptions {
        highlighter: Some(Arc::new(highlighter)),
        theme: Some(theme),
        idiomatic: false,
    })
}

/// Resolve a highlight style: a built-in name takes priority over a same-named file, and any other
/// value is read as a JSON theme file.
#[cfg(feature = "highlight")]
fn resolve_theme(spec: &str) -> Result<carta::Theme> {
    if let Some(result) = carta::builtin_style(spec) {
        return result.map_err(|error| Error::Highlight(format!("style '{spec}': {error}")));
    }
    let bytes = fs::read(spec)?;
    carta::Theme::from_json(&bytes).map_err(|error| Error::Highlight(format!("{spec}: {error}")))
}

/// Print a highlight style as a JSON theme and exit.
#[cfg(feature = "highlight")]
fn print_highlight_style(spec: &str) -> Result<()> {
    let json = resolve_theme(spec)?
        .to_json()
        .map_err(|error| Error::Highlight(error.to_string()))?;
    let mut out = io::stdout().lock();
    writeln!(out, "{json}")?;
    Ok(())
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

/// Parse `--toc-depth`, accepting only the 1–6 heading levels a table of contents can reach.
fn parse_toc_depth(value: &str) -> std::result::Result<usize, String> {
    match value.parse::<usize>() {
        Ok(depth @ 1..=6) => Ok(depth),
        _ => Err(format!("'{value}' is not a heading level between 1 and 6")),
    }
}

fn parse_split_level(value: &str) -> std::result::Result<usize, String> {
    match value.parse::<usize>() {
        Ok(level @ 1..=6) => Ok(level),
        _ => Err(format!("'{value}' is not a heading level between 1 and 6")),
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
            Some((key, value)) => (key, MetaValue::MetaString(value.into())),
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
    let writer = carta::any_writer_for(&base)?;
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

/// Print one owned string per line. A companion to [`print_lines`] for the listings the highlight
/// catalog hands back as owned strings.
#[cfg(feature = "highlight")]
fn print_owned_lines(lines: &[String]) -> Result<()> {
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

fn write_output(path: Option<&Path>, output: &Output, verbatim: bool) -> Result<()> {
    if matches!(output, Output::Bytes(_)) && binary_to_terminal(path) {
        return Err(Error::Io(io::Error::other(
            "refusing to write binary output to a terminal (use -o FILE or redirect stdout)",
        )));
    }

    let mut writer: Box<dyn Write> = match path {
        Some(path) => Box::new(fs::File::create(path)?),
        None => Box::new(io::stdout().lock()),
    };
    match output {
        Output::Text(text) => {
            writer.write_all(text.as_bytes())?;
            // A fragment gets exactly one trailing newline; a template's output is emitted byte-for-byte.
            if !verbatim {
                writer.write_all(b"\n")?;
            }
        }
        // Binary output is emitted exactly, with no trailing newline.
        Output::Bytes(bytes) => writer.write_all(bytes)?,
    }
    Ok(())
}

/// Whether writing would send binary output straight to a terminal — no `-o FILE` and stdout is a
/// tty. Such output corrupts the terminal, so it is refused.
fn binary_to_terminal(path: Option<&Path>) -> bool {
    path.is_none() && io::stdout().is_terminal()
}

#[cfg(test)]
mod tests {
    // Indexing a metadata map by a key the case has just inserted is the idiomatic assertion here; a
    // missing key should fail the test loudly.
    #![allow(clippy::indexing_slicing)]

    use super::{Cli, parse_metadata, parse_toc_depth, parse_variables, parse_wrap, template_dir};
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
        assert_eq!(map["a"], MetaValue::MetaString("val".into()));
        assert_eq!(map["b"], MetaValue::MetaBool(true));
        assert_eq!(map["c"], MetaValue::MetaString("x=y".into()));
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
        assert_eq!(map["c"], MetaValue::MetaString("text".into()));
        assert_eq!(map["d"], MetaValue::MetaBool(true));
        // Only lowercase `true`/`false` are booleans; anything else stays a string.
        assert_eq!(map["e"], MetaValue::MetaString("True".into()));
    }

    #[test]
    fn repeated_metadata_key_accumulates_into_a_list() {
        // Two occurrences promote the key to a two-element list, in order.
        let two = parse_metadata(&["k=first".to_owned(), "k=second".to_owned()]);
        assert_eq!(
            two["k"],
            MetaValue::MetaList(vec![
                MetaValue::MetaString("first".into()),
                MetaValue::MetaString("second".into()),
            ])
        );
        // Further occurrences append; a bare first occurrence keeps its boolean element.
        let mixed = parse_metadata(&["k".to_owned(), "k=a".to_owned(), "k=b".to_owned()]);
        assert_eq!(
            mixed["k"],
            MetaValue::MetaList(vec![
                MetaValue::MetaBool(true),
                MetaValue::MetaString("a".into()),
                MetaValue::MetaString("b".into()),
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

    #[test]
    fn toc_depth_accepts_one_through_six_and_rejects_the_rest() {
        assert_eq!(parse_toc_depth("1"), Ok(1));
        assert_eq!(parse_toc_depth("6"), Ok(6));
        assert!(parse_toc_depth("0").is_err());
        assert!(parse_toc_depth("7").is_err());
        assert!(parse_toc_depth("two").is_err());
    }
}
