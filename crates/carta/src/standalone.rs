//! Standalone output: wrap a rendered body in the target format's template.
//!
//! This builds the variable context a template renders against — document metadata rendered through
//! the target writer, the body, a derived plain-text `pagetitle`, and the raw `-V` overlay — and
//! merges the extra metadata layers (`-M` above the document, metadata-file defaults below it) into
//! the document before the body is produced.

use std::collections::BTreeMap;
use std::path::Path;

use carta_ast::{
    Block, Document, Inline, MetaValue, Text, single_block_inlines, to_plain_inlines, to_plain_text,
};
use carta_core::sections::build_toc;
use carta_core::template::{Template, Value};
use carta_core::{MathMethod, MetaVarStyle, Result, TocStyle, Writer, WriterOptions};

/// The deepest heading level a table of contents includes when no explicit depth is given.
pub(crate) const DEFAULT_TOC_DEPTH: usize = 3;

/// Where a list-style table of contents comes from: built from the document's blocks when the
/// context is assembled, or built earlier from the unnumbered tree because section numbers have
/// since been spliced into the headings in place (building it late would number its entries twice).
pub(crate) enum TocSource {
    Document,
    Prebuilt(Option<Block>),
}

/// Layer the extra metadata sources into `document.meta`: metadata-file defaults sit lowest, the
/// document's own values override them, and `-M` values override the document. Merging is whole-key
/// replacement — a higher layer's value for a key replaces the lower layer's entirely (nested maps
/// are not deep-merged).
pub(crate) fn merge_metadata(document: &mut Document, options: &WriterOptions) {
    if options.metadata_defaults.is_empty() && options.metadata.is_empty() {
        return;
    }
    let mut merged: BTreeMap<Text, MetaValue> = options
        .metadata_defaults
        .iter()
        .map(|(key, value)| (Text::from(key.as_str()), value.clone()))
        .collect();
    for (key, value) in std::mem::take(&mut document.meta) {
        merged.insert(key, value);
    }
    for (key, value) in &options.metadata {
        merged.insert(Text::from(key.as_str()), value.clone());
    }
    document.meta = merged;
}

/// Wrap `body` in a template, or return it unchanged when the format has no standalone wrapper and
/// no override was supplied (standalone output then equals the fragment). `to_base` is the target
/// format name, used as the extension for partial files.
pub(crate) fn render(
    writer: &dyn Writer,
    document: &Document,
    body: String,
    options: &WriterOptions,
    to_base: &str,
    toc_source: TocSource,
) -> Result<String> {
    // A format whose standalone form is structural (the data form embeds metadata and blocks in one
    // value) builds it directly, bypassing the template engine.
    if let Some(structural) = writer.standalone_document(document, options)? {
        return Ok(structural);
    }
    let source: &str = match &options.template {
        Some(text) => text.as_ref(),
        None => match writer.default_template() {
            Some(text) => text,
            None => return Ok(body),
        },
    };
    let template = Template::parse(source)?;
    let context = build_context(document, writer, body, options, toc_source)?;

    let dir = options.template_dir.clone();
    let datadir = options.template_datadir.clone();
    // A partial inherits the including template's extension; a built-in default has no file, so the
    // format name stands in (its own templates avoid partials, so this only guides user overrides).
    let ext = options
        .template_ext
        .clone()
        .unwrap_or_else(|| to_base.to_owned());
    let resolve = move |name: &str| resolve_partial(dir.as_deref(), datadir.as_deref(), &ext, name);
    let mut output = template.render(&context, &resolve)?;
    // A block-level body or metadata value ends in a blank line, so a run of newlines can pile up at
    // the very end of the document; it collapses to a single trailing newline. Output that ends at a
    // glyph is left without one.
    let kept = output.trim_end_matches('\n').len();
    if kept < output.len() {
        output.truncate(kept);
        output.push('\n');
    }
    Ok(output)
}

/// Assemble the template context: every metadata entry rendered through the target writer, the
/// `body`, a plain-text `pagetitle` derived from the title, and the raw `-V` overlay on top.
fn build_context(
    document: &Document,
    writer: &dyn Writer,
    mut body: String,
    options: &WriterOptions,
    toc_source: TocSource,
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
        // A writer that flattens block metadata builds the two forms from different content
        // (inline flattening against full blocks), so each is rendered on its own; every other
        // writer renders each leaf once, the forms differing only in the trailing appended to
        // block-shaped content.
        let (context_value, json) = if writer.flatten_block_metadata() {
            let context_value = meta_to_value(value, writer, options, BlockMode::Inline)?;
            let json_value = meta_to_value(
                value,
                writer,
                options,
                BlockMode::Full {
                    trailing: json_trailing,
                },
            )?;
            (context_value, value_to_json(&json_value))
        } else {
            meta_to_value_pair(value, writer, options, context_trailing, json_trailing)?
        };
        meta_json.insert(key.to_string(), json);
        context.insert(key.to_string(), context_value);
    }
    context.insert(
        "meta-json".to_owned(),
        Value::Str(serde_json::Value::Object(meta_json).to_string()),
    );
    // Writers that lay the document out as newline-terminated lines carry a trailing blank line into
    // the body variable; an empty body stays empty.
    if line_oriented && !body.is_empty() {
        body.push_str("\n\n");
    }
    insert_identity_vars(&mut context, document, writer, options)?;
    insert_output_vars(&mut context, document, writer, options, &body, toc_source)?;
    context.insert("body".to_owned(), Value::Str(body));
    if let Some(block) = writer.title_block(document, options)? {
        context.insert("titleblock".to_owned(), Value::Str(block));
    }
    overlay_variables(&mut context, &options.variables);
    // A requested link, file, citation, URL, or table-of-contents color implies colored links — a
    // property of the assembled context, not of any one writer — so it is applied uniformly. A
    // template that exposes neither colors nor `colorlinks` is simply unaffected.
    enable_colorlinks(&mut context);
    Ok(Value::Map(context))
}

/// Turn on `colorlinks` whenever a specific link, file, citation, URL, or table-of-contents color is
/// set: requesting a color implies colored links. A `colorlinks` already supplied by the document or
/// an overlay is left as is.
fn enable_colorlinks(context: &mut BTreeMap<String, Value>) {
    if context.get("colorlinks").is_some_and(Value::is_truthy) {
        return;
    }
    let any_color = [
        "linkcolor",
        "filecolor",
        "citecolor",
        "urlcolor",
        "toccolor",
    ]
    .iter()
    .any(|key| context.get(*key).is_some_and(Value::is_truthy));
    if any_color {
        context.insert("colorlinks".to_owned(), Value::Bool(true));
    }
}

/// How a metadata value's block-shaped content becomes a template value.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockMode<'a> {
    /// Render blocks as themselves, appending `trailing` so they sit in the surrounding layout the
    /// way the format separates blocks.
    Full { trailing: &'a str },
    /// Flatten a lone-paragraph block to its inline content; any other block shape becomes empty.
    /// Used for a writer that draws metadata into single-line header fields.
    Inline,
}

/// Convert one metadata value to a template value, rendering inline and block content through the
/// target writer so interpolation carries the right markup and escaping for the format. `mode`
/// decides how a block sequence is treated; inline, string, and boolean values render the same way
/// regardless, and lists and maps recurse with the same `mode`.
fn meta_to_value(
    value: &MetaValue,
    writer: &dyn Writer,
    options: &WriterOptions,
    mode: BlockMode,
) -> Result<Value> {
    Ok(match value {
        MetaValue::MetaBool(b) => Value::Bool(*b),
        MetaValue::MetaString(text) => {
            Value::Str(writer.render_meta_inlines(&[Inline::Str(text.clone())], options)?)
        }
        MetaValue::MetaInlines(inlines) => {
            Value::Str(writer.render_meta_inlines(inlines, options)?)
        }
        MetaValue::MetaBlocks(blocks) => match mode {
            BlockMode::Full { trailing } => {
                let mut rendered = writer.render_meta_blocks(blocks, options)?;
                if !rendered.is_empty() {
                    rendered.push_str(trailing);
                }
                Value::Str(rendered)
            }
            BlockMode::Inline => {
                Value::Str(writer.render_meta_inlines(single_block_inlines(blocks), options)?)
            }
        },
        MetaValue::MetaList(items) => Value::List(
            items
                .iter()
                .map(|item| meta_to_value(item, writer, options, mode))
                .collect::<Result<_>>()?,
        ),
        MetaValue::MetaMap(map) => {
            let mut entries = BTreeMap::new();
            for (key, item) in map {
                entries.insert(key.to_string(), meta_to_value(item, writer, options, mode)?);
            }
            Value::Map(entries)
        }
    })
}

/// Convert one metadata value to its template form and its `meta-json` form together, rendering
/// each inline or block leaf through the target writer once. The two forms are identical except
/// for the trailing appended to non-empty block-shaped content: `context_trailing` on the template
/// side, `json_trailing` on the JSON side.
fn meta_to_value_pair(
    value: &MetaValue,
    writer: &dyn Writer,
    options: &WriterOptions,
    context_trailing: &str,
    json_trailing: &str,
) -> Result<(Value, serde_json::Value)> {
    Ok(match value {
        MetaValue::MetaBool(b) => (Value::Bool(*b), serde_json::Value::Bool(*b)),
        MetaValue::MetaString(text) => {
            let rendered = writer.render_meta_inlines(&[Inline::Str(text.clone())], options)?;
            (
                Value::Str(rendered.clone()),
                serde_json::Value::String(rendered),
            )
        }
        MetaValue::MetaInlines(inlines) => {
            let rendered = writer.render_meta_inlines(inlines, options)?;
            (
                Value::Str(rendered.clone()),
                serde_json::Value::String(rendered),
            )
        }
        MetaValue::MetaBlocks(blocks) => {
            let rendered = writer.render_meta_blocks(blocks, options)?;
            let mut json_form = rendered.clone();
            let mut context_form = rendered;
            if !context_form.is_empty() {
                context_form.push_str(context_trailing);
                json_form.push_str(json_trailing);
            }
            (
                Value::Str(context_form),
                serde_json::Value::String(json_form),
            )
        }
        MetaValue::MetaList(items) => {
            let mut context_items = Vec::with_capacity(items.len());
            let mut json_items = Vec::with_capacity(items.len());
            for item in items {
                let (context_item, json_item) =
                    meta_to_value_pair(item, writer, options, context_trailing, json_trailing)?;
                context_items.push(context_item);
                json_items.push(json_item);
            }
            (
                Value::List(context_items),
                serde_json::Value::Array(json_items),
            )
        }
        MetaValue::MetaMap(map) => {
            let mut context_entries = BTreeMap::new();
            let mut json_entries = serde_json::Map::new();
            for (key, item) in map {
                let (context_item, json_item) =
                    meta_to_value_pair(item, writer, options, context_trailing, json_trailing)?;
                context_entries.insert(key.to_string(), context_item);
                json_entries.insert(key.to_string(), json_item);
            }
            (
                Value::Map(context_entries),
                serde_json::Value::Object(json_entries),
            )
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
/// authors, and date stripped of markup but with quotation preserved, then rendered through the
/// target writer (a document `<title>` or a PDF property carries no styling, but its quote glyphs
/// still belong to the format). A web document exposes `pagetitle` (the title, falling back to the
/// source name), `date-meta`, and `author-meta` as a list; a PDF document exposes `title-meta` and
/// `author-meta` joined into one string with `; `. A format that exposes none leaves the context
/// untouched. Each variable is omitted when its underlying metadata is absent.
fn insert_identity_vars(
    context: &mut BTreeMap<String, Value>,
    document: &Document,
    writer: &dyn Writer,
    options: &WriterOptions,
) -> Result<()> {
    // The plain-text forms decide presence (whether a key contributes any text at all); the inline
    // forms carry the quotation that survives into the rendered variable.
    let title_text = plain_meta(document, "title");
    let title = plain_meta_inlines(document, "title");
    let authors = author_plain_inlines(document);
    let date_text = plain_meta(document, "date");
    let date = plain_meta_inlines(document, "date");

    match writer.meta_var_style() {
        MetaVarStyle::None => {}
        MetaVarStyle::Web => {
            // `pagetitle` is the title, falling back to the source name; present whenever either
            // exists. `date-meta` is present only when the document carries a date. `author-meta` is
            // a list with one entry per author, always defined (empty when there are none).
            let page = if title_text.is_empty() {
                options
                    .source_name
                    .clone()
                    .filter(|name| !name.is_empty())
                    .map(|name| vec![Inline::Str(name.into())])
            } else {
                Some(title)
            };
            if let Some(page) = page {
                context.insert(
                    "pagetitle".to_owned(),
                    Value::Str(writer.render_meta_inlines(&page, options)?),
                );
            }
            if !date_text.is_empty() {
                context.insert(
                    "date-meta".to_owned(),
                    Value::Str(writer.render_meta_inlines(&date, options)?),
                );
            }
            let mut list = Vec::with_capacity(authors.len());
            for author in &authors {
                list.push(Value::Str(writer.render_meta_inlines(author, options)?));
            }
            context.insert("author-meta".to_owned(), Value::List(list));
        }
        MetaVarStyle::Pdf => {
            // `title-meta` and `author-meta` are always defined (empty when the metadata is absent),
            // so a template may reference them unconditionally.
            context.insert(
                "title-meta".to_owned(),
                Value::Str(writer.render_meta_inlines(&title, options)?),
            );
            let mut rendered = Vec::with_capacity(authors.len());
            for author in &authors {
                rendered.push(writer.render_meta_inlines(author, options)?);
            }
            context.insert("author-meta".to_owned(), Value::Str(rendered.join("; ")));
        }
    }
    Ok(())
}

/// Insert the variables that drive a standalone document's table of contents, section numbering, and
/// math typesetting. A contents request exposes `toc-depth` and `toc` — a list rendered through the
/// target writer, or a boolean flag for a format that assembles its own contents from a template
/// directive. Native section numbering exposes `numbersections`. A math renderer exposes its loader's
/// flag and URL. A document that requests none of these leaves the context untouched.
fn insert_output_vars(
    context: &mut BTreeMap<String, Value>,
    document: &Document,
    writer: &dyn Writer,
    options: &WriterOptions,
    body: &str,
    toc_source: TocSource,
) -> Result<()> {
    if options.toc {
        let depth = options.toc_depth.unwrap_or(DEFAULT_TOC_DEPTH);
        match writer.toc_style() {
            TocStyle::Native => {
                context.insert("toc".to_owned(), Value::Bool(true));
            }
            TocStyle::List => {
                let toc = match toc_source {
                    TocSource::Document => build_toc(
                        &document.blocks,
                        depth,
                        options.number_sections,
                        writer.toc_link_anchors(),
                    ),
                    TocSource::Prebuilt(block) => block,
                };
                if let Some(block) = toc {
                    let rendered = writer.render_meta_blocks(&[block], options)?;
                    context.insert("toc".to_owned(), Value::Str(rendered));
                }
            }
        }
        context.insert("toc-depth".to_owned(), Value::Str(depth.to_string()));
    }
    if options.number_sections && writer.numbers_sections_natively() {
        context.insert("numbersections".to_owned(), Value::Bool(true));
    }
    match &options.math_method {
        MathMethod::Plain => {}
        MathMethod::MathJax(url) => {
            context.insert("mathjax".to_owned(), Value::Bool(true));
            context.insert("mathjaxurl".to_owned(), Value::Str(url.clone()));
        }
        MathMethod::Katex(url) => {
            context.insert("katex".to_owned(), Value::Bool(true));
            context.insert("katexurl".to_owned(), Value::Str(url.clone()));
        }
    }
    insert_highlight_vars(context, writer, options, body);
    Ok(())
}

/// Inject the standalone highlighting variables the template preamble needs: a web target that
/// carries colorized code gets the theme's stylesheet in `highlighting-css`; a print target gets the
/// per-token macros in `highlighting-macros`; and idiomatic print output, which routes code through
/// the target's own listing construct, sets `listings` so its package preamble is emitted.
#[cfg(feature = "highlight")]
fn insert_highlight_vars(
    context: &mut BTreeMap<String, Value>,
    writer: &dyn Writer,
    options: &WriterOptions,
    body: &str,
) {
    match writer.meta_var_style() {
        MetaVarStyle::Web => {
            if let Some(theme) = &options.highlight.theme
                && body.contains("class=\"sourceCode")
            {
                context.insert(
                    "highlighting-css".to_owned(),
                    Value::Str(carta_writers::theme_css(theme)),
                );
            }
        }
        MetaVarStyle::Pdf => {
            if options.highlight.idiomatic {
                context.insert("listings".to_owned(), Value::Bool(true));
            }
            if let Some(theme) = &options.highlight.theme
                && (body.contains("\\begin{Highlighting}") || body.contains("\\VERB"))
            {
                context.insert(
                    "highlighting-macros".to_owned(),
                    Value::Str(carta_writers::theme_latex_macros(theme)),
                );
            }
        }
        MetaVarStyle::None => {}
    }
}

#[cfg(not(feature = "highlight"))]
fn insert_highlight_vars(
    _context: &mut BTreeMap<String, Value>,
    _writer: &dyn Writer,
    _options: &WriterOptions,
    _body: &str,
) {
}

/// The plain, markup-free text of a single-valued inline or string metadata entry; empty when the
/// key is absent or holds a different shape. Used to decide whether the entry contributes any text.
fn plain_meta(document: &Document, key: &str) -> String {
    match document.meta.get(key) {
        Some(MetaValue::MetaInlines(inlines)) => to_plain_text(inlines),
        Some(MetaValue::MetaString(text)) => text.to_string(),
        Some(MetaValue::MetaBlocks(blocks)) => to_plain_text(single_block_inlines(blocks)),
        _ => String::new(),
    }
}

/// A single-valued metadata entry stripped to plain text but keeping quotation, as an inline
/// sequence ready to render through the target writer; empty when the key is absent or holds a
/// different shape.
fn plain_meta_inlines(document: &Document, key: &str) -> Vec<Inline> {
    match document.meta.get(key) {
        Some(MetaValue::MetaInlines(inlines)) => to_plain_inlines(inlines),
        Some(MetaValue::MetaString(text)) if !text.is_empty() => vec![Inline::Str(text.clone())],
        Some(MetaValue::MetaBlocks(blocks)) => to_plain_inlines(single_block_inlines(blocks)),
        _ => Vec::new(),
    }
}

/// The authors as markup-stripped, quotation-preserving inline sequences, one entry each. The
/// `author` metadata is a list of authors, a single author, or absent; each author is flattened and
/// entries that carry no text are dropped.
fn author_plain_inlines(document: &Document) -> Vec<Vec<Inline>> {
    fn plain_one(value: &MetaValue) -> (String, Vec<Inline>) {
        match value {
            MetaValue::MetaInlines(inlines) => (to_plain_text(inlines), to_plain_inlines(inlines)),
            MetaValue::MetaString(text) => (text.to_string(), vec![Inline::Str(text.clone())]),
            MetaValue::MetaBlocks(blocks) => {
                let inlines = single_block_inlines(blocks);
                (to_plain_text(inlines), to_plain_inlines(inlines))
            }
            _ => (String::new(), Vec::new()),
        }
    }
    let entries: Vec<(String, Vec<Inline>)> = match document.meta.get("author") {
        Some(MetaValue::MetaList(items)) => items.iter().map(plain_one).collect(),
        Some(value) => vec![plain_one(value)],
        None => Vec::new(),
    };
    entries
        .into_iter()
        .filter(|(text, _)| !text.is_empty())
        .map(|(_, inlines)| inlines)
        .collect()
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

/// Resolve a partial `$name()$` to its source text. A name carrying its own extension is read
/// verbatim; otherwise it takes the including template's extension `ext`, or is looked up bare when
/// that extension is empty (the including template had none). The including template's own directory
/// `dir` is consulted first; a shared `datadir` of partials supplies those common to several
/// templates.
fn resolve_partial(
    dir: Option<&Path>,
    datadir: Option<&Path>,
    ext: &str,
    name: &str,
) -> Option<String> {
    let filename = if ext.is_empty() || Path::new(name).extension().is_some() {
        name.to_owned()
    } else {
        format!("{name}.{ext}")
    };
    dir.and_then(|dir| std::fs::read_to_string(dir.join(&filename)).ok())
        .or_else(|| {
            datadir.and_then(|datadir| std::fs::read_to_string(datadir.join(&filename)).ok())
        })
}
