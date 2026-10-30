//! Keep source directories through normalization and relative-link resolution.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use carta_core::{Extension, ReaderOptions};

pub(super) type Directories = BTreeMap<usize, Rc<PathBuf>>;

pub(super) fn first_directory(options: &ReaderOptions) -> Option<&Path> {
    if !options.extensions.contains(Extension::RebaseRelativePaths) {
        return None;
    }
    options
        .source_dirs
        .range(..=0)
        .next_back()
        .map(|(_, path)| path.as_path())
        .or(options.source_dir.as_deref())
}

pub(super) fn constant(directory: Option<&Path>) -> Directories {
    directory
        .map(|directory| (0, Rc::new(directory.to_path_buf())))
        .into_iter()
        .collect()
}

pub(super) fn directories(
    input: &str,
    options: &ReaderOptions,
    skipped_lines: usize,
) -> Directories {
    if !options.extensions.contains(Extension::RebaseRelativePaths) {
        return Directories::new();
    }
    let mut directories = constant(first_directory(options));
    let mut characters = input.char_indices().peekable();
    let mut line = 0usize;
    for (offset, directory) in &options.source_dirs {
        while characters
            .peek()
            .is_some_and(|(position, _)| position < offset)
        {
            let Some((_, character)) = characters.next() else {
                break;
            };
            if character == '\n'
                || (character == '\r' && characters.peek().is_none_or(|(_, next)| *next != '\n'))
            {
                line += 1;
            }
        }
        directories.insert(
            line.saturating_sub(skipped_lines),
            Rc::new(directory.clone()),
        );
    }
    directories
}

pub(super) fn rebase(url: &str, directory: Option<&Path>) -> String {
    let Some(directory) = directory.filter(|directory| !directory.as_os_str().is_empty()) else {
        return url.to_owned();
    };
    if url.is_empty() || url.starts_with(['/', '#']) || has_scheme(url) {
        return url.to_owned();
    }
    let mut prefix = String::new();
    for character in directory.to_string_lossy().chars() {
        match character {
            '#' => prefix.push_str("%23"),
            '?' => prefix.push_str("%3F"),
            '%' => prefix.push_str("%25"),
            ':' => prefix.push_str("%3A"),
            '\\' => prefix.push_str("%5C"),
            _ => prefix.push(character),
        }
    }
    let prefix = super::scan::escape_uri(&prefix);
    format!("{}/{url}", prefix.trim_end_matches('/'))
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
