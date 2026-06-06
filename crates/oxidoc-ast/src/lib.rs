//! The oxidoc abstract syntax tree: a pandoc-compatible document model and its JSON
//! serialization (matching pandoc's `pandoc-api-version`).
//!
//! This AST is the single contract between readers and writers (see `docs/PORTING.md` §3–4).
//! The concrete `Pandoc` / `Block` / `Inline` / `Attr` / `MetaValue` types and their serde
//! representation land in slice 0; this crate is currently a placeholder so the workspace builds.
