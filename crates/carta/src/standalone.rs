//! Standalone output: wrap a rendered body in the target format's template.
//!
//! This builds the variable context a template renders against — document metadata rendered through
//! the target writer, the body, a derived plain-text `pagetitle`, and the raw `-V` overlay — and
//! merges the extra metadata layers (`-M` above the document, metadata-file defaults below it) into
//! the document before the body is produced.

use std::collections::BTreeMap;
use std::path::Path;

use carta_ast::{Document, Inline, MetaValue, to_plain_text};
use carta_core::template::{Template, Value};
use carta_core::{MetaVarStyle, Result, Writer, WriterOptions};

/// Layer the extra metadata sources into `document.meta`: metadata-file defaults sit lowest, the
/// document's own values override them, and `-M` values override the document. Merging is whole-key
/// replacement — a higher layer's value for a key replaces the lower layer's entirely (nested maps
/// are not deep-merged).
pub(crate) fn merge_metadata(document: &mut Document, options: &WriterOptions) {
    if options.metadata_defaults.is_empty() && options.metadata.is_empty() {
        return;
    }
    let mut merged = options.metadata_defaults.clone();
    for (key, value) in std::mem::take(&mut document.meta) {
        merged.insert(key, value);
    }
    for (key, value) in &options.metadata {
        merged.insert(key.clone(), value.clone());
    }
    document.meta = merged;
}

/// Wrap `body` in a template, or return `None` when the format has no standalone wrapper and no
/// override was supplied (standalone output then equals the fragment). `to_base` is the target
/// format name, used as the extension for partial files.
pub(crate) fn render(
    writer: &dyn Writer,
    document: &Document,
    body: &str,
    options: &WriterOptions,
    to_base: &str,
) -> Result<Option<String>> {
    // A format whose standalone form is structural (the data form embeds metadata and blocks in one
    // value) builds it directly, bypassing the template engine.
    if let Some(structural) = writer.standalone_document(document, options)? {
        return Ok(Some(structural));
    }
    let source = match &options.template {
        Some(text) => text.clone(),
        None => match writer.default_template() {
            Some(text) => text.to_owned(),
            None => return Ok(None),
        },
    };
    let template = Template::parse(&source)?;
    let context = build_context(document, writer, body, options)?;

    let dir = options.template_dir.clone();
    // A partial inherits the including template's extension; a built-in default has no file, so the
    // format name stands in (its own templates avoid partials, so this only guides user overrides).
    let ext = options
        .template_ext
        .clone()
        .unwrap_or_else(|| to_base.to_owned());
    let resolve = move |name: &str| resolve_partial(dir.as_deref(), &ext, name);
    let mut output = template.render(&context, &resolve)?;
    // A block-level body or metadata value ends in a blank line, so a run of newlines can pile up at
    // the very end of the document; it collapses to a single trailing newline. Output that ends at a
    // glyph is left without one.
    let kept = output.trim_end_matches('\n').len();
    if kept < output.len() {
        output.truncate(kept);
        output.push('\n');
    }
    Ok(Some(output))
}

/// Assemble the template context: every metadata entry rendered through the target writer, the
/// `body`, a plain-text `pagetitle` derived from the title, and the raw `-V` overlay on top.
fn build_context(
    document: &Document,
    writer: &dyn Writer,
    body: &str,
    options: &WriterOptions,
) -> Result<Value> {
    let line_oriented = writer.body_ends_with_newline();
    // A block-level value rendered by a line-oriented writer ends in a blank line, so a metadata
    // variable carries two trailing newlines; `meta-json` keeps the value's plain single-newline
    // form. A writer that ends its output at a glyph adds neither.
    let context_trailing = if line_oriented { "\n\n" } else { "" };
    let json_trailing = if line_oriented { "\n" } else { "" };

    let mut context: BTreeMap<String, Value> = BTreeMap::new();
    let mut meta_json = serde_json::Map::new();
    for (key, value) in &document.meta {
        context.insert(
            key.clone(),
            meta_to_value(value, writer, options, context_trailing)?,
        );
        meta_json.insert(
            key.clone(),
            value_to_json(&meta_to_value(value, writer, options, json_trailing)?),
        );
    }
    context.insert(
        "meta-json".to_owned(),
        Value::Str(serde_json::Value::Object(meta_json).to_string()),
    );
    // Writers that lay the document out as newline-terminated lines carry a trailing blank line into
    // the body variable; an empty body stays empty.
    let body = if line_oriented && !body.is_empty() {
        format!("{body}\n\n")
    } else {
        body.to_owned()
    };
    context.insert("body".to_owned(), Value::Str(body));
    insert_identity_vars(&mut context, document, writer, options)?;
    if let Some(block) = writer.title_block(document, options)? {
        context.insert("titleblock".to_owned(), Value::Str(block));
    }
    overlay_variables(&mut context, &options.variables);
    Ok(Value::Map(context))
}

/// Convert one metadata value to a template value, rendering inline and block content through the
/// target writer so interpolation carries the right markup and escaping for the format. A rendered
/// block sequence gains `block_trailing` so it sits in the surrounding layout the way the format
/// separates blocks.
fn meta_to_value(
    value: &MetaValue,
    writer: &dyn Writer,
    options: &WriterOptions,
    block_trailing: &str,
) -> Result<Value> {
    Ok(match value {
        MetaValue::MetaBool(b) => Value::Bool(*b),
        MetaValue::MetaString(text) => {
            Value::Str(writer.render_meta_inlines(&[Inline::Str(text.clone())], options)?)
        }
        MetaValue::MetaInlines(inlines) => {
            Value::Str(writer.render_meta_inlines(inlines, options)?)
        }
        MetaValue::MetaBlocks(blocks) => {
            let mut rendered = writer.render_meta_blocks(blocks, options)?;
            if !rendered.is_empty() {
                rendered.push_str(block_trailing);
            }
            Value::Str(rendered)
        }
        MetaValue::MetaList(items) => Value::List(
            items
                .iter()
                .map(|item| meta_to_value(item, writer, options, block_trailing))
                .collect::<Result<_>>()?,
        ),
        MetaValue::MetaMap(map) => {
            let mut entries = BTreeMap::new();
            for (key, item) in map {
                entries.insert(
                    key.clone(),
                    meta_to_value(item, writer, options, block_trailing)?,
                );
            }
            Value::Map(entries)
        }
    })
}

/// Encode a rendered metadata value as JSON for the `meta-json` variable: strings, booleans, lists,
/// and keyed maps map to their JSON counterparts, with map keys in sorted order.
fn value_to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Str(text) => serde_json::Value::String(text.clone()),
        Value::Bool(flag) => serde_json::Value::Bool(*flag),
        Value::List(items) => serde_json::Value::Array(items.iter().map(value_to_json).collect()),
        Value::Map(map) => serde_json::Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), value_to_json(item)))
                .collect(),
        ),
    }
}

/// Insert the plain-text identity variables the writer's standalone template draws on — the title,
/// authors, and date rendered as markup-free, target-escaped text (a document `<title>` or a PDF
/// property cannot carry markup). A web document exposes `pagetitle` (the title, falling back to the
/// source name), `date-meta`, and `author-meta` as a list; a PDF document exposes `title-meta` and
/// `author-meta` joined into one string with `; `. A format that exposes none leaves the context
/// untouched. Each variable is omitted when its underlying metadata is absent.
fn insert_identity_vars(
    context: &mut BTreeMap<String, Value>,
    document: &Document,
    writer: &dyn Writer,
    options: &WriterOptions,
) -> Result<()> {
    let style = writer.meta_var_style();
    if style == MetaVarStyle::None {
        return Ok(());
    }
    let title = plain_meta(document, "title");
    let authors = author_plains(document);
    let date = plain_meta(document, "date");

    match style {
        MetaVarStyle::None => {}
        MetaVarStyle::Web => {
            // `pagetitle` is the title, falling back to the source name; present whenever either
            // exists. `date-meta` is present only when the document carries a date. `author-meta` is
            // a list with one entry per author, always defined (empty when there are none).
            let page = if title.is_empty() {
                options.source_name.clone().filter(|name| !name.is_empty())
            } else {
                Some(title)
            };
            if let Some(page) = page {
                context.insert(
                    "pagetitle".to_owned(),
                    Value::Str(render_plain(writer, options, &page)?),
                );
            }
            if !date.is_empty() {
                context.insert(
                    "date-meta".to_owned(),
                    Value::Str(render_plain(writer, options, &date)?),
                );
            }
            let mut list = Vec::with_capacity(authors.len());
            for author in &authors {
                list.push(Value::Str(render_plain(writer, options, author)?));
            }
            context.insert("author-meta".to_owned(), Value::List(list));
        }
        MetaVarStyle::Pdf => {
            // `title-meta` and `author-meta` are always defined (empty when the metadata is absent),
            // so a template may reference them unconditionally.
            context.insert(
                "title-meta".to_owned(),
                Value::Str(render_plain(writer, options, &title)?),
            );
            let mut rendered = Vec::with_capacity(authors.len());
            for author in &authors {
                rendered.push(render_plain(writer, options, author)?);
            }
            context.insert("author-meta".to_owned(), Value::Str(rendered.join("; ")));
        }
    }
    Ok(())
}

/// Render markup-free `text` through the target writer, so an identity variable carries the
/// format's escaping for a plain string.
fn render_plain(writer: &dyn Writer, options: &WriterOptions, text: &str) -> Result<String> {
    writer.render_meta_inlines(&[Inline::Str(text.to_owned())], options)
}

/// The plain, markup-free text of a single-valued inline or string metadata entry; empty when the
/// key is absent or holds a different shape.
fn plain_meta(document: &Document, key: &str) -> String {
    match document.meta.get(key) {
        Some(MetaValue::MetaInlines(inlines)) => to_plain_text(inlines),
        Some(MetaValue::MetaString(text)) => text.clone(),
        _ => String::new(),
    }
}

/// The authors as plain, markup-free text, one entry each. The `author` metadata is a list of
/// authors, a single author, or absent; each author is flattened to plain text and empty entries
/// are dropped.
fn author_plains(document: &Document) -> Vec<String> {
    fn plain_one(value: &MetaValue) -> String {
        match value {
            MetaValue::MetaInlines(inlines) => to_plain_text(inlines),
            MetaValue::MetaString(text) => text.clone(),
            _ => String::new(),
        }
    }
    let names = match document.meta.get("author") {
        Some(MetaValue::MetaList(items)) => items.iter().map(plain_one).collect(),
        Some(value) => vec![plain_one(value)],
        None => Vec::new(),
    };
    names.into_iter().filter(|name| !name.is_empty()).collect()
}

/// Overlay the raw `-V` variables at the highest precedence: each replaces any metadata-derived
/// value for its key, and a key supplied more than once accumulates into a list in order.
fn overlay_variables(context: &mut BTreeMap<String, Value>, variables: &[(String, String)]) {
    let mut overlay: BTreeMap<String, Value> = BTreeMap::new();
    for (key, val) in variables {
        let next = match overlay.remove(key) {
            None => Value::Str(val.clone()),
            Some(Value::List(mut items)) => {
                items.push(Value::Str(val.clone()));
                Value::List(items)
            }
            Some(first) => Value::List(vec![first, Value::Str(val.clone())]),
        };
        overlay.insert(key.clone(), next);
    }
    for (key, value) in overlay {
        context.insert(key, value);
    }
}

/// Resolve a partial `$name()$` to its source text by reading from `dir`. A name carrying its own
/// extension is read verbatim; otherwise it takes the including template's extension `ext`, or is
/// looked up bare when that extension is empty (the including template had none).
fn resolve_partial(dir: Option<&Path>, ext: &str, name: &str) -> Option<String> {
    let dir = dir?;
    let filename = if ext.is_empty() || Path::new(name).extension().is_some() {
        name.to_owned()
    } else {
        format!("{name}.{ext}")
    };
    std::fs::read_to_string(dir.join(filename)).ok()
}
