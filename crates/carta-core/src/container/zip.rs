//! A deterministic ZIP archive builder and reader.
//!
//! The builder writes entries with a fixed modification timestamp and no extra fields, so identical
//! inputs always produce byte-identical archives — a hard requirement for reproducible output. An
//! entry is either stored uncompressed ([`ZipArchive::store`]) or DEFLATE-compressed
//! ([`ZipArchive::deflate`], falling back to stored when compression would not shrink it). The
//! reader ([`read`]) parses such archives back into their entries, so a round trip is exact.

use crate::{Error, Result};

/// DEFLATE level used by [`ZipArchive::deflate`]. Fixed so output stays reproducible.
const DEFLATE_LEVEL: u8 = 9;

const METHOD_STORE: u16 = 0;
const METHOD_DEFLATE: u16 = 8;

/// Modification date/time stamped on every entry: 1980-01-01 00:00:00, the ZIP epoch. A constant
/// keeps archives reproducible regardless of the wall clock.
const DOS_TIME: u16 = 0;
const DOS_DATE: u16 = 0x0021;

const VERSION_MADE_BY: u16 = 20;
const VERSION_STORE: u16 = 10;
const VERSION_DEFLATE: u16 = 20;

const SIG_LOCAL: u32 = 0x0403_4b50;
const SIG_CENTRAL: u32 = 0x0201_4b50;
const SIG_EOCD: u32 = 0x0605_4b50;

/// General-purpose bit 11: the file name is UTF-8. Set only when a name has non-ASCII bytes.
const GPBF_UTF8: u16 = 1 << 11;

const LOCAL_HEADER_LEN: usize = 30;
const CENTRAL_HEADER_LEN: usize = 46;
const EOCD_LEN: usize = 22;

/// A builder that accumulates entries and serializes them into a ZIP archive.
#[derive(Debug, Default)]
pub struct ZipArchive {
    body: Vec<u8>,
    entries: Vec<CentralEntry>,
}

#[derive(Debug)]
struct CentralEntry {
    name: String,
    method: u16,
    version_needed: u16,
    crc: u32,
    compressed_size: u32,
    uncompressed_size: u32,
    local_offset: u32,
    utf8: bool,
}

impl ZipArchive {
    /// An empty archive.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `data` under `name`, stored uncompressed. Required for an entry that must not be
    /// compressed, such as an e-book package's signature `mimetype`.
    pub fn store(&mut self, name: &str, data: &[u8]) -> Result<()> {
        self.write_entry(name, data, METHOD_STORE, data, VERSION_STORE)
    }

    /// Adds `data` under `name`, DEFLATE-compressed — unless compression fails to shrink it, in
    /// which case the entry is stored (so an already-compressed image never grows).
    pub fn deflate(&mut self, name: &str, data: &[u8]) -> Result<()> {
        let compressed = miniz_oxide::deflate::compress_to_vec(data, DEFLATE_LEVEL);
        if compressed.len() < data.len() {
            self.write_entry(name, data, METHOD_DEFLATE, &compressed, VERSION_DEFLATE)
        } else {
            self.store(name, data)
        }
    }

    fn write_entry(
        &mut self,
        name: &str,
        uncompressed: &[u8],
        method: u16,
        payload: &[u8],
        version_needed: u16,
    ) -> Result<()> {
        let uncompressed_size =
            u32::try_from(uncompressed.len()).map_err(|_| entry_too_large(name))?;
        let compressed_size = u32::try_from(payload.len()).map_err(|_| entry_too_large(name))?;
        let local_offset = u32::try_from(self.body.len()).map_err(|_| entry_too_large(name))?;
        let name_bytes = name.as_bytes();
        let name_len = u16::try_from(name_bytes.len()).map_err(|_| entry_too_large(name))?;
        let utf8 = !name.is_ascii();
        let flags = if utf8 { GPBF_UTF8 } else { 0 };
        let crc = crc32(uncompressed);

        self.body.extend_from_slice(&SIG_LOCAL.to_le_bytes());
        self.body.extend_from_slice(&version_needed.to_le_bytes());
        self.body.extend_from_slice(&flags.to_le_bytes());
        self.body.extend_from_slice(&method.to_le_bytes());
        self.body.extend_from_slice(&DOS_TIME.to_le_bytes());
        self.body.extend_from_slice(&DOS_DATE.to_le_bytes());
        self.body.extend_from_slice(&crc.to_le_bytes());
        self.body.extend_from_slice(&compressed_size.to_le_bytes());
        self.body
            .extend_from_slice(&uncompressed_size.to_le_bytes());
        self.body.extend_from_slice(&name_len.to_le_bytes());
        self.body.extend_from_slice(&0u16.to_le_bytes());
        self.body.extend_from_slice(name_bytes);
        self.body.extend_from_slice(payload);

        self.entries.push(CentralEntry {
            name: name.to_owned(),
            method,
            version_needed,
            crc,
            compressed_size,
            uncompressed_size,
            local_offset,
            utf8,
        });
        Ok(())
    }

    /// Serializes the archive: the accumulated entry bodies, then the central directory, then the
    /// end-of-central-directory record.
    pub fn finish(self) -> Result<Vec<u8>> {
        let mut out = self.body;
        let central_offset = u32::try_from(out.len()).map_err(|_| archive_too_large())?;

        for entry in &self.entries {
            let name_bytes = entry.name.as_bytes();
            let name_len = u16::try_from(name_bytes.len()).map_err(|_| archive_too_large())?;
            let flags = if entry.utf8 { GPBF_UTF8 } else { 0 };
            out.extend_from_slice(&SIG_CENTRAL.to_le_bytes());
            out.extend_from_slice(&VERSION_MADE_BY.to_le_bytes());
            out.extend_from_slice(&entry.version_needed.to_le_bytes());
            out.extend_from_slice(&flags.to_le_bytes());
            out.extend_from_slice(&entry.method.to_le_bytes());
            out.extend_from_slice(&DOS_TIME.to_le_bytes());
            out.extend_from_slice(&DOS_DATE.to_le_bytes());
            out.extend_from_slice(&entry.crc.to_le_bytes());
            out.extend_from_slice(&entry.compressed_size.to_le_bytes());
            out.extend_from_slice(&entry.uncompressed_size.to_le_bytes());
            out.extend_from_slice(&name_len.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // extra field length
            out.extend_from_slice(&0u16.to_le_bytes()); // comment length
            out.extend_from_slice(&0u16.to_le_bytes()); // disk number start
            out.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
            out.extend_from_slice(&0u32.to_le_bytes()); // external attributes
            out.extend_from_slice(&entry.local_offset.to_le_bytes());
            out.extend_from_slice(name_bytes);
        }

        let central_end = u32::try_from(out.len()).map_err(|_| archive_too_large())?;
        let central_size = central_end - central_offset;
        let count = u16::try_from(self.entries.len()).map_err(|_| archive_too_large())?;

        out.extend_from_slice(&SIG_EOCD.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // this disk number
        out.extend_from_slice(&0u16.to_le_bytes()); // disk with central directory
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&central_size.to_le_bytes());
        out.extend_from_slice(&central_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // comment length
        Ok(out)
    }
}

/// One entry recovered from an archive by [`read`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipEntry {
    /// The entry's name (its path within the archive).
    pub name: String,
    /// The entry's uncompressed bytes.
    pub data: Vec<u8>,
}

/// Parses `archive` into its entries, in central-directory order, decompressing DEFLATE entries.
///
/// # Errors
/// Returns [`Error::Container`] if the archive is truncated, uses an unsupported compression method,
/// carries a non-UTF-8 name, or fails to decompress.
pub fn read(archive: &[u8]) -> Result<Vec<ZipEntry>> {
    let eocd =
        find_eocd(archive).ok_or_else(|| corrupt("missing end-of-central-directory record"))?;
    let count = u16_at(archive, eocd + 10)?;
    let mut pos =
        usize::try_from(u32_at(archive, eocd + 16)?).map_err(|_| corrupt("offset too large"))?;

    let mut entries = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        if u32_at(archive, pos)? != SIG_CENTRAL {
            return Err(corrupt("bad central-directory signature"));
        }
        let method = u16_at(archive, pos + 10)?;
        let compressed_size =
            usize::try_from(u32_at(archive, pos + 20)?).map_err(|_| corrupt("size too large"))?;
        let uncompressed_size =
            usize::try_from(u32_at(archive, pos + 24)?).map_err(|_| corrupt("size too large"))?;
        let name_len = usize::from(u16_at(archive, pos + 28)?);
        let extra_len = usize::from(u16_at(archive, pos + 30)?);
        let comment_len = usize::from(u16_at(archive, pos + 32)?);
        let local_offset =
            usize::try_from(u32_at(archive, pos + 42)?).map_err(|_| corrupt("offset too large"))?;

        let name_start = add(pos, CENTRAL_HEADER_LEN)?;
        let name_bytes = archive
            .get(name_start..add(name_start, name_len)?)
            .ok_or_else(|| corrupt("truncated central-directory name"))?;
        let name =
            String::from_utf8(name_bytes.to_vec()).map_err(|_| corrupt("non-UTF-8 entry name"))?;

        let local_name_len = usize::from(u16_at(archive, add(local_offset, 26)?)?);
        let local_extra_len = usize::from(u16_at(archive, add(local_offset, 28)?)?);
        let data_start = add(
            add(add(local_offset, LOCAL_HEADER_LEN)?, local_name_len)?,
            local_extra_len,
        )?;
        let raw = archive
            .get(data_start..add(data_start, compressed_size)?)
            .ok_or_else(|| corrupt("truncated entry data"))?;

        let data = match method {
            METHOD_STORE => raw.to_vec(),
            // Decompress no further than the central directory's declared uncompressed size, so a
            // crafted entry whose tiny payload would otherwise inflate without bound cannot exhaust
            // memory; a result that disagrees with the declared size marks a corrupt archive.
            METHOD_DEFLATE => {
                let out =
                    miniz_oxide::inflate::decompress_to_vec_with_limit(raw, uncompressed_size)
                        .map_err(|_| corrupt("DEFLATE decode failed"))?;
                if out.len() != uncompressed_size {
                    return Err(corrupt("DEFLATE size disagrees with declared size"));
                }
                out
            }
            other => return Err(corrupt(&format!("unsupported compression method {other}"))),
        };
        entries.push(ZipEntry { name, data });
        pos = add(
            add(add(add(pos, CENTRAL_HEADER_LEN)?, name_len)?, extra_len)?,
            comment_len,
        )?;
    }
    Ok(entries)
}

/// Add two archive offsets, mapping overflow to a corrupt-archive error. An offset and a length
/// taken from a hostile archive can sum past the address space; without this the addition would wrap
/// in release (yielding an in-range slice of the wrong bytes) or panic in debug.
fn add(a: usize, b: usize) -> Result<usize> {
    a.checked_add(b)
        .ok_or_else(|| corrupt("archive offset out of range"))
}

fn find_eocd(buf: &[u8]) -> Option<usize> {
    if buf.len() < EOCD_LEN {
        return None;
    }
    let signature = SIG_EOCD.to_le_bytes();
    let earliest = buf.len().saturating_sub(EOCD_LEN + usize::from(u16::MAX));
    let mut at = buf.len() - EOCD_LEN;
    loop {
        if buf.get(at..at + 4) == Some(&signature[..]) {
            return Some(at);
        }
        if at == earliest {
            return None;
        }
        at -= 1;
    }
}

fn u16_at(buf: &[u8], at: usize) -> Result<u16> {
    let bytes = buf
        .get(at..)
        .and_then(|rest| rest.get(..2))
        .ok_or_else(|| corrupt("truncated field"))?;
    let array = <[u8; 2]>::try_from(bytes).map_err(|_| corrupt("truncated field"))?;
    Ok(u16::from_le_bytes(array))
}

fn u32_at(buf: &[u8], at: usize) -> Result<u32> {
    let bytes = buf
        .get(at..)
        .and_then(|rest| rest.get(..4))
        .ok_or_else(|| corrupt("truncated field"))?;
    let array = <[u8; 4]>::try_from(bytes).map_err(|_| corrupt("truncated field"))?;
    Ok(u32::from_le_bytes(array))
}

/// The CRC-32 (IEEE 802.3) of `data`, computed bitwise so no lookup table is needed.
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= u32::from(byte);
        let mut bit = 0;
        while bit < 8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            bit += 1;
        }
    }
    !crc
}

fn entry_too_large(name: &str) -> Error {
    Error::Container(format!(
        "zip entry '{name}' exceeds 4 GiB (ZIP64 is unsupported)"
    ))
}

fn archive_too_large() -> Error {
    Error::Container("zip archive exceeds 4 GiB (ZIP64 is unsupported)".to_owned())
}

fn corrupt(detail: &str) -> Error {
    Error::Container(format!("corrupt zip archive: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::{ZipArchive, ZipEntry, crc32, read};

    #[test]
    fn crc32_matches_known_vector() {
        // The canonical CRC-32 check value for the ASCII string "123456789".
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn stored_and_deflated_entries_round_trip() {
        let mut archive = ZipArchive::new();
        let text = b"hello, world; ".repeat(64);
        archive
            .store("mimetype", b"application/epub+zip")
            .expect("store");
        archive.deflate("EPUB/content.opf", &text).expect("deflate");
        archive.deflate("small", b"x").expect("deflate small");
        let bytes = archive.finish().expect("finish");

        let entries = read(&bytes).expect("read");
        assert_eq!(
            entries,
            vec![
                ZipEntry {
                    name: "mimetype".to_owned(),
                    data: b"application/epub+zip".to_vec()
                },
                ZipEntry {
                    name: "EPUB/content.opf".to_owned(),
                    data: text
                },
                ZipEntry {
                    name: "small".to_owned(),
                    data: b"x".to_vec()
                },
            ]
        );
    }

    #[test]
    fn incompressible_data_falls_back_to_stored() {
        // A single byte cannot be shrunk by DEFLATE, so the entry must be stored and still read back.
        let mut archive = ZipArchive::new();
        archive.deflate("one", b"Z").expect("deflate");
        let bytes = archive.finish().expect("finish");
        let entries = read(&bytes).expect("read");
        assert_eq!(entries.first().map(|e| e.data.clone()), Some(b"Z".to_vec()));
    }

    #[test]
    fn output_is_reproducible() {
        let build = || {
            let mut archive = ZipArchive::new();
            archive
                .store("mimetype", b"application/epub+zip")
                .expect("store");
            archive
                .deflate("a.txt", b"repeatable content ".repeat(16).as_slice())
                .expect("deflate");
            archive.finish().expect("finish")
        };
        assert_eq!(build(), build());
    }

    #[test]
    fn read_rejects_truncated_archive() {
        assert!(read(b"not a zip").is_err());
        assert!(read(b"").is_err());
    }

    #[test]
    fn read_rejects_deflate_exceeding_declared_size() {
        // A deflated entry whose central-directory record understates the uncompressed size is
        // rejected rather than inflated past that bound — the guard against a decompression bomb
        // that would otherwise expand a tiny payload into an unbounded allocation.
        let mut archive = ZipArchive::new();
        archive
            .deflate("big", b"AAAAAAAAAAAAAAAA".repeat(64).as_slice())
            .expect("deflate");
        let mut bytes = archive.finish().expect("finish");
        let central = bytes
            .windows(4)
            .position(|window| window == [0x50, 0x4b, 0x01, 0x02])
            .expect("central-directory header");
        bytes
            .get_mut(central + 24..central + 28)
            .expect("uncompressed-size field")
            .copy_from_slice(&4u32.to_le_bytes());
        assert!(read(&bytes).is_err());
    }
}
