#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
//! The document model and optional JSON interchange codec.
//!
//! [`Document`] contains [`Block`] and [`Inline`] nodes. The `serde` feature enables JSON codecs.

mod ast;
#[cfg(feature = "serde")]
mod codec;
mod drop;
#[cfg(feature = "serde")]
mod serde_impls;

pub use ast::*;
#[cfg(feature = "serde")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
pub use codec::{from_json, to_json, to_json_writer};

/// Formats a value (a `char`, an integer, …) directly into a [`Text`], keeping short results in
/// inline storage. Re-exported here so downstream crates build `Text` values without naming the
/// backing crate.
pub use compact_str::ToCompactString;

/// The JSON object key carrying the AST schema version.
///
/// This is an opaque external protocol identifier, deliberately confined to this one constant; do
/// not duplicate the literal elsewhere.
pub const API_VERSION_KEY: &str = "pandoc-api-version";

/// The AST schema version stamped onto freshly constructed documents. Parsed documents echo back
/// the version they were read with instead (see [`ApiVersion`]).
pub const CURRENT_API_VERSION: &[u32] = &[1, 23, 1, 2];
