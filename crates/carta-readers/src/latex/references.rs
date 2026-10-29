//! Resolve numbered links after all labelled targets are known.

use std::collections::BTreeMap;

use carta_ast::{Block, Document, Inline};
use carta_core::walk::{for_each_block, for_each_document_inline};

use super::LABEL_ATTRIBUTE;

#[allow(clippy::too_many_lines)]
pub(super) fn resolve(document: &mut Document, base_level: i32) {
    let mut has_chapters = false;
    for_each_block(&mut document.blocks, &mut |block| {
        if let Block::Header(level, _, _) = block
            && *level + i64::from(base_level) - 1 == 0
        {
            has_chapters = true;
        }
    });
    let mut targets = BTreeMap::new();
    let mut levels = [0_u64; 6];
    let mut figures = 0_u64;
    let mut tables = 0_u64;
    let mut parts = 0_u64;
    for_each_block(&mut document.blocks, &mut |block| {
        if let Block::Header(level, attributes, _) = block {
            let labelled = attributes
                .attributes
                .iter()
                .any(|(key, _)| key == LABEL_ATTRIBUTE);
            attributes
                .attributes
                .retain(|(key, _)| key != LABEL_ATTRIBUTE);
            if attributes.classes.iter().any(|class| class == "unnumbered") {
                return;
            }
            let intrinsic = *level + i64::from(base_level) - 1;
            let number = if intrinsic < 0 {
                parts = parts.saturating_add(1);
                if !labelled {
                    return;
                }
                roman(parts)
            } else {
                let Ok(position) = usize::try_from(intrinsic) else {
                    return;
                };
                let Some(counter) = levels.get_mut(position) else {
                    return;
                };
                *counter = counter.saturating_add(1);
                for counter in levels.iter_mut().skip(position + 1) {
                    *counter = 0;
                }
                if position == 0 {
                    figures = 0;
                    tables = 0;
                }
                if !labelled {
                    return;
                }
                let first = usize::from(!has_chapters);
                levels
                    .iter()
                    .skip(first)
                    .take(position + 1 - first)
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(".")
            };
            targets.insert(attributes.id.clone(), number);
            return;
        }
        let (attributes, counter) = match block {
            Block::Figure(attributes, caption, _) if !caption.long.is_empty() => {
                (attributes.as_mut(), &mut figures)
            }
            Block::Table(table) if !table.caption.long.is_empty() => (&mut table.attr, &mut tables),
            _ => return,
        };
        *counter = counter.saturating_add(1);
        if attributes.id.is_empty() {
            return;
        }
        let chapter = levels.first().copied().unwrap_or(0);
        let number = if chapter > 0 {
            format!("{chapter}.{counter}")
        } else {
            counter.to_string()
        };
        targets.insert(attributes.id.clone(), number);
    });
    for_each_document_inline(document, &mut |inline| {
        let Inline::Link(attributes, label, _) = inline else {
            return;
        };
        let kind = attributes
            .attributes
            .iter()
            .find(|(key, _)| key == "reference-type")
            .map(|(_, value)| value);
        if !kind.is_some_and(|kind| matches!(kind.as_str(), "ref" | "ref+label" | "ref+Label")) {
            return;
        }
        let Some((_, target)) = attributes
            .attributes
            .iter()
            .find(|(key, _)| key == "reference")
        else {
            return;
        };
        if let Some(number) = targets.get(target) {
            *label = vec![Inline::Str(number.clone().into())];
        }
    });
}

fn roman(mut number: u64) -> String {
    let mut output = String::new();
    for (value, text) in [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ] {
        while number >= value {
            output.push_str(text);
            number -= value;
        }
    }
    output
}
