//! Host access for the files and URLs a document names.

#![expect(
    clippy::disallowed_methods,
    reason = "the one module that reaches the host for a document"
)]

use std::io;
use std::path::{Path, PathBuf};

use crate::MediaItem;

/// Retrieves the resource a URL names.
pub type Fetcher = fn(&str) -> io::Result<MediaItem>;

/// What a conversion may reach on the host for the files and URLs a document names.
///
/// A document can name companion files (an included source, an image to embed) and remote
/// resources. Every such lookup goes through this type, so one value decides what untrusted input
/// can reach. Files the caller names itself (inputs, templates, option files) are not subject to it.
#[derive(Debug, Clone, Copy, Default)]
pub enum HostAccess {
    /// Files are read from disk; no URL is fetched.
    #[default]
    Local,
    /// Files are read from disk and URLs are fetched with the given function.
    Networked(Fetcher),
    /// Nothing is reachable: every file and URL lookup fails as not found.
    Sandboxed,
}

impl HostAccess {
    /// How many files deep the includes of a document may nest before a reader fails the read.
    pub const NESTING_LIMIT: usize = 128;

    /// Reads the file at `path`.
    ///
    /// # Errors
    /// The file system's error, or [`io::ErrorKind::NotFound`] in the sandbox.
    pub fn read(self, path: &Path) -> io::Result<Vec<u8>> {
        self.files()?;
        std::fs::read(path)
    }

    /// Reads the file at `path` as UTF-8 text.
    ///
    /// # Errors
    /// The file system's error, or [`io::ErrorKind::NotFound`] in the sandbox.
    pub fn read_to_string(self, path: &Path) -> io::Result<String> {
        self.files()?;
        std::fs::read_to_string(path)
    }

    /// Resolves `path` to its canonical absolute form, which exists only for a file that does.
    ///
    /// # Errors
    /// The file system's error, or [`io::ErrorKind::NotFound`] in the sandbox.
    pub fn canonicalize(self, path: &Path) -> io::Result<PathBuf> {
        self.files()?;
        std::fs::canonicalize(path)
    }

    /// Fetches the resource `url` names.
    ///
    /// # Errors
    /// The fetcher's error, [`io::ErrorKind::Unsupported`] without network access, or
    /// [`io::ErrorKind::NotFound`] in the sandbox.
    pub fn fetch(self, url: &str) -> io::Result<MediaItem> {
        match self {
            Self::Networked(fetch) => fetch(url),
            Self::Local => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "network access is not available",
            )),
            Self::Sandboxed => Err(sandboxed()),
        }
    }

    fn files(self) -> io::Result<()> {
        match self {
            Self::Local | Self::Networked(_) => Ok(()),
            Self::Sandboxed => Err(sandboxed()),
        }
    }
}

fn sandboxed() -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, "not available in the sandbox")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sandbox_reaches_no_file_that_exists() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(HostAccess::Local.read(&path).is_ok());
        for result in [
            HostAccess::Sandboxed.read(&path).map(drop),
            HostAccess::Sandboxed.read_to_string(&path).map(drop),
            HostAccess::Sandboxed.canonicalize(&path).map(drop),
        ] {
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
        }
    }

    #[test]
    fn only_a_networked_host_fetches() {
        let fetcher: Fetcher = |url| {
            Ok(MediaItem {
                bytes: url.as_bytes().to_vec(),
                mime: None,
            })
        };
        assert_eq!(
            HostAccess::Networked(fetcher).fetch("u").unwrap().bytes,
            b"u"
        );
        assert_eq!(
            HostAccess::Local.fetch("u").unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        assert_eq!(
            HostAccess::Sandboxed.fetch("u").unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }
}
