//! The oxidoc abstract syntax tree: the document model and its JSON interchange format.
//!
//! This AST is the single contract between readers and writers (see `docs/PORTING.md` §3–4).
//! The concrete `Document` / `Block` / `Inline` / `Attr` / `MetaValue` types and their serde
//! representation land in slice 0; this crate is currently a placeholder so the workspace builds.
