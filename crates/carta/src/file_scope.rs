//! Separate identifiers and internal links for independently parsed input files.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use carta::ast::{Attr, Block, Document, Inline, slug};
use carta::walk;

pub(super) fn input_identifiers(inputs: &[PathBuf]) -> BTreeMap<PathBuf, String> {
    let directory = std::env::current_dir().unwrap_or_default();
    let mut identifiers = BTreeMap::new();
    for path in inputs
        .iter()
        .filter(|path| path.as_path() != Path::new("-"))
    {
        identifiers
            .entry(normalize(&directory.join(path)))
            .or_insert_with(|| file_identifier(path));
    }
    identifiers
}

fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir
                if matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                ) =>
            {
                normalized.pop();
            }
            Component::ParentDir if normalized.has_root() => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn has_scheme(url: &str) -> bool {
    let Some((scheme, _)) = url.split_once(':') else {
        return false;
    };
    let mut characters = scheme.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        })
}

pub(super) fn apply(
    document: &mut Document,
    path: &Path,
    identifiers: &BTreeMap<PathBuf, String>,
    anchors: &mut BTreeSet<String>,
    rebased: bool,
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
    let directory = std::env::current_dir().unwrap_or_default();
    let current_file = normalize(&directory.join(path));
    let base = if rebased {
        directory.as_path()
    } else {
        current_file.parent().unwrap_or(&directory)
    };
    walk::for_each_document_inline(document, &mut |inline| {
        let (Inline::Link(_, _, target) | Inline::Image(_, _, target)) = inline else {
            return;
        };
        let (file, fragment) = target.url.split_once('#').unwrap_or((&target.url, ""));
        if has_scheme(file) || file.contains('?') {
            return;
        }
        let file = carta::media::percent_decode(file).unwrap_or_else(|| file.to_owned());
        let fragment =
            carta::media::percent_decode(fragment).unwrap_or_else(|| fragment.to_owned());
        let name = if file.is_empty() {
            prefix.clone()
        } else {
            let target = normalize(&base.join(&file));
            if target == current_file {
                prefix.clone()
            } else if let Some(name) = identifiers.get(&target) {
                name.clone()
            } else {
                return;
            }
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
