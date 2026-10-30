//! Separate identifiers and internal links for independently parsed input files.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use carta::ast::{Attr, Block, Document, Inline, slug};
use carta::walk;

pub(super) fn apply(
    document: &mut Document,
    path: &Path,
    inputs: &[PathBuf],
    anchors: &mut BTreeSet<String>,
) -> String {
    let prefix = if path == Path::new("-") {
        String::new()
    } else {
        file_identifier(path)
    };
    if !prefix.is_empty() {
        walk::for_each_document_attribute(document, &mut |attr| {
            if !attr.id.is_empty() {
                attr.id = format!("{prefix}__{}", attr.id).into();
            }
        });
    }
    walk::for_each_document_inline(document, &mut |inline| {
        let (Inline::Link(_, _, target) | Inline::Image(_, _, target)) = inline else {
            return;
        };
        let (file, fragment) = target.url.split_once('#').unwrap_or((&target.url, ""));
        let file = carta::media::percent_decode(file).unwrap_or_else(|| file.to_owned());
        let fragment =
            carta::media::percent_decode(fragment).unwrap_or_else(|| fragment.to_owned());
        let name = if file.is_empty() {
            prefix.clone()
        } else if inputs.iter().any(|path| path.to_string_lossy() == file) {
            file_identifier(Path::new(&file))
        } else {
            return;
        };
        if name.is_empty() {
            return;
        }
        target.url = if fragment.is_empty() {
            anchors.insert(name.clone());
            format!("#{name}")
        } else {
            format!("#{name}__{fragment}")
        }
        .into();
    });
    prefix
}

pub(super) fn add_anchors(
    document: &mut Document,
    starts: &[(String, usize)],
    anchors: &BTreeSet<String>,
) {
    for (name, index) in starts.iter().rev() {
        if !anchors.contains(name) {
            continue;
        }
        let anchor = Inline::Span(
            Box::new(Attr {
                id: name.as_str().into(),
                ..Attr::default()
            }),
            Vec::new(),
        );
        match document.blocks.get_mut(*index) {
            Some(Block::Header(_, _, inlines) | Block::Para(inlines) | Block::Plain(inlines)) => {
                inlines.insert(0, anchor);
            }
            _ => document.blocks.insert(*index, Block::Plain(vec![anchor])),
        }
    }
}

fn file_identifier(path: &Path) -> String {
    let name = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("__");
    slug(&name)
}
