//! Resource resolution: media extraction, disk lookup, and remote fetch for self-contained output.

// A document reaches the host only through `carta::HostAccess`.
#![warn(clippy::disallowed_methods)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(feature = "write-html")]
use carta::Resource;
use carta::{Document, HostAccess, MediaBag, Result, media};

use super::Cli;

/// The directories a referenced resource is looked for in, in search order: those named by
/// `--resource-path` (split on the platform path separator), then the working directory as
/// a final fallback.
pub(super) fn resource_search_path(cli: &Cli) -> Vec<PathBuf> {
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
pub(super) fn resolve_resource(
    reference: &str,
    search_path: &[PathBuf],
    host: HostAccess,
) -> Option<Vec<u8>> {
    let decoded = media::percent_decode(reference).unwrap_or_else(|| reference.to_owned());
    let reference = Path::new(&decoded);
    if reference.is_absolute() {
        return host.read(reference).ok();
    }
    search_path
        .iter()
        .find_map(|dir| host.read(&dir.join(reference)).ok())
}

pub(super) fn gather_local_images(
    document: &mut Document,
    media: &mut MediaBag,
    search_path: &[PathBuf],
    host: HostAccess,
) {
    let mut references = BTreeSet::new();
    carta::walk::for_each_document_image_target(document, &mut |target| {
        references.insert(target.url.to_string());
    });
    for reference in references {
        if media.contains(&reference)
            || reference.starts_with("//")
            || reference.contains("://")
            || reference
                .get(..5)
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case("data:"))
        {
            continue;
        }
        if let Some(bytes) = resolve_resource(&reference, search_path, host) {
            let mime = media::image_mime_for_extension(&reference).map(str::to_owned);
            media.insert(reference, mime, bytes);
        }
    }
}

/// Read the cover image and the stylesheets an EPUB document's metadata names (`cover-image`,
/// `css`, `stylesheet`). A cover from the command line stays; metadata stylesheets come first.
///
/// # Errors
/// A named file that cannot be read.
#[cfg(feature = "write-epub")]
pub(super) fn gather_epub_metadata(
    document: &Document,
    epub: &mut carta::EpubOptions,
    host: HostAccess,
) -> Result<()> {
    let read = |name: &str| {
        host.read(Path::new(name))
            .map_err(|error| std::io::Error::new(error.kind(), format!("{name}: {error}")))
    };
    if epub.cover_image.is_none()
        && let Some(name) = metadata_names(document, "cover-image").first()
    {
        let file_name = Path::new(name).file_name().unwrap_or_default();
        epub.cover_image = Some((file_name.to_string_lossy().into_owned(), read(name)?));
    }
    let mut stylesheets = Vec::new();
    for key in ["css", "stylesheet"] {
        for name in metadata_names(document, key) {
            stylesheets.push(String::from_utf8_lossy(&read(&name)?).into_owned());
        }
    }
    stylesheets.append(&mut epub.stylesheets);
    epub.stylesheets = stylesheets;
    Ok(())
}

/// The file names a metadata entry holds: one for a single value, one per item for a list.
#[cfg(feature = "write-epub")]
fn metadata_names(document: &Document, key: &str) -> Vec<String> {
    use carta::ast::{MetaValue, single_block_inlines, to_plain_text};
    fn name(value: &MetaValue) -> Option<String> {
        match value {
            MetaValue::MetaString(text) => Some(text.to_string()),
            MetaValue::MetaInlines(inlines) => Some(to_plain_text(inlines)),
            MetaValue::MetaBlocks(blocks) => Some(to_plain_text(single_block_inlines(blocks))),
            _ => None,
        }
    }
    match document.meta.get(key) {
        Some(MetaValue::MetaList(values)) => values.iter().filter_map(name).collect(),
        Some(value) => name(value).into_iter().collect(),
        None => Vec::new(),
    }
}

/// Resolve a reference the self-contained HTML pass encounters into its bytes and MIME type, in order:
/// a resource the reader carried in the bag, then a resource retrieved over HTTP(S), then a local
/// file found along `search_path`. A reference that resolves nowhere is left external (the pass
/// keeps it as written).
#[cfg(feature = "write-html")]
pub(super) fn resolve_embed(
    reference: &str,
    bag: &MediaBag,
    search_path: &[PathBuf],
    host: HostAccess,
) -> Option<Resource> {
    if let Some(item) = bag.get(reference) {
        return Some(Resource {
            bytes: item.bytes.clone(),
            mime: item.mime.clone(),
        });
    }
    if is_remote_url(reference) {
        return fetch(reference, host).map(|item| Resource {
            bytes: item.bytes,
            mime: item.mime,
        });
    }
    let bytes = resolve_resource(reference, search_path, host)?;
    Some(Resource {
        bytes,
        mime: mime_for_path(reference),
    })
}

/// Whether a reference is retrieved over the network rather than read from disk.
#[cfg(any(feature = "write-html", feature = "write-pdf", feature = "write-epub"))]
fn is_remote_url(reference: &str) -> bool {
    reference.split_once("://").is_some_and(|(scheme, _)| {
        scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
    })
}

/// The MIME type a reference's file extension implies, for the `data:` URI that inlines it. Covers the
/// resource kinds a self-contained page embeds: images, fonts, media, and stylesheets; an
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

/// Fetch a remote resource, reporting a failure and returning `None`.
#[cfg(any(feature = "write-html", feature = "write-pdf", feature = "write-epub"))]
fn fetch(url: &str, host: HostAccess) -> Option<carta::MediaItem> {
    host.fetch(url)
        .inspect_err(|error| eprintln!("carta: could not fetch {url}: {error}"))
        .ok()
}

/// Fetch a resource over HTTP(S), keeping its reported MIME type.
#[cfg(feature = "fetch")]
pub(super) fn fetch_remote(url: &str) -> std::io::Result<carta::MediaItem> {
    // Embedded images can be large, so lift the client's default read limit.
    const LIMIT: u64 = 128 * 1024 * 1024;
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .max_redirects(5)
        .build();
    let mut response = ureq::Agent::from(config)
        .get(url)
        .call()
        .map_err(std::io::Error::other)?;
    let mime = response.body().mime_type().map(str::to_owned);
    let bytes = response
        .body_mut()
        .with_config()
        .limit(LIMIT)
        .read_to_vec()
        .map_err(std::io::Error::other)?;
    Ok(carta::MediaItem { mime, bytes })
}

/// Write embedded media under `dir` and rewrite image URLs under `references`.
/// Return media under the rewritten URLs, preserving bytes and MIME types.
/// Data URIs use content-addressed names; `preserve_names` keeps other safe resource names.
pub(super) fn extract_media(
    dir: &Path,
    references: &str,
    mut media: MediaBag,
    document: &mut Document,
    preserve_names: bool,
) -> Result<MediaBag> {
    carta::walk::for_each_document_image_target(document, &mut |target| {
        if !media.contains(&target.url)
            && let Some((bytes, mime)) = media::decode_data_uri(&target.url)
        {
            media.insert(target.url.to_string(), Some(mime), bytes);
        }
    });
    for (name, item) in media.iter() {
        let safe = extracted_name(name, item, preserve_names);
        let path = dir.join(&safe);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, &item.bytes)?;
    }
    carta::walk::for_each_document_image_target(document, &mut |target| {
        if let Some(item) = media.get(&target.url) {
            let name = extracted_name(&target.url, item, preserve_names);
            target.url = media::extracted_path(references, &name).into();
        }
    });
    let mut extracted = MediaBag::new();
    for (name, item) in media {
        let name = extracted_name(&name, &item, preserve_names);
        extracted.insert(
            media::extracted_path(references, &name),
            item.mime,
            item.bytes,
        );
    }
    Ok(extracted)
}

fn extracted_name(name: &str, item: &carta::MediaItem, preserve_names: bool) -> String {
    if preserve_names {
        media::extraction_target(name, item)
    } else {
        media::content_addressed_name(
            item.mime.as_deref().unwrap_or("application/octet-stream"),
            &item.bytes,
        )
    }
}

pub(super) fn extract_tex_media(
    output: Option<&Path>,
    media: MediaBag,
    document: &mut Document,
) -> Result<()> {
    let directory = output
        .and_then(Path::parent)
        .unwrap_or_else(|| Path::new("."))
        .join("media");
    extract_media(&directory, "media", media, document, false).map(|_| ())
}

/// Fetch remote images before packaging or external compilation.
#[cfg(any(feature = "write-pdf", feature = "write-epub"))]
pub(super) fn fetch_remote_images(
    document: &mut carta::Document,
    resources: &mut MediaBag,
    host: HostAccess,
) {
    let mut references = BTreeSet::new();
    carta::walk::for_each_document_image_target(document, &mut |target| {
        let url = target.url.as_ref();
        if !resources.contains(url) && is_remote_url(url) {
            references.insert(url.to_owned());
        }
    });
    for url in references {
        if let Some(item) = fetch(&url, host) {
            resources.insert(url, item.mime, item.bytes);
        }
    }
}
