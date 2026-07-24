//! Serialization of document metadata to a YAML title block.

use std::borrow::Cow;
use std::fmt::Write;

use carta_ast::{Document, Inline, MetaValue};
use carta_core::{Result, Writer, WriterOptions};

/// Serialize document metadata as a sorted-key YAML block delimited by `---` lines, or `None` when
/// there is no metadata. Scalars render through `writer` so inline markup survives; block values
/// become literal block scalars; sequences and maps nest by indentation. Keys emit in the map's
/// sorted order.
pub(super) fn yaml_metadata_block(
    writer: &dyn Writer,
    document: &Document,
    options: &WriterOptions,
) -> Result<Option<String>> {
    if document.meta.is_empty() {
        return Ok(None);
    }
    let mut out = String::from("---\n");
    for (key, value) in &document.meta {
        yaml_field(&mut out, writer, key, value, 0, options)?;
    }
    out.push_str("---");
    Ok(Some(out))
}

/// Emit one `key: value` field (and any nested lines) at `depth` levels of two-space indentation.
fn yaml_field(
    out: &mut String,
    writer: &dyn Writer,
    key: &str,
    value: &MetaValue,
    depth: usize,
    options: &WriterOptions,
) -> Result<()> {
    let pad = "  ".repeat(depth);
    match value {
        MetaValue::MetaBool(flag) => {
            let _ = writeln!(out, "{pad}{key}: {flag}");
        }
        MetaValue::MetaString(text) => {
            let scalar = writer.render_meta_inlines(&[Inline::Str(text.clone())], options)?;
            push_yaml_scalar(out, &pad, key, &scalar);
        }
        MetaValue::MetaInlines(inlines) => {
            let scalar = writer.render_meta_inlines(inlines, options)?;
            push_yaml_scalar(out, &pad, key, &scalar);
        }
        MetaValue::MetaBlocks(blocks) => {
            let rendered = writer.render_meta_blocks(blocks, options)?;
            let _ = writeln!(out, "{pad}{key}: |");
            push_yaml_literal(out, &pad, &rendered);
        }
        MetaValue::MetaList(items) => {
            let _ = writeln!(out, "{pad}{key}:");
            for item in items {
                yaml_seq_item(out, writer, item, depth, options)?;
            }
        }
        MetaValue::MetaMap(map) => {
            let _ = writeln!(out, "{pad}{key}:");
            for (sub_key, sub_value) in map {
                yaml_field(out, writer, sub_key, sub_value, depth + 1, options)?;
            }
        }
    }
    Ok(())
}

/// Emit one `- value` sequence entry at `depth` levels of indentation. Scalars sit on the dash line;
/// richer values open a nested block under it.
fn yaml_seq_item(
    out: &mut String,
    writer: &dyn Writer,
    value: &MetaValue,
    depth: usize,
    options: &WriterOptions,
) -> Result<()> {
    let pad = "  ".repeat(depth);
    match value {
        MetaValue::MetaBool(flag) => {
            let _ = writeln!(out, "{pad}- {flag}");
        }
        MetaValue::MetaString(text) => {
            let scalar = writer.render_meta_inlines(&[Inline::Str(text.clone())], options)?;
            push_yaml_scalar_item(out, &pad, &scalar);
        }
        MetaValue::MetaInlines(inlines) => {
            let scalar = writer.render_meta_inlines(inlines, options)?;
            push_yaml_scalar_item(out, &pad, &scalar);
        }
        MetaValue::MetaBlocks(blocks) => {
            let rendered = writer.render_meta_blocks(blocks, options)?;
            let _ = writeln!(out, "{pad}- |");
            push_yaml_literal(out, &format!("{pad}  "), &rendered);
        }
        MetaValue::MetaList(items) => {
            let _ = writeln!(out, "{pad}-");
            for item in items {
                yaml_seq_item(out, writer, item, depth + 1, options)?;
            }
        }
        MetaValue::MetaMap(map) => {
            let _ = writeln!(out, "{pad}-");
            for (sub_key, sub_value) in map {
                yaml_field(out, writer, sub_key, sub_value, depth + 1, options)?;
            }
        }
    }
    Ok(())
}

/// Emit a scalar field, choosing a literal block scalar when the value spans multiple lines.
fn push_yaml_scalar(out: &mut String, pad: &str, key: &str, scalar: &str) {
    if scalar.contains('\n') {
        let _ = writeln!(out, "{pad}{key}: |");
        push_yaml_literal(out, pad, scalar);
    } else {
        let _ = writeln!(out, "{pad}{key}: {}", yaml_inline_scalar(scalar));
    }
}

/// Emit a scalar sequence entry, choosing a literal block scalar when the value spans multiple lines.
fn push_yaml_scalar_item(out: &mut String, pad: &str, scalar: &str) {
    if scalar.contains('\n') {
        let _ = writeln!(out, "{pad}- |");
        push_yaml_literal(out, &format!("{pad}  "), scalar);
    } else {
        let _ = writeln!(out, "{pad}- {}", yaml_inline_scalar(scalar));
    }
}

/// Render a single-line scalar as YAML flow text: a double-quoted string when a plain scalar would be
/// reparsed as something other than its text, otherwise the text verbatim.
pub(super) fn yaml_inline_scalar(scalar: &str) -> Cow<'_, str> {
    if yaml_needs_quoting(scalar) {
        Cow::Owned(yaml_quote(scalar))
    } else {
        Cow::Borrowed(scalar)
    }
}

/// Whether a single-line scalar must be double-quoted to round-trip as itself. An empty string,
/// surrounding spaces, an embedded colon or ` #` comment opener, a leading YAML indicator character,
/// or a word YAML reads as a boolean or null all force quoting.
pub(super) fn yaml_needs_quoting(scalar: &str) -> bool {
    if scalar.is_empty() || scalar.starts_with(' ') || scalar.ends_with(' ') {
        return true;
    }
    if scalar.contains(':') || scalar.contains(" #") {
        return true;
    }
    if scalar
        .chars()
        .next()
        .is_some_and(|first| "-?,[]{}#&*!|>'\"%@`".contains(first))
    {
        return true;
    }
    matches!(
        scalar.to_ascii_lowercase().as_str(),
        "true" | "false" | "yes" | "no" | "on" | "off" | "null"
    )
}

/// Double-quote a scalar, escaping backslashes and quotes so it parses back to the same text.
fn yaml_quote(scalar: &str) -> String {
    let mut quoted = String::with_capacity(scalar.len() + 2);
    quoted.push('"');
    for ch in scalar.chars() {
        match ch {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

/// Indent every line of `text` two spaces past `pad`, forming the body of a literal block scalar.
fn push_yaml_literal(out: &mut String, pad: &str, text: &str) {
    for line in text.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            let _ = writeln!(out, "{pad}  {line}");
        }
    }
}
