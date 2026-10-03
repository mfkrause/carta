//! Packaging primitives shared by container-based document formats.
//!
//! A container format (an e-book or office-document package) is a ZIP archive of XML parts. This
//! module supplies the two pieces such a writer builds on: a deterministic ZIP archive builder
//! (and matching reader) and a small [`xml`] emitter that always produces well-formed output. Both
//! are byte-reproducible: identical inputs yield identical bytes.

pub mod xml;
#[cfg(feature = "container")]
#[cfg_attr(docsrs, doc(cfg(feature = "container")))]
pub mod zip;
