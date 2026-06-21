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
use carta_core::{Result, Writer, WriterOptions};

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
    // A standalone document carries at most one trailing newline beyond its last line: when the
    // filled template ends in a blank line (its final line and the body both end in a newline),
    // one of the two is dropped so no spurious blank trails the document. A single or absent
    // trailing newline is left untouched.
    if output.ends_with("\n\n") {
        output.pop();
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
    let mut context: BTreeMap<String, Value> = BTreeMap::new();
    for (key, value) in &document.meta {
        context.insert(key.clone(), meta_to_value(value, writer, options)?);
    }
    // Writers that lay the document out as newline-terminated lines carry that final newline into
    // the body variable; an empty body stays empty.
    let body = if writer.body_ends_with_newline() && !body.is_empty() {
        format!("{body}\n")
    } else {
        body.to_owned()
    };
    context.insert("body".to_owned(), Value::Str(body));
    if let Some(page_title) = pagetitle(document, writer, options)? {
        context.insert("pagetitle".to_owned(), Value::Str(page_title));
    }
    if let Some(block) = writer.title_block(document, options)? {
        context.insert("titleblock".to_owned(), Value::Str(block));
    }
    overlay_variables(&mut context, &options.variables);
    Ok(Value::Map(context))
}

/// Convert one metadata value to a template value, rendering inline and block content through the
/// target writer so interpolation carries the right markup and escaping for the format.
fn meta_to_value(value: &MetaValue, writer: &dyn Writer, options: &WriterOptions) -> Result<Value> {
    Ok(match value {
        MetaValue::MetaBool(b) => Value::Bool(*b),
        MetaValue::MetaString(text) => {
            Value::Str(writer.render_meta_inlines(&[Inline::Str(text.clone())], options)?)
        }
        MetaValue::MetaInlines(inlines) => {
            Value::Str(writer.render_meta_inlines(inlines, options)?)
        }
        MetaValue::MetaBlocks(blocks) => Value::Str(writer.render_meta_blocks(blocks, options)?),
        MetaValue::MetaList(items) => Value::List(
            items
                .iter()
                .map(|item| meta_to_value(item, writer, options))
                .collect::<Result<_>>()?,
        ),
        MetaValue::MetaMap(map) => {
            let mut entries = BTreeMap::new();
            for (key, item) in map {
                entries.insert(key.clone(), meta_to_value(item, writer, options)?);
            }
            Value::Map(entries)
        }
    })
}

/// Derive `pagetitle` — the title as plain, markup-free, target-escaped text (a document `<title>`
/// and the like cannot carry markup). `None` when there is no non-empty title.
fn pagetitle(
    document: &Document,
    writer: &dyn Writer,
    options: &WriterOptions,
) -> Result<Option<String>> {
    let plain = match document.meta.get("title") {
        Some(MetaValue::MetaInlines(inlines)) => to_plain_text(inlines),
        Some(MetaValue::MetaString(text)) => text.clone(),
        _ => String::new(),
    };
    if plain.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        writer.render_meta_inlines(&[Inline::Str(plain)], options)?,
    ))
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
