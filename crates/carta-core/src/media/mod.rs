//! Embedded resources stored in a [`MediaBag`] alongside the document tree.
//!
//! Readers populate the bag; writers embed its bytes; extraction writes files and rewrites references.

mod base64;
mod sha1;

pub use base64::{
    decode as base64_decode, encode as base64_encode, encode_mime as base64_encode_mime,
};
pub use sha1::hex as sha1_hex;

use crate::walk;
use carta_ast::Block;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

/// Decode the `%XX` percent-escapes in `url`, returning the decoded string, or `None` when an escape
/// is truncated or malformed or the decoded bytes are not valid UTF-8.
#[must_use]
pub fn percent_decode(url: &str) -> Option<String> {
    let bytes = url.as_bytes();
    let mut decoded: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'%' {
            let high = bytes.get(index + 1).copied().and_then(hex_digit)?;
            let low = bytes.get(index + 2).copied().and_then(hex_digit)?;
            decoded.push(high << 4 | low);
            index += 3;
        } else {
            decoded.push(byte);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// One entry in a [`MediaBag`]: a resource's bytes together with its MIME type, when the source
/// recorded one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaItem {
    /// The resource's MIME type, when known.
    pub mime: Option<String>,
    /// The resource's raw bytes.
    pub bytes: Vec<u8>,
}

/// A document's embedded resources, keyed by the name the document references each one under.
///
/// Entries are held in sorted order, so iteration (and any file extraction or re-embedding derived
/// from it) is byte-reproducible across runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MediaBag {
    items: BTreeMap<String, MediaItem>,
}

impl MediaBag {
    /// An empty bag.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records `bytes` under `name`, replacing any existing entry with that name.
    pub fn insert(&mut self, name: impl Into<String>, mime: Option<String>, bytes: Vec<u8>) {
        self.items.insert(name.into(), MediaItem { mime, bytes });
    }

    /// The entry recorded under `name`, if any.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&MediaItem> {
        self.items.get(name)
    }

    /// Whether an entry is recorded under `name`.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.items.contains_key(name)
    }

    /// The entries in name order, as `(name, item)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &MediaItem)> {
        self.items.iter().map(|(name, item)| (name.as_str(), item))
    }

    /// The name of every entry, in sorted order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.items.keys().map(String::as_str)
    }

    /// The number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the bag holds no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Decodes a `data:` URI into its raw bytes and MIME type, for a resource embedded directly in a
/// reference. Only a base64 payload is decoded; a reference that is not a `data:` URI, carries no
/// base64 marker, or holds malformed base64 yields `None`. A payload whose header names no type
/// reports `text/plain`.
#[must_use]
pub fn decode_data_uri(url: &str) -> Option<(Vec<u8>, String)> {
    let rest = url.strip_prefix("data:")?;
    let (header, payload) = rest.split_once(',')?;
    let header = header.strip_suffix(";base64")?;
    let mime = match header.split(';').next() {
        Some(kind) if !kind.is_empty() => kind,
        _ => "text/plain",
    };
    let bytes = base64_decode(payload)?;
    Some((bytes, mime.to_owned()))
}

/// The conventional file extension (without a leading dot) for an image-like MIME type. An
/// unrecognized type falls back to its subtype with any structured-syntax suffix removed, so
/// `image/svg+xml` yields `svg`.
#[must_use]
pub fn extension_for_mime(mime: &str) -> &str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        "application/pdf" => "pdf",
        other => other
            .rsplit('/')
            .next()
            .and_then(|subtype| subtype.split('+').next())
            .unwrap_or(other),
    }
}

/// A content-addressed name for a resource: the SHA-1 of its bytes, then a dot, then the extension
/// its MIME type implies. Identical bytes therefore always resolve to the same name.
#[must_use]
pub fn content_addressed_name(mime: &str, bytes: &[u8]) -> String {
    format!("{}.{}", sha1_hex(bytes), extension_for_mime(mime))
}

/// Return an extraction path that remains inside the destination directory.
///
/// Safe nested paths remain unchanged. Absolute paths, drive prefixes, and escaping `..`
/// components use a content-addressed single-segment name. Extraction and link rewriting share it.
#[must_use]
pub fn extraction_target(name: &str, item: &MediaItem) -> String {
    match sanitized_relative(name) {
        Some(safe) => safe,
        None => content_addressed_name(
            item.mime.as_deref().unwrap_or("application/octet-stream"),
            &item.bytes,
        ),
    }
}

/// A forward-slash-joined relative path when `name` stays within its base
/// directory (only plain `Normal` components, at least one of them); `None` when
/// any component is a root, a drive/UNC prefix, or `..`.
fn sanitized_relative(name: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for component in Path::new(name).components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str()?),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

/// The path a resource named `name` occupies when a document's media is extracted under `dir`: the
/// directory and the name joined with `/`. A reference rewritten to this path resolves to the file
/// the extraction writes out.
#[must_use]
pub fn extracted_path(dir: &str, name: &str) -> String {
    format!("{}/{}", dir.trim_end_matches('/'), name)
}

/// Rewrites every image reference in `blocks` that names an entry of `media` to the path it occupies
/// once extracted under `dir` (see [`extracted_path`]), turning an embedded resource into an external
/// file reference. A reference to anything the bag does not hold is left untouched.
pub fn rewrite_extracted_references(blocks: &mut [Block], media: &MediaBag, dir: &str) {
    walk::for_each_image_target(blocks, &mut |target| {
        if let Some(item) = media.get(target.url.as_str()) {
            let safe = extraction_target(target.url.as_str(), item);
            target.url = extracted_path(dir, &safe).into();
        }
    });
}

/// Pulls into `media` every image the document references but does not already carry, so a container
/// writer can embed it. Each distinct reference is offered to `resolve`, which turns it into the
/// resource's bytes (reading a file, fetching a URL, whatever the caller supports) or returns `None`
/// to leave the reference as written. References already held in the bag, and those that name a URL or
/// an inline `data:` payload, are skipped without consulting the resolver. The resource is recorded
/// under the reference itself, with the MIME type its extension implies.
pub fn embed_referenced_media(
    blocks: &mut [Block],
    media: &mut MediaBag,
    mut resolve: impl FnMut(&str) -> Option<Vec<u8>>,
) {
    let mut references: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    walk::for_each_image_target(blocks, &mut |target| {
        let url = target.url.to_string();
        if seen.insert(url.clone()) {
            references.push(url);
        }
    });
    for url in references {
        if media.contains(&url) || is_remote_reference(&url) {
            continue;
        }
        if let Some(bytes) = resolve(&url) {
            let mime = mime_for_extension(&url);
            media.insert(url, mime, bytes);
        }
    }
}

/// Whether a reference points outside the local filesystem (a URL or an inline `data:` payload)
/// rather than a file the caller could read.
fn is_remote_reference(url: &str) -> bool {
    url.starts_with("data:") || url.starts_with("//") || url.contains("://")
}

/// The image MIME type a reference's file extension implies, or `None` when the extension names no
/// recognized image type (or the reference has no extension). Covers the raster and vector formats a
/// package embeds, so the container readers and writers share one table instead of each keeping their
/// own; a caller supplies its own policy for the unrecognized case.
#[must_use]
pub fn image_mime_for_extension(reference: &str) -> Option<&'static str> {
    let extension = reference.rsplit_once('.')?.1.to_ascii_lowercase();
    let mime = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        _ => return None,
    };
    Some(mime)
}

/// The image MIME type a reference's extension implies, or `None` when it names no recognized image
/// type. The inverse of [`extension_for_mime`], covering the raster and vector formats a document is
/// likely to embed.
fn mime_for_extension(reference: &str) -> Option<String> {
    let extension = reference.rsplit('.').next()?.to_ascii_lowercase();
    let mime = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        _ => return None,
    };
    Some(mime.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        MediaBag, MediaItem, content_addressed_name, embed_referenced_media, extension_for_mime,
        extracted_path, extraction_target, rewrite_extracted_references,
    };
    use carta_ast::{Block, Inline, Target};

    fn png_item() -> MediaItem {
        MediaItem {
            mime: Some("image/png".into()),
            bytes: vec![1, 2, 3],
        }
    }

    #[test]
    fn extraction_target_preserves_benign_names() {
        let item = png_item();
        assert_eq!(extraction_target("logo.png", &item), "logo.png");
        assert_eq!(extraction_target("media/img.png", &item), "media/img.png");
        assert_eq!(
            extraction_target("./media/./img.png", &item),
            "media/img.png"
        );
    }

    #[test]
    fn extraction_target_rejects_escaping_names() {
        let item = png_item();
        let content_addressed = content_addressed_name("image/png", &[1, 2, 3]);

        let parent = extraction_target("../../etc/evil", &item);
        assert_eq!(parent, content_addressed);
        assert!(!parent.contains(".."));

        assert_eq!(extraction_target("/etc/cron.d/x", &item), content_addressed);
        assert_eq!(extraction_target(".", &item), content_addressed);
    }

    #[test]
    fn extraction_target_falls_back_to_default_mime_when_absent() {
        let item = MediaItem {
            mime: None,
            bytes: vec![4, 5, 6],
        };
        assert_eq!(
            extraction_target("../escape", &item),
            content_addressed_name("application/octet-stream", &[4, 5, 6])
        );
    }

    #[test]
    fn rewrite_points_escaping_reference_at_its_safe_name() {
        let mut bag = MediaBag::new();
        bag.insert("../../evil.png", Some("image/png".into()), vec![1, 2, 3]);
        let mut blocks = vec![Block::Para(vec![image("../../evil.png")])];
        rewrite_extracted_references(&mut blocks, &bag, "out");
        let expected = format!("out/{}", content_addressed_name("image/png", &[1, 2, 3]));
        let first = blocks.first().expect("one block");
        let url = image_url(first);
        assert_eq!(url, expected);
        assert!(!url.contains(".."));
    }

    #[test]
    fn extension_falls_back_to_subtype_without_suffix() {
        assert_eq!(extension_for_mime("image/png"), "png");
        assert_eq!(extension_for_mime("image/jpeg"), "jpg");
        assert_eq!(extension_for_mime("image/svg+xml"), "svg");
        assert_eq!(extension_for_mime("application/pdf"), "pdf");
        assert_eq!(extension_for_mime("image/webp"), "webp");
        assert_eq!(extension_for_mime("application/geo+json"), "geo");
    }

    #[test]
    fn content_addressed_name_is_stable_for_equal_bytes() {
        let name = content_addressed_name("image/png", b"the same bytes");
        assert_eq!(name, content_addressed_name("image/png", b"the same bytes"));
        assert_eq!(name.rsplit('.').next(), Some("png"));
        assert_eq!(name.len(), 40 + 1 + 3);
    }

    #[test]
    fn bag_keeps_entries_in_name_order() {
        let mut bag = MediaBag::new();
        bag.insert("z.png", Some("image/png".to_owned()), vec![1]);
        bag.insert("a.png", None, vec![2]);
        let names: Vec<&str> = bag.names().collect();
        assert_eq!(names, ["a.png", "z.png"]);
        assert_eq!(bag.len(), 2);
        assert!(bag.contains("a.png"));
        assert_eq!(
            bag.get("a.png").map(|item| item.bytes.clone()),
            Some(vec![2])
        );
    }

    #[test]
    fn extracted_path_joins_with_a_single_slash() {
        assert_eq!(extracted_path("media", "a.png"), "media/a.png");
        assert_eq!(extracted_path("assets/img", "a.png"), "assets/img/a.png");
        assert_eq!(extracted_path("media/", "a.png"), "media/a.png");
    }

    #[test]
    fn rewrite_points_bag_references_at_their_extracted_paths() {
        let mut bag = MediaBag::new();
        bag.insert("a.png", Some("image/png".to_owned()), vec![1]);
        let mut blocks = vec![
            Block::Para(vec![image("a.png")]),
            Block::Para(vec![image("https://example.com/b.png")]),
        ];
        rewrite_extracted_references(&mut blocks, &bag, "media");
        let urls: Vec<&str> = blocks.iter().map(image_url).collect();
        assert_eq!(urls, ["media/a.png", "https://example.com/b.png"]);
    }

    fn image(url: &str) -> Inline {
        Inline::Image(
            Box::default(),
            Vec::new(),
            Box::new(Target {
                url: url.into(),
                title: carta_ast::Text::default(),
            }),
        )
    }

    fn image_url(block: &Block) -> &str {
        let Block::Para(inlines) = block else {
            panic!("expected para");
        };
        let Some(Inline::Image(_, _, target)) = inlines.first() else {
            panic!("expected image");
        };
        target.url.as_str()
    }

    #[test]
    fn embed_resolves_each_local_reference_once_and_types_it() {
        let mut bag = MediaBag::new();
        let mut blocks = vec![
            Block::Para(vec![image("a.png"), image("photo.JPG")]),
            Block::Para(vec![image("a.png")]),
            Block::Para(vec![image("https://example.com/b.png")]),
            Block::Para(vec![image("data:image/png;base64,AAAA")]),
            Block::Para(vec![image("gone.gif")]),
        ];
        let mut resolved = Vec::new();
        embed_referenced_media(&mut blocks, &mut bag, |reference| {
            resolved.push(reference.to_owned());
            if reference == "gone.gif" {
                None
            } else {
                Some(reference.as_bytes().to_vec())
            }
        });
        assert_eq!(resolved, ["a.png", "photo.JPG", "gone.gif"]);
        let entries: Vec<(&str, Option<&str>)> = bag
            .iter()
            .map(|(name, item)| (name, item.mime.as_deref()))
            .collect();
        assert_eq!(
            entries,
            [
                ("a.png", Some("image/png")),
                ("photo.JPG", Some("image/jpeg")),
            ]
        );
    }

    #[test]
    fn embed_skips_a_reference_the_bag_already_holds() {
        let mut bag = MediaBag::new();
        bag.insert("a.png", Some("image/png".to_owned()), vec![9]);
        let mut blocks = vec![Block::Para(vec![image("a.png")])];
        let mut consulted = false;
        embed_referenced_media(&mut blocks, &mut bag, |_| {
            consulted = true;
            Some(vec![1])
        });
        assert!(!consulted);
        assert_eq!(
            bag.get("a.png").map(|item| item.bytes.clone()),
            Some(vec![9])
        );
    }

    #[test]
    fn insert_replaces_an_existing_name() {
        let mut bag = MediaBag::new();
        bag.insert("x", None, vec![1]);
        bag.insert("x", Some("image/png".to_owned()), vec![2, 3]);
        assert_eq!(bag.len(), 1);
        let item = bag.get("x").expect("entry present");
        assert_eq!(item.bytes, vec![2, 3]);
        assert_eq!(item.mime.as_deref(), Some("image/png"));
    }
    #[test]
    fn percent_decoding_preserves_plus_and_rejects_bad_escapes() {
        assert_eq!(
            super::percent_decode("a+b%20%C3%A9.png").as_deref(),
            Some("a+b é.png")
        );
        for source in ["bad%", "bad%2", "bad%zz", "%FF"] {
            assert_eq!(super::percent_decode(source), None);
        }
    }
}
