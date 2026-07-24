//! Resource resolution: media extraction, disk lookup, and remote fetch for self-contained output.

use std::fs;
use std::path::{Path, PathBuf};

#[cfg(feature = "write-html")]
use carta::Resource;
use carta::ast::Block;
use carta::{MediaBag, Result, media};

use super::Cli;

/// The directories a referenced resource is looked for in, in search order: those named by
/// `--resource-path` (each entry split on the platform path separator), then the working directory as
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
pub(super) fn resolve_resource(reference: &str, search_path: &[PathBuf]) -> Option<Vec<u8>> {
    let reference = Path::new(reference);
    if reference.is_absolute() {
        return fs::read(reference).ok();
    }
    search_path
        .iter()
        .find_map(|dir| fs::read(dir.join(reference)).ok())
}

/// Resolve a reference the self-contained HTML pass encounters into its bytes and MIME type, in order:
/// a resource the reader carried in the bag, then a local file found along `search_path`, then, with
/// the `fetch` feature, a resource retrieved over HTTP(S). A reference that resolves nowhere is left
/// external (the pass keeps it as written).
#[cfg(feature = "write-html")]
pub(super) fn resolve_embed(
    reference: &str,
    bag: &MediaBag,
    search_path: &[PathBuf],
    sandbox: bool,
) -> Option<Resource> {
    if let Some(item) = bag.get(reference) {
        return Some(Resource {
            bytes: item.bytes.clone(),
            mime: item.mime.clone(),
        });
    }
    if is_remote_url(reference) {
        if sandbox {
            eprintln!("carta: not fetching {reference} (--sandbox); leaving reference external");
            return None;
        }
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

/// Retrieve a resource over HTTP(S) for inlining, returning its bytes and the MIME type its
/// `Content-Type` reports. A network failure, an error status, or an unreadable body is reported and
/// the reference is left external.
#[cfg(all(feature = "write-html", feature = "fetch"))]
fn fetch_remote(url: &str) -> Option<Resource> {
    // Self-contained pages may embed large media; lift the read ceiling above the client default.
    const LIMIT: u64 = 128 * 1024 * 1024;
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .max_redirects(5)
        .build();
    let agent = ureq::Agent::from(config);
    match agent.get(url).call() {
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
pub(super) fn extract_media(dir: &Path, media: &MediaBag, blocks: &mut [Block]) -> Result<()> {
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
