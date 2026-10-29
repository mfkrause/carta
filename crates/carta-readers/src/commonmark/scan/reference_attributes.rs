//! Attributes that follow a Markdown reference destination and title.

use std::collections::BTreeMap;

use carta_ast::Attr;

use super::{char_at, scan_destination, scan_title, unescape_string};

fn key_end(text: &str, start: usize) -> Option<usize> {
    if !char_at(text, start)?.is_alphabetic() {
        return None;
    }
    let mut index = start;
    while let Some(character) = char_at(text, index) {
        if character.is_alphanumeric() || matches!(character, '_' | '-' | '.' | ':') {
            index += character.len_utf8();
        } else {
            break;
        }
    }
    (char_at(text, index) == Some('=')).then_some(index)
}

pub(super) fn find(text: &str, start: usize) -> Option<usize> {
    let mut index = start;
    loop {
        let end = if matches!(char_at(text, index), Some('"' | '\'' | '('))
            && let Some((_, end)) = scan_title(text, index)
        {
            end
        } else if char_at(text, index) == Some('<') {
            scan_destination(text, index)?.1
        } else {
            let mut end = index;
            let mut depth = 0usize;
            while let Some(character) = char_at(text, end) {
                if character == '\n' || (depth == 0 && matches!(character, ' ' | '\t')) {
                    break;
                }
                if character == '\\'
                    && let Some(next) = char_at(text, end + 1)
                    && (matches!(next, ' ' | '\t') || super::is_ascii_punctuation(next))
                {
                    end += 1 + next.len_utf8();
                    continue;
                }
                match character {
                    '(' => depth += 1,
                    ')' => depth = depth.saturating_sub(1),
                    _ => {}
                }
                end += character.len_utf8();
            }
            end
        };
        if end == index {
            return None;
        }
        index = end;
        let mut newline = false;
        while let Some(character) = char_at(text, index) {
            match character {
                ' ' | '\t' => index += 1,
                '\n' if !newline => {
                    newline = true;
                    index += 1;
                }
                _ => break,
            }
        }
        if index == end {
            return None;
        }
        if key_end(text, index).is_some() {
            return Some(index);
        }
        if newline && !matches!(char_at(text, index), Some('"' | '\'' | '(')) {
            return None;
        }
    }
}

pub(super) fn parse(text: &str, start: usize) -> Option<(Attr, usize)> {
    let mut attributes = Attr::default();
    let mut positions = BTreeMap::new();
    let mut index = start;
    loop {
        let end = key_end(text, index)?;
        let key = text.get(index..end)?;
        index = end + 1;
        let (value, end) = if matches!(char_at(text, index), Some('"' | '\''))
            && let Some((_, end)) = scan_title(text, index)
        {
            (text.get(index + 1..end.checked_sub(1)?)?.to_owned(), end)
        } else {
            let mut value = String::new();
            while let Some(character) = char_at(text, index) {
                if matches!(character, ' ' | '\t' | '\n') {
                    break;
                }
                if character == '}' {
                    return None;
                }
                if character == '\\'
                    && let Some(next) = char_at(text, index + 1)
                    && (matches!(next, ' ' | '\t') || super::is_ascii_punctuation(next))
                {
                    if !matches!(next, ' ' | '\t') {
                        value.push('\\');
                    }
                    value.push(next);
                    index += 1 + next.len_utf8();
                } else {
                    value.push(character);
                    index += character.len_utf8();
                }
            }
            (value, index)
        };
        index = end;
        let value = unescape_string(&value.replace('\n', " "));
        match key {
            "id" => attributes.id = value.into(),
            "class" => attributes
                .classes
                .extend(value.split_whitespace().map(Into::into)),
            _ => {
                if let Some(position) = positions.get(key) {
                    if let Some((_, previous)) = attributes.attributes.get_mut(*position) {
                        *previous = value.into();
                    }
                } else {
                    positions.insert(key, attributes.attributes.len());
                    attributes.attributes.push((key.into(), value.into()));
                }
            }
        }
        while matches!(char_at(text, index), Some(' ' | '\t')) {
            index += 1;
        }
        if char_at(text, index).is_none() {
            return Some((attributes, index));
        }
        if char_at(text, index) == Some('\n') {
            let end = index + 1;
            index = end;
            while matches!(char_at(text, index), Some(' ' | '\t')) {
                index += 1;
            }
            if key_end(text, index).is_none() {
                return Some((attributes, end));
            }
        }
    }
}
