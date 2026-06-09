//! The carta abstract syntax tree: the document model and its JSON interchange format.
//!
//! This AST is the single contract between readers and writers (see `docs/PORTING.md` §3–4).
//! [`Document`] is the root; [`Block`] and [`Inline`] are the two load-bearing node families.
//! JSON (de)serialization is provided by serde derives plus the manual array codecs in
//! `serde_impls`, and the convenience entry points [`from_json`] and [`to_json_writer`].

mod ast;
mod codec;
mod serde_impls;

pub use ast::*;
pub use codec::{from_json, to_json, to_json_writer};

/// The JSON object key carrying the AST schema version.
///
/// This is an opaque external protocol identifier, deliberately confined to this one constant; do
/// not duplicate the literal elsewhere.
pub const API_VERSION_KEY: &str = "pandoc-api-version";

/// The AST schema version stamped onto freshly constructed documents. Parsed documents echo back
/// the version they were read with instead (see [`ApiVersion`]).
pub const CURRENT_API_VERSION: &[u32] = &[1, 23, 1, 2];
