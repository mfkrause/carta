//! Filename format selection and ordered input loading.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use carta::ast::Document;
use carta::{AnyReader, Error, MediaBag, ReaderOptions, Result, any_reader_for, read_document};

pub(super) fn input_format(paths: &[PathBuf]) -> &'static str {
    if let Some(format) = paths
        .iter()
        .find_map(|path| filename_formats(path).map(|pair| pair.0))
    {
        return format;
    }
    if let Some(path) = paths.iter().find(|path| path.as_path() != Path::new("-")) {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("");
        eprintln!(
            "carta: could not deduce input format from extension '{extension}'; defaulting to markdown"
        );
    }
    "markdown"
}

pub(super) fn output_format(path: Option<&Path>) -> &'static str {
    path.and_then(filename_formats)
        .map_or("html", |pair| pair.1)
}

fn filename_formats(path: &Path) -> Option<(&'static str, &'static str)> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let formats = match extension.as_str() {
        "md" | "markdown" | "mdown" | "mkd" | "mkdn" | "mdwn" | "txt" | "text" => {
            ("markdown", "markdown")
        }
        "lhs" => ("markdown+literate_haskell", "markdown+literate_haskell"),
        "html" | "htm" | "xhtml" => ("html", "html"),
        "tex" | "latex" | "ltx" => ("latex", "latex"),
        "rst" => ("rst", "rst"),
        "adoc" | "asciidoc" => ("rst", "asciidoc"),
        "org" => ("org", "org"),
        "json" => ("json", "json"),
        "native" => ("native", "native"),
        "csv" => ("csv", "csv"),
        "tsv" => ("tsv", "tsv"),
        "opml" => ("opml", "opml"),
        "ipynb" => ("ipynb", "ipynb"),
        "wiki" => ("mediawiki", "mediawiki"),
        "dokuwiki" => ("dokuwiki", "dokuwiki"),
        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" => ("man", "man"),
        "typ" => ("typst", "typst"),
        "db" => ("docbook", "docbook"),
        "xml" => ("docbook", "xml"),
        "rtf" => ("rtf", "rtf"),
        "docx" => ("docx", "docx"),
        "epub" => ("epub", "epub"),
        "odt" => ("odt", "odt"),
        "doc" => ("doc", "doc"),
        "pdf" => ("pdf", "pdf"),
        "textile" => ("textile", "textile"),
        "fb2" => ("fb2", "fb2"),
        "s5" => ("s5", "s5"),
        "context" | "ctx" => ("context", "context"),
        "ms" | "roff" => ("ms", "ms"),
        "texi" | "texinfo" => ("texinfo", "texinfo"),
        _ => return None,
    };
    Some(formats)
}

pub(super) fn read_documents(
    from: &str,
    paths: &[PathBuf],
    options: &ReaderOptions,
) -> Result<(Document, MediaBag)> {
    let base = carta::parse_format_spec(from)?.0;
    let text = matches!(any_reader_for(&base)?, AnyReader::Text(_));
    if !text && paths.len() > 1 {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "multiple inputs require a text format",
        )));
    }
    if paths
        .iter()
        .filter(|path| path.as_path() == Path::new("-"))
        .count()
        > 1
    {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "standard input can only be read once",
        )));
    }

    let mut options = options.clone();
    options.source_dir = super::source_dir(paths.first().map(PathBuf::as_path));
    if paths.is_empty() {
        let bytes = read_input(None, text)?;
        return read_document(from, &bytes, &options);
    }
    if base == "json" && paths.len() > 1 {
        let mut document = Document::default();
        for path in paths {
            let bytes = read_input(Some(path), text)?;
            let (part, _) = read_document(from, &bytes, &options)?;
            document.api_version = part.api_version;
            document.meta.extend(part.meta);
            document.blocks.extend(part.blocks);
        }
        return Ok((document, MediaBag::new()));
    }

    let mut input = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        if index > 0 && text {
            input.extend_from_slice(b"\n\n");
        }
        options.source_dirs.insert(
            input.len(),
            super::source_dir(Some(path)).unwrap_or_default(),
        );
        input.extend(read_input(Some(path), text)?);
    }
    read_document(from, &input, &options)
}

fn read_input(path: Option<&Path>, text: bool) -> Result<Vec<u8>> {
    let path = path.filter(|path| *path != Path::new("-"));
    let input = if let Some(path) = path {
        fs::read(path)
            .map_err(|error| io::Error::new(error.kind(), format!("{}: {error}", path.display())))?
    } else {
        let mut buffer = Vec::new();
        io::stdin().read_to_end(&mut buffer)?;
        buffer
    };
    if !text || std::str::from_utf8(&input).is_ok() {
        return Ok(input);
    }
    let source = path.map_or_else(|| "input".to_owned(), |path| path.display().to_string());
    eprintln!("carta: {source} is not UTF-8 encoded: falling back to latin1");
    Ok(input
        .iter()
        .map(|&byte| char::from(byte))
        .collect::<String>()
        .into_bytes())
}
