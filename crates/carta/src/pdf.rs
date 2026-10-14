//! External compilation of standalone Typst documents.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;

use carta_ast::{Document, Inline};
use carta_core::{BytesWriter, Error, Output, PdfEngine, Result, WriterOptions, media, walk};

pub(crate) struct PdfWriter;

impl BytesWriter for PdfWriter {
    fn render_document(&self, document: &Document, options: &WriterOptions) -> Result<Vec<u8>> {
        let directory = tempfile::tempdir()?;
        let directory_path = directory.path().canonicalize()?;
        let working_directory = std::env::current_dir()?.canonicalize()?;
        let root = working_directory
            .ancestors()
            .last()
            .unwrap_or(&working_directory);
        let mut document = document.clone();
        crate::standalone::merge_metadata(&mut document, options);
        stage_images(&mut document, options, &directory_path, root)?;
        let mut options = options.clone();
        options.standalone = true;
        options.metadata.clear();
        options.metadata_defaults.clear();
        let Output::Text(source) =
            crate::render_text("typst", &carta_writers::TypstWriter, document, &mut options)?
        else {
            return Err(Error::Pdf("Typst source is not text".to_owned()));
        };
        let input = directory.path().join("document.typ");
        let output = directory.path().join("document.pdf");
        fs::write(&input, source)?;
        let engine = match options.pdf.engine {
            PdfEngine::Typst => "typst",
        };
        let mut command = Command::new(engine);
        command
            .arg("compile")
            .arg("--root")
            .arg(root)
            .args(&options.pdf.engine_options)
            .arg("--format=pdf")
            .arg("-")
            .arg(&output)
            .stdin(File::open(input)?);
        // Keep generated dates stable unless the caller supplies a build timestamp.
        if std::env::var_os("SOURCE_DATE_EPOCH").is_none() {
            command.env("SOURCE_DATE_EPOCH", "0");
        }
        let result = command.output().map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::PdfEngineNotFound(engine.to_owned())
            } else {
                Error::Pdf(format!("could not run {engine}: {error}"))
            }
        })?;
        if !result.status.success() {
            return Err(Error::Pdf(format!(
                "{engine} exited with {}:\n{}",
                result.status,
                String::from_utf8_lossy(&result.stderr)
            )));
        }
        if !result.stderr.is_empty() {
            eprint!("{}", String::from_utf8_lossy(&result.stderr));
        }
        let bytes =
            fs::read(output).map_err(|error| Error::Pdf(format!("could not read PDF: {error}")))?;
        if !bytes.starts_with(b"%PDF-") {
            return Err(Error::Pdf("engine did not produce PDF output".to_owned()));
        }
        Ok(bytes)
    }
}

fn stage_images(
    document: &mut Document,
    options: &WriterOptions,
    directory: &Path,
    root: &Path,
) -> Result<()> {
    let mut paths = BTreeMap::<String, String>::new();
    let mut result = Ok(());
    walk::for_each_document_inline(document, &mut |inline| {
        let Inline::Image(_, alt, target) = inline else {
            return;
        };
        if result.is_err() {
            return;
        }
        let reference = target.url.to_string();
        if let Some(path) = paths.get(&reference) {
            target.url = path.clone().into();
            return;
        }
        let Some(bytes) = image_bytes(&reference, options) else {
            eprintln!("carta: could not read image '{reference}'; using its description");
            *inline = Inline::Span(Box::default(), std::mem::take(alt));
            return;
        };
        result = (|| {
            // Image decoding uses the file content, so resource names cannot escape this directory.
            let path = directory.join(format!("image-{}", paths.len()));
            fs::write(&path, bytes)?;
            let relative = path
                .strip_prefix(root)
                .map_err(|error| Error::Pdf(error.to_string()))?;
            let path = format!(
                "/{}",
                relative
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/")
            );
            target.url = path.clone().into();
            paths.insert(reference, path);
            Ok(())
        })();
    });
    result
}

fn image_bytes(reference: &str, options: &WriterOptions) -> Option<Vec<u8>> {
    if let Some(item) = options.media.get(reference) {
        return Some(item.bytes.clone());
    }
    if let Some((bytes, _)) = media::decode_data_uri(reference) {
        return Some(bytes);
    }
    let path =
        PathBuf::from(media::percent_decode(reference).unwrap_or_else(|| reference.to_owned()));
    options
        .pdf
        .resource_path
        .iter()
        .map(|directory| directory.join(&path))
        .chain(std::iter::once(path.clone()))
        .find_map(|path| fs::read(path).ok())
}
