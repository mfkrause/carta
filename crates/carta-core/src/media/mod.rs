//! The media bag: a document's embedded resources — images and other binary payloads — carried
//! alongside the syntax tree rather than inside it.
//!
//! A container format (a notebook today; a word-processor or e-book package later) references its
//! resources by name while storing their bytes out of band. Keeping those bytes out of the syntax
//! tree mirrors that split: a reader fills a [`MediaBag`] as it decodes, a writer consumes it to
//! re-embed the bytes, and the extract step writes them out as files and rewrites the references to
//! point at them. The tree stays a pure model of structure; the bytes travel next to it.

mod base64;
mod sha1;

pub use base64::{
    decode as base64_decode, encode as base64_encode, encode_mime as base64_encode_mime,
};
pub use sha1::hex as sha1_hex;

use std::collections::BTreeMap;

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
/// Entries are held in sorted order, so iteration — and any file extraction or re-embedding derived
/// from it — is byte-reproducible across runs.
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

#[cfg(test)]
mod tests {
    use super::{MediaBag, content_addressed_name, extension_for_mime};

    #[test]
    fn extension_falls_back_to_subtype_without_suffix() {
        assert_eq!(extension_for_mime("image/png"), "png");
        assert_eq!(extension_for_mime("image/jpeg"), "jpg");
        assert_eq!(extension_for_mime("image/svg+xml"), "svg");
        assert_eq!(extension_for_mime("application/pdf"), "pdf");
        assert_eq!(extension_for_mime("image/webp"), "webp");
        // The structured-syntax suffix is dropped just as it is for svg+xml.
        assert_eq!(extension_for_mime("application/geo+json"), "geo");
    }

    #[test]
    fn content_addressed_name_is_stable_for_equal_bytes() {
        let name = content_addressed_name("image/png", b"the same bytes");
        assert_eq!(name, content_addressed_name("image/png", b"the same bytes"));
        assert!(name.ends_with(".png"));
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
    fn insert_replaces_an_existing_name() {
        let mut bag = MediaBag::new();
        bag.insert("x", None, vec![1]);
        bag.insert("x", Some("image/png".to_owned()), vec![2, 3]);
        assert_eq!(bag.len(), 1);
        let item = bag.get("x").expect("entry present");
        assert_eq!(item.bytes, vec![2, 3]);
        assert_eq!(item.mime.as_deref(), Some("image/png"));
    }
}
