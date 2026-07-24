//! Greater blocks: `#+begin_…`/`#+end_…` regions and their verbatim content handling.

use std::collections::BTreeMap;

use carta_ast::{Attr, Block, Format, MetaValue, Text};
use carta_core::Extensions;

use crate::heading_ids::IdRegistry;

use super::blocks::parse_blocks;
use super::inline::parse_inlines;
use super::strip_prefix_ci;

/// The block name of a `#+begin_<name>` line, as written (case preserved). Callers compare it
/// case-insensitively.
pub(super) fn greater_block_open(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let rest = strip_prefix_ci(trimmed, "#+begin_")?;
    let name: String = rest
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect::<String>();
    if name.is_empty() { None } else { Some(name) }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn parse_greater_block(
    lines: &[&str],
    start: usize,
    name: &str,
    ext: Extensions,
    notes: &BTreeMap<String, Vec<Block>>,
    ids: &mut IdRegistry,
    meta: &mut BTreeMap<Text, MetaValue>,
) -> (Option<Block>, usize) {
    // `name` came from this open line, so header arguments are whatever follows it there.
    let open_line = lines.get(start).copied().unwrap_or("");
    let header_args = strip_prefix_ci(open_line.trim_start(), "#+begin_")
        .unwrap_or("")
        .get(name.len()..)
        .unwrap_or("")
        .trim();

    let lower = name.to_ascii_lowercase();
    let end_marker = format!("#+end_{lower}");
    let mut depth = 1usize;
    let mut content: Vec<&str> = Vec::new();
    let mut i = start + 1;
    while let Some(&line) = lines.get(i) {
        let t = line.trim_start();
        if let Some(open) = greater_block_open(line)
            && open.eq_ignore_ascii_case(name)
        {
            depth += 1;
        }
        if t.eq_ignore_ascii_case(&end_marker) {
            depth -= 1;
            if depth == 0 {
                i += 1;
                break;
            }
        }
        content.push(line);
        i += 1;
    }
    let consumed = i - start;

    let block = match lower.as_str() {
        "src" => {
            let lang = header_args
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_owned();
            let attr = Attr {
                classes: if lang.is_empty() {
                    vec![]
                } else {
                    vec![lang.into()]
                },
                ..Attr::default()
            };
            Some(Block::CodeBlock(
                Box::new(attr),
                dedent_verbatim(&content).into(),
            ))
        }
        "example" => Some(Block::CodeBlock(
            Box::default(),
            dedent_verbatim(&content).into(),
        )),
        "export" => {
            let fmt = header_args
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_owned();
            Some(Block::RawBlock(
                Format(fmt.into()),
                verbatim(&content).into(),
            ))
        }
        "quote" => Some(Block::BlockQuote(parse_blocks(
            &content, ext, notes, ids, meta,
        ))),
        "verse" => Some(Block::LineBlock(
            content
                .iter()
                .map(|l| parse_inlines(l.trim(), ext, notes))
                .collect(),
        )),
        "comment" => None,
        _ => {
            let attr = Attr {
                classes: vec![name.into()],
                ..Attr::default()
            };
            Some(Block::Div(
                Box::new(attr),
                parse_blocks(&content, ext, notes, ids, meta),
            ))
        }
    };
    (block, consumed)
}

/// Joins verbatim content lines with a trailing newline on each.
fn verbatim(lines: &[&str]) -> String {
    let mut out = String::new();
    for line in lines {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Joins verbatim content, first stripping the common leading indentation shared by all non-blank
/// lines.
fn dedent_verbatim(lines: &[&str]) -> String {
    let indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut out = String::new();
    for line in lines {
        let trimmed = line.get(indent..).unwrap_or("");
        out.push_str(if line.trim().is_empty() {
            line
        } else {
            trimmed
        });
        out.push('\n');
    }
    out
}
