//! The EPUB container writer: it lays a document out as a reflowable e-book — a ZIP archive of
//! XHTML chapter files, a package document, navigation, a stylesheet and any embedded resources.
//!
//! Both EPUB dialects are produced from the same pipeline. The document body is split into chapter
//! files at a chosen heading level; each chapter is rendered to XHTML and wrapped in a page; the
//! metadata becomes the package's Dublin Core record; and two tables of contents (the XHTML
//! navigation document and the NCX) are derived from the chapter headings. [`Version::Epub3`] and
//! [`Version::Epub2`] differ only in the package version, the navigation primacy, and the XHTML
//! dialect of each page.
//!
//! Output is byte-reproducible: the archive uses fixed timestamps, maps are ordered, and a missing
//! publication identifier is derived from the content rather than generated at random.

mod metadata;
mod navigation;
mod package;
mod pages;
mod sections;
mod styles;

use carta_ast::{Block, Document};
use carta_core::container::zip::ZipArchive;
use carta_core::media::{MediaItem, extension_for_mime};
use carta_core::{BytesWriter, EpubOptions, Result, WriterOptions};
use metadata::BookMeta;
use navigation::{Landmarks, collect_toc, nav_xhtml, toc_ncx};
use package::{Dates, ManifestItem, SpineItem, content_opf};
use pages::{BodyKind, container_xml, cover_page, ibooks_display_options, title_page, xhtml_page};
use std::collections::BTreeMap;
use styles::{DEFAULT_STYLESHEET, DEFAULT_STYLESHEET_NAME};

/// The EPUB dialect a package targets.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Version {
    /// EPUB 2: the NCX is the primary table of contents and pages are XHTML 1.1.
    Epub2,
    /// EPUB 3: the XHTML navigation document is primary and pages carry `epub:type` semantics.
    Epub3,
}

impl Version {
    /// Whether this is the EPUB 3 dialect.
    pub(crate) fn is_epub3(self) -> bool {
        matches!(self, Version::Epub3)
    }
}

/// One chapter file: its name within the container, the manifest id it is referenced by, and the
/// blocks it holds after sectioning.
pub(crate) struct Chapter {
    pub file: String,
    pub item_id: String,
    pub blocks: Vec<Block>,
}

/// A resource stored in the container's `media/` or `fonts/` directory.
struct Asset {
    /// Path relative to the container root, e.g. `media/file0.png`.
    href: String,
    /// Manifest item id, e.g. `file0_png`.
    item_id: String,
    media_type: String,
    /// The EPUB manifest property this asset carries, e.g. `cover-image`.
    properties: Option<String>,
    bytes: Vec<u8>,
}

impl Asset {
    fn manifest_item(&self) -> ManifestItem {
        ManifestItem {
            id: self.item_id.clone(),
            href: self.href.clone(),
            media_type: self.media_type.clone(),
            properties: self.properties.clone(),
        }
    }
}

/// The cover image's place among the media assets and its pixel dimensions, used to size the
/// generated cover page.
struct Cover {
    media_index: usize,
    width: u32,
    height: u32,
}

/// The EPUB 3 container writer.
#[derive(Debug)]
pub struct Epub3Writer;

/// The EPUB 2 container writer.
#[derive(Debug)]
pub struct Epub2Writer;

impl BytesWriter for Epub3Writer {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<Vec<u8>> {
        write_epub(Version::Epub3, document, options)
    }
}

impl BytesWriter for Epub2Writer {
    fn write(&self, document: &Document, options: &WriterOptions) -> Result<Vec<u8>> {
        write_epub(Version::Epub2, document, options)
    }
}

/// Assemble the complete EPUB archive for `version`.
fn write_epub(version: Version, document: &Document, options: &WriterOptions) -> Result<Vec<u8>> {
    let epub = &options.epub;
    let epub3 = version.is_epub3();
    let dir = epub
        .subdirectory
        .as_deref()
        .unwrap_or("EPUB")
        .trim_end_matches('/');
    let split_level = epub
        .split_level
        .map_or(1, |level| i32::try_from(level).unwrap_or(i32::MAX));
    let toc_depth = options.toc_depth.unwrap_or(3);

    // Turn the flat body into chapter files, keeping a map from each section to the file it lands in.
    let (mut chapters, section_files, sectioned) = build_chapters(document, options, split_level);

    // Collect the referenced images, the cover, and the embedded fonts into the container's resources.
    let (media, cover, fonts) = gather_media(epub, &mut chapters, options);

    // Render each chapter body, then seed the publication identifier from that content so a book
    // without an explicit identifier still gets a stable one.
    let bodies: Vec<String> = chapters
        .iter()
        .map(|chapter| crate::html::render_epub_chapter(&chapter.blocks, epub3, options))
        .collect();
    let (meta, doc_meta) = resolve_metadata(document, epub, &bodies.join("\n"));

    let (stylesheet_names, stylesheets) = stylesheets(epub);

    let chapter_pages: Vec<String> = chapters
        .iter()
        .zip(&bodies)
        .map(|(chapter, body)| {
            xhtml_page(
                version,
                &meta.language,
                &chapter.file,
                "../",
                BodyKind::Bodymatter,
                &stylesheet_names,
                body,
            )
        })
        .collect();

    let title_page_doc = title_page(version, &doc_meta, meta.display_title(), &stylesheet_names);
    let cover_page_doc = cover.as_ref().map(|cover| {
        let href = media
            .get(cover.media_index)
            .map_or_else(String::new, |asset| format!("../{}", asset.href));
        cover_page(
            version,
            &meta.language,
            meta.display_title(),
            &href,
            cover.width,
            cover.height,
            &stylesheet_names,
        )
    });

    let toc_entries = collect_toc(&sectioned, &section_files, toc_depth);
    let landmarks = Landmarks {
        cover: cover.is_some(),
    };
    let nav_doc = nav_xhtml(
        version,
        &meta,
        &doc_meta,
        &toc_entries,
        &landmarks,
        &stylesheet_names,
        options.source_name.as_deref(),
    );
    // The navigation control file records the cover under an identifier derived from its source name.
    let cover_ncx_id = epub.cover_image.as_ref().map(|(name, _)| item_id_for(name));
    let ncx_doc = toc_ncx(&meta, &doc_meta, &toc_entries, cover_ncx_id.as_deref());

    let modified = iso_from_epoch(epub.source_date_epoch.unwrap_or(1));
    let dates = Dates {
        publication: meta.date.clone().unwrap_or_else(|| modified.clone()),
        modified: if epub3 { Some(modified.clone()) } else { None },
    };

    let cover_id = cover
        .as_ref()
        .and_then(|cover| media.get(cover.media_index))
        .map(|asset| asset.item_id.clone());

    let manifest = build_manifest(
        &chapters,
        &media,
        &fonts,
        &stylesheets,
        cover.is_some(),
        epub3,
    );
    let spine = build_spine(&chapters, &doc_meta, cover.is_some());
    let opf = content_opf(
        version,
        &meta,
        &dates,
        cover_id.as_deref(),
        &manifest,
        &spine,
    );

    let container = container_xml(dir);
    let ibooks = ibooks_display_options();

    // Pack in the fixed order a reading system expects: the uncompressed signature first, the
    // container bookkeeping next, then the package, navigation, pages, stylesheets and resources.
    let mut zip = ZipArchive::new();
    zip.store("mimetype", b"application/epub+zip")?;
    zip.deflate("META-INF/container.xml", container.as_bytes())?;
    zip.deflate(
        "META-INF/com.apple.ibooks.display-options.xml",
        ibooks.as_bytes(),
    )?;
    zip.deflate(&join(dir, "content.opf"), opf.as_bytes())?;
    zip.deflate(&join(dir, "toc.ncx"), ncx_doc.as_bytes())?;
    zip.deflate(&join(dir, "nav.xhtml"), nav_doc.as_bytes())?;
    zip.deflate(
        &join(dir, "text/title_page.xhtml"),
        title_page_doc.as_bytes(),
    )?;
    for (name, contents) in &stylesheets {
        zip.deflate(&join(dir, &format!("styles/{name}")), contents.as_bytes())?;
    }
    for asset in &media {
        zip.deflate(&join(dir, &asset.href), &asset.bytes)?;
    }
    if let Some(page) = &cover_page_doc {
        zip.deflate(&join(dir, "text/cover.xhtml"), page.as_bytes())?;
    }
    for (chapter, page) in chapters.iter().zip(&chapter_pages) {
        zip.deflate(
            &join(dir, &format!("text/{}", chapter.file)),
            page.as_bytes(),
        )?;
    }
    for font in &fonts {
        zip.deflate(&join(dir, &font.href), &font.bytes)?;
    }
    zip.finish()
}

/// Split the document body into chapter files and record, for every section, the file it lands in so
/// the table of contents can keep the original nesting even where a split lifted a subsection into a
/// file of its own. Returns the chapters, that section-to-file map, and the sectioned block tree.
fn build_chapters(
    document: &Document,
    options: &WriterOptions,
    split_level: i32,
) -> (Vec<Chapter>, BTreeMap<String, String>, Vec<Block>) {
    let title_inlines = document
        .meta
        .get("title")
        .map(metadata::meta_inlines)
        .unwrap_or_default();
    let mut body = document.blocks.clone();
    if options.number_sections {
        carta_core::sections::number_sections(&mut body);
    }
    let sectioned = sections::make_sections(&body, &title_inlines);
    let chapters: Vec<Chapter> = sections::split_chapters(sectioned.clone(), split_level)
        .into_iter()
        .enumerate()
        .map(|(index, blocks)| {
            let file = format!("ch{:03}.xhtml", index + 1);
            Chapter {
                item_id: item_id_for(&file),
                file,
                blocks,
            }
        })
        .collect();

    let mut section_files: BTreeMap<String, String> = BTreeMap::new();
    for chapter in &chapters {
        record_section_files(&chapter.blocks, &chapter.file, &mut section_files);
    }
    (chapters, section_files, sectioned)
}

/// Gather the container's binary resources: the images the chapters reference (rewriting each
/// reference to its stored path), the cover image, and the embedded fonts.
fn gather_media(
    epub: &EpubOptions,
    chapters: &mut [Chapter],
    options: &WriterOptions,
) -> (Vec<Asset>, Option<Cover>, Vec<Asset>) {
    let mut media: Vec<Asset> = Vec::new();
    collect_images(chapters, options, &mut media);

    // The cover image takes the next media slot; its page is built once the metadata is known.
    let cover = epub.cover_image.as_ref().map(|(name, bytes)| {
        let extension = file_extension(name);
        let index = media.len();
        let file = format!("file{index}.{extension}");
        let (width, height) = image_dimensions(bytes);
        media.push(Asset {
            item_id: item_id_for(&file),
            href: format!("media/{file}"),
            media_type: image_media_type(&extension).to_owned(),
            properties: Some(String::from("cover-image")),
            bytes: bytes.clone(),
        });
        Cover {
            media_index: index,
            width,
            height,
        }
    });

    let fonts: Vec<Asset> = epub
        .fonts
        .iter()
        .map(|(name, bytes)| {
            let base = basename(name);
            Asset {
                item_id: item_id_for(base),
                href: format!("fonts/{base}"),
                media_type: font_media_type(base).to_owned(),
                properties: None,
                bytes: bytes.clone(),
            }
        })
        .collect();

    (media, cover, fonts)
}

/// Resolve the two metadata views. The document's own metadata builds the title page and the
/// per-file titles; a supplied Dublin Core fragment overrides only the publication (package)
/// metadata. The two views diverge when the fragment names, say, a title the document itself does
/// not — the package records the fragment's, while the title page stays as the document authored it.
/// The resolved language, however, is one value applied to every document, so the package view's
/// language flows back to the document view. Returns `(package, document)`.
fn resolve_metadata(document: &Document, epub: &EpubOptions, seed: &str) -> (BookMeta, BookMeta) {
    let doc_meta = BookMeta::from_meta(&document.meta, seed);
    let mut meta = doc_meta.clone();
    if let Some(fragment) = &epub.metadata_xml {
        meta.apply_metadata_xml(fragment);
    }
    let doc_meta = BookMeta {
        language: meta.language.clone(),
        ..doc_meta
    };
    (meta, doc_meta)
}

/// Collect every image the chapters reference into `media`, assigning each a `media/fileN.ext` name
/// in order of first appearance, and rewrite each reference to point at the stored file.
fn collect_images(chapters: &mut [Chapter], options: &WriterOptions, media: &mut Vec<Asset>) {
    let mut assigned: BTreeMap<String, usize> = BTreeMap::new();
    for chapter in chapters.iter_mut() {
        carta_core::walk::for_each_image_target(&mut chapter.blocks, &mut |target| {
            let url = target.url.to_string();
            if assigned.contains_key(&url) {
                return;
            }
            let Some(item) = options.media.get(&url) else {
                return;
            };
            let index = media.len();
            let extension = image_extension(&url, item);
            let file = format!("file{index}.{extension}");
            media.push(Asset {
                item_id: item_id_for(&file),
                href: format!("media/{file}"),
                media_type: item
                    .mime
                    .clone()
                    .unwrap_or_else(|| image_media_type(&extension).to_owned()),
                properties: None,
                bytes: item.bytes.clone(),
            });
            assigned.insert(url, index);
        });
    }
    for chapter in chapters.iter_mut() {
        carta_core::walk::for_each_image_target(&mut chapter.blocks, &mut |target| {
            if let Some(asset) = assigned
                .get(target.url.as_str())
                .and_then(|&i| media.get(i))
            {
                target.url = format!("../{}", asset.href).into();
            } else if is_relative_resource(target.url.as_str()) {
                // Chapters live one level down in `text/`; a relative resource that is not embedded
                // still needs to climb back to the container root to resolve.
                target.url = format!("../{}", target.url).into();
            }
        });
    }
}

/// Whether a reference is a working-directory-relative local path — one that a chapter, nested a
/// level down, must reach with a `../` prefix. Absolute paths, scheme URLs, protocol-relative URLs
/// and inline `data:` payloads resolve on their own and are left untouched.
fn is_relative_resource(url: &str) -> bool {
    !(url.is_empty()
        || url.starts_with('/')
        || url.starts_with('#')
        || url.starts_with("data:")
        || url.starts_with("//")
        || url.contains("://"))
}

/// The stylesheets linked from every page: the file names in link order, and each `(name, contents)`
/// to store. A user stylesheet replaces the built-in one; several are numbered in order.
fn stylesheets(epub: &carta_core::EpubOptions) -> (Vec<String>, Vec<(String, String)>) {
    if epub.stylesheets.is_empty() {
        return (
            vec![DEFAULT_STYLESHEET_NAME.to_owned()],
            vec![(
                DEFAULT_STYLESHEET_NAME.to_owned(),
                DEFAULT_STYLESHEET.to_owned(),
            )],
        );
    }
    let mut names = Vec::new();
    let mut files = Vec::new();
    for (index, contents) in epub.stylesheets.iter().enumerate() {
        let name = format!("stylesheet{}.css", index + 1);
        names.push(name.clone());
        files.push((name, contents.clone()));
    }
    (names, files)
}

/// The manifest, in the order a package lists its files: the two tables of contents, the
/// stylesheets, the cover page, the title page, the chapters, then the media (cover image first) and
/// fonts.
fn build_manifest(
    chapters: &[Chapter],
    media: &[Asset],
    fonts: &[Asset],
    stylesheets: &[(String, String)],
    has_cover: bool,
    epub3: bool,
) -> Vec<ManifestItem> {
    let mut items = Vec::new();
    items.push(ManifestItem {
        id: String::from("ncx"),
        href: String::from("toc.ncx"),
        media_type: String::from("application/x-dtbncx+xml"),
        properties: None,
    });
    items.push(ManifestItem {
        id: String::from("nav"),
        href: String::from("nav.xhtml"),
        media_type: String::from("application/xhtml+xml"),
        properties: epub3.then(|| String::from("nav")),
    });
    for (index, (name, _)) in stylesheets.iter().enumerate() {
        items.push(ManifestItem {
            id: format!("stylesheet{}", index + 1),
            href: format!("styles/{name}"),
            media_type: String::from("text/css"),
            properties: None,
        });
    }
    if has_cover {
        items.push(ManifestItem {
            id: String::from("cover_xhtml"),
            href: String::from("text/cover.xhtml"),
            media_type: String::from("application/xhtml+xml"),
            properties: epub3.then(|| String::from("svg")),
        });
    }
    items.push(ManifestItem {
        id: String::from("title_page_xhtml"),
        href: String::from("text/title_page.xhtml"),
        media_type: String::from("application/xhtml+xml"),
        properties: None,
    });
    for chapter in chapters {
        items.push(ManifestItem {
            id: chapter.item_id.clone(),
            href: format!("text/{}", chapter.file),
            media_type: String::from("application/xhtml+xml"),
            properties: None,
        });
    }
    // The cover image is listed ahead of the content images; both precede the fonts. The manifest
    // `properties` attribute belongs to the EPUB 3 vocabulary, so EPUB 2 omits it here.
    for asset in media.iter().filter(|asset| asset.properties.is_some()) {
        let mut item = asset.manifest_item();
        if !epub3 {
            item.properties = None;
        }
        items.push(item);
    }
    for asset in media.iter().filter(|asset| asset.properties.is_none()) {
        items.push(asset.manifest_item());
    }
    for font in fonts {
        items.push(font.manifest_item());
    }
    items
}

/// The spine, in reading order: the cover page (when present), the title page, then the chapters.
fn build_spine(chapters: &[Chapter], meta: &BookMeta, has_cover: bool) -> Vec<SpineItem> {
    let mut spine = Vec::new();
    if has_cover {
        spine.push(SpineItem {
            idref: String::from("cover_xhtml"),
            linear: None,
        });
    }
    spine.push(SpineItem {
        idref: String::from("title_page_xhtml"),
        linear: Some(if title_page_has_content(meta) {
            "yes"
        } else {
            "no"
        }),
    });
    for chapter in chapters {
        spine.push(SpineItem {
            idref: chapter.item_id.clone(),
            linear: None,
        });
    }
    spine
}

/// Whether the generated title page carries any content, which decides if it is part of the linear
/// reading order.
fn title_page_has_content(meta: &BookMeta) -> bool {
    !meta.title_inlines.is_empty()
        || meta.subtitle_inlines.is_some()
        || !meta.creators.is_empty()
        || meta.publisher.is_some()
        || meta.date.is_some()
        || meta.rights_inlines.is_some()
}

/// Join a container-relative path onto the container directory, keeping the archive root when the
/// directory is empty.
fn join(dir: &str, rel: &str) -> String {
    if dir.is_empty() {
        rel.to_owned()
    } else {
        format!("{dir}/{rel}")
    }
}

/// The manifest id for a file, formed from its base name with each dot replaced by an underscore.
fn item_id_for(basename: &str) -> String {
    basename.replace('.', "_")
}

/// Record which file each section landed in, descending through nested sections. Every section found
/// within `blocks` is mapped to `file`.
fn record_section_files(blocks: &[Block], file: &str, map: &mut BTreeMap<String, String>) {
    for block in blocks {
        if let Block::Div(attr, children) = block {
            if attr.classes.iter().any(|class| class == "section") {
                map.insert(attr.id.to_string(), file.to_owned());
            }
            record_section_files(children, file, map);
        }
    }
}

/// The final path component of `path`, treating both slash styles as separators.
fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// The lowercase file extension of `name`, or an empty string when it has none.
fn file_extension(name: &str) -> String {
    basename(name)
        .rsplit_once('.')
        .map_or_else(String::new, |(_, extension)| extension.to_ascii_lowercase())
}

/// The extension to store an image under: the reference's own extension when it is a plain word,
/// otherwise the one its MIME type implies.
fn image_extension(url: &str, item: &MediaItem) -> String {
    let from_url = file_extension(url);
    if !from_url.is_empty() && from_url.chars().all(|c| c.is_ascii_alphanumeric()) {
        return from_url;
    }
    item.mime
        .as_deref()
        .map_or("bin", extension_for_mime)
        .to_owned()
}

/// The MIME type for a stored image, by its extension.
fn image_media_type(extension: &str) -> &'static str {
    match extension {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

/// The MIME type for an embedded font, by its file's extension.
fn font_media_type(name: &str) -> &'static str {
    match file_extension(name).as_str() {
        "otf" => "font/otf",
        "ttf" => "font/ttf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// Format a Unix timestamp as an ISO 8601 instant in UTC, e.g. `2006-01-02T15:04:05Z`.
fn iso_from_epoch(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let time = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (time / 3600, (time % 3600) / 60, time % 60);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// The civil date (year, month, day) for a count of days since the Unix epoch, by the standard
/// days-from-civil inverse.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = u32::try_from(day_of_year - (153 * month_prime + 2) / 5 + 1).unwrap_or(0);
    let month_number = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let month = u32::try_from(month_number).unwrap_or(0);
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

/// The pixel dimensions of an image, read from its header. Returns `(0, 0)` for a format that is not
/// recognized or a header that is too short to parse.
fn image_dimensions(bytes: &[u8]) -> (u32, u32) {
    png_dimensions(bytes)
        .or_else(|| gif_dimensions(bytes))
        .or_else(|| jpeg_dimensions(bytes))
        .unwrap_or((0, 0))
}

/// The dimensions in a PNG's `IHDR` chunk, or `None` when the signature does not match.
fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.get(..8) != Some(SIGNATURE) {
        return None;
    }
    Some((read_be_u32(bytes, 16)?, read_be_u32(bytes, 20)?))
}

/// The dimensions in a GIF's logical screen descriptor, or `None` when the signature does not match.
fn gif_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let header = bytes.get(..6)?;
    if header != b"GIF87a".as_slice() && header != b"GIF89a".as_slice() {
        return None;
    }
    Some((
        u32::from(read_le_u16(bytes, 6)?),
        u32::from(read_le_u16(bytes, 8)?),
    ))
}

/// The dimensions in a JPEG's first frame header, or `None` when the marker structure does not match.
fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..2) != Some([0xff, 0xd8].as_slice()) {
        return None;
    }
    let mut offset = 2usize;
    loop {
        let mut marker = *bytes.get(offset)?;
        // A marker is introduced by one or more 0xff fill bytes.
        while marker == 0xff {
            offset = offset.checked_add(1)?;
            marker = *bytes.get(offset)?;
        }
        offset = offset.checked_add(1)?;
        // Restart, start-of-image, end-of-image and temporary markers carry no length payload.
        if (0xd0..=0xd9).contains(&marker) || marker == 0x01 {
            continue;
        }
        let length = usize::from(read_be_u16(bytes, offset)?);
        // A start-of-frame marker holds the frame dimensions; the four Huffman/arithmetic table
        // markers in the same range do not.
        let is_frame = (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc);
        if is_frame {
            let height = read_be_u16(bytes, offset + 3)?;
            let width = read_be_u16(bytes, offset + 5)?;
            return Some((u32::from(width), u32::from(height)));
        }
        offset = offset.checked_add(length)?;
    }
}

/// A big-endian `u32` at `offset`, or `None` when the slice is too short.
fn read_be_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let array: [u8; 4] = bytes.get(offset..offset + 4)?.try_into().ok()?;
    Some(u32::from_be_bytes(array))
}

/// A big-endian `u16` at `offset`, or `None` when the slice is too short.
fn read_be_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let array: [u8; 2] = bytes.get(offset..offset + 2)?.try_into().ok()?;
    Some(u16::from_be_bytes(array))
}

/// A little-endian `u16` at `offset`, or `None` when the slice is too short.
fn read_le_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let array: [u8; 2] = bytes.get(offset..offset + 2)?.try_into().ok()?;
    Some(u16::from_le_bytes(array))
}
