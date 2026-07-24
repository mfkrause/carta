//! Command-line option builders for the EPUB, DOCX, and syntax-highlighting writer configuration.

use std::fs;
use std::path::Path;

use carta::{DocxOptions, EpubOptions, Result};

use super::Cli;

#[cfg(feature = "highlight")]
use carta::Error;
#[cfg(feature = "highlight")]
use std::io::{self, Write};
#[cfg(feature = "highlight")]
use std::path::PathBuf;
#[cfg(feature = "highlight")]
use std::sync::Arc;

/// Assemble the EPUB writer options from the command line: stylesheets, cover image, embedded fonts,
/// the Dublin Core metadata fragment, the container subdirectory, the chapter split level, and the
/// reproducible-build timestamp. Each referenced file is read from disk here.
pub(super) fn epub_options(cli: &Cli) -> Result<EpubOptions> {
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
pub(super) fn docx_options(cli: &Cli) -> Result<DocxOptions> {
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

/// The built-in style code blocks are colorized with when no style is named.
#[cfg(feature = "highlight")]
const DEFAULT_HIGHLIGHT_STYLE: &str = "pygments";

/// Build the syntax-highlighting configuration from the highlight flags. `--no-highlight` and
/// `--syntax-highlighting=none` leave code plain; `idiomatic` selects the format's own listing
/// construct; otherwise a highlighter runs with the chosen (or default) style. `--highlight-style`
/// overrides the style, and each `--syntax-definition` adds (or replaces by name) a language.
// Single-threaded refcounted cache, never shared across threads: the wrap is sound despite the lint.
#[allow(clippy::arc_with_non_send_sync)]
#[cfg(feature = "highlight")]
pub(super) fn highlight_options(
    cli: &Cli,
    data_dir: Option<&Path>,
) -> Result<carta::HighlightOptions> {
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

    // `--highlight-style` wins; else a non-`default` `--syntax-highlighting` path; else built-in.
    let style = cli.highlight_style.as_deref().or(match mode {
        Some("default") | None => None,
        named => named,
    });
    let theme = resolve_theme(style.unwrap_or(DEFAULT_HIGHLIGHT_STYLE))?;

    let mut highlighter = carta::Highlighter::new();
    for path in &cli.syntax_definition {
        let xml = fs::read_to_string(path)?;
        add_syntax_definition(&mut highlighter, path, &xml)
            .map_err(|error| Error::Highlight(format!("{}: {error}", path.display())))?;
    }
    for directory in syntax_directories(data_dir) {
        load_syntax_directory(&mut highlighter, &directory);
    }

    Ok(carta::HighlightOptions {
        highlighter: Some(Arc::new(highlighter)),
        theme: Some(theme),
        idiomatic: false,
    })
}

/// Register a definition under its file stem, matching how bundled definitions resolve.
#[cfg(feature = "highlight")]
fn add_syntax_definition(
    highlighter: &mut carta::Highlighter,
    path: &Path,
    xml: &str,
) -> std::result::Result<String, String> {
    let registry = highlighter.registry_mut();
    match path.file_stem().and_then(|stem| stem.to_str()) {
        Some(stem) => registry.add_definition_with_stem(xml, stem),
        None => registry.add_definition(xml),
    }
    .map_err(|error| error.to_string())
}

/// The directories of runtime-loaded syntax definitions. `$CARTA_SYNTAX_DIR`, when set, is the
/// only one (its empty value disables directory loading). Otherwise the data directory's `syntax/`
/// loads first (a user's definitions win name collisions), then a `syntax` directory beside the
/// executable, the layout the release archives ship the separately licensed grammar pack in.
#[cfg(feature = "highlight")]
fn syntax_directories(data_dir: Option<&Path>) -> Vec<PathBuf> {
    if let Some(configured) = std::env::var_os("CARTA_SYNTAX_DIR") {
        if configured.is_empty() {
            return Vec::new();
        }
        return vec![PathBuf::from(configured)];
    }
    let mut directories = Vec::new();
    if let Some(in_data_dir) = data_dir.map(|dir| dir.join("syntax"))
        && in_data_dir.is_dir()
    {
        directories.push(in_data_dir);
    }
    if let Some(beside_executable) = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("syntax")))
        && beside_executable.is_dir()
    {
        directories.push(beside_executable);
    }
    directories
}

/// Load every `.xml` definition in `directory`, in file-name order. A file that cannot be read or
/// parsed is skipped with a warning: one stale definition should not fail every conversion.
#[cfg(feature = "highlight")]
fn load_syntax_directory(highlighter: &mut carta::Highlighter, directory: &Path) {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!(
                "carta: warning: cannot read syntax directory {}: {error}",
                directory.display()
            );
            return;
        }
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "xml"))
        .collect();
    paths.sort();
    for path in paths {
        let loaded = fs::read_to_string(&path)
            .map_err(|error| error.to_string())
            .and_then(|xml| add_syntax_definition(highlighter, &path, &xml));
        if let Err(error) = loaded {
            eprintln!(
                "carta: warning: skipping syntax definition {}: {error}",
                path.display()
            );
        }
    }
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
pub(super) fn print_highlight_style(spec: &str) -> Result<()> {
    let json = resolve_theme(spec)?
        .to_json()
        .map_err(|error| Error::Highlight(error.to_string()))?;
    let mut out = io::stdout().lock();
    writeln!(out, "{json}")?;
    Ok(())
}
