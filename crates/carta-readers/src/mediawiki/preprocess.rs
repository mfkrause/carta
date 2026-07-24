//! Source preprocessing: tab expansion, comment stripping, and behavior switch extraction.

use super::tags::verbatim_region_end;
use super::{ScanBounds, at, collect_range, find_seq, matches_prefix_ci};

/// Expands tab characters to spaces on a four-column grid, with the column resetting at each line
/// break. Wikitext markup is column-sensitive (a leading space marks preformatted text), so tabs
/// are normalized before any block scanning runs.
///
/// A deliberate variant of [`crate::tabs::expand_tabs`]: it runs over the whole input at once
/// (resetting the column at line breaks) rather than line by line.
pub(super) fn expand_tabs(input: &str) -> String {
    if !input.contains('\t') {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut col = 0usize;
    for ch in input.chars() {
        match ch {
            '\t' => {
                let spaces = 4 - (col % 4);
                for _ in 0..spaces {
                    out.push(' ');
                }
                col += spaces;
            }
            '\n' => {
                out.push('\n');
                col = 0;
            }
            other => {
                out.push(other);
                col += 1;
            }
        }
    }
    out
}

/// Removes wikitext comments. A comment that is the whole line (preceded by a line start and
/// followed by a line end) is dropped together with its trailing newline; one embedded in other
/// text collapses to a single space. Verbatim regions (`pre`, `nowiki`, `math`, `source`,
/// `syntaxhighlight`) are copied unchanged so comment-like text inside them survives. An
/// unterminated `<!--` is left as literal text.
pub(super) fn strip_comments(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let n = chars.len();
    let bounds = ScanBounds::of(&chars);
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        let Some(c) = at(&chars, i) else { break };
        if c == '<' && bounds.open_possible(i) {
            if let Some(after) = verbatim_region_end(&chars, i, bounds) {
                out.push_str(&collect_range(&chars, i, after));
                i = after;
                continue;
            }
            if matches_prefix_ci(&chars, i, "<!--") {
                if let Some(dash) = find_seq(&chars, i + 4, &['-', '-', '>']) {
                    let comment_end = dash + 3;
                    let preceded = i == 0 || at(&chars, i - 1) == Some('\n');
                    let followed = comment_end >= n || at(&chars, comment_end) == Some('\n');
                    if preceded && followed {
                        i = if comment_end < n {
                            comment_end + 1
                        } else {
                            comment_end
                        };
                    } else if preceded || followed {
                        // At a line boundary the comment leaves nothing behind: no leading space (would mark preformatted), no trailing one.
                        i = comment_end;
                    } else {
                        // Between text, the comment collapses to a single space.
                        out.push(' ');
                        i = comment_end;
                    }
                    continue;
                }
                out.push('<');
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Behavior switches recognized in `__WORD__` form. A matched switch is removed from the text and
/// recorded as a boolean metadata entry under its lowercased name; the comparison is case-sensitive,
/// so only the uppercase spelling is a switch.
const BEHAVIOR_SWITCHES: &[&str] = &[
    "ARCHIVEDTALK",
    "DISAMBIG",
    "EXPECTUNUSEDCATEGORY",
    "EXPECTUNUSEDTEMPLATE",
    "FORCETOC",
    "HIDDENCAT",
    "INDEX",
    "NEWSECTIONLINK",
    "NOCC",
    "NOCONTENTCONVERT",
    "NOEDITSECTION",
    "NOGALLERY",
    "NOGLOBAL",
    "NOINDEX",
    "NONEWSECTIONLINK",
    "NOTC",
    "NOTITLECONVERT",
    "NOTOC",
    "STATICREDIRECT",
    "TOC",
];

/// Removes every recognized `__WORD__` behavior switch from the text, returning the cleaned text and
/// the lowercased names of the switches found in document order. Switches inside verbatim regions
/// (`<nowiki>`, `<pre>`, …) are left untouched as literal text.
pub(super) fn extract_behavior_switches(input: &str) -> (String, Vec<String>) {
    let chars: Vec<char> = input.chars().collect();
    let n = chars.len();
    let bounds = ScanBounds::of(&chars);
    let mut out = String::new();
    let mut found: Vec<String> = Vec::new();
    let mut i = 0;
    while i < n {
        if at(&chars, i) == Some('<')
            && let Some(after) = verbatim_region_end(&chars, i, bounds)
        {
            out.push_str(&collect_range(&chars, i, after));
            i = after;
            continue;
        }
        if at(&chars, i) == Some('_')
            && at(&chars, i + 1) == Some('_')
            && let Some((word, after)) = behavior_switch_at(&chars, i)
        {
            let key = word.to_ascii_lowercase();
            if !found.contains(&key) {
                found.push(key);
            }
            i = after;
            // A line-leading switch is removed with its following spaces/tabs so the line gains no
            // leading space (would mark it preformatted); the line break itself stays.
            if out.is_empty() || out.ends_with('\n') {
                while matches!(at(&chars, i), Some(' ' | '\t')) {
                    i += 1;
                }
            }
            continue;
        }
        if let Some(c) = at(&chars, i) {
            out.push(c);
        }
        i += 1;
    }
    (out, found)
}

/// Reads a `__WORD__` behavior switch at `i`, returning the uppercase word and the index past it.
fn behavior_switch_at(chars: &[char], i: usize) -> Option<(String, usize)> {
    let start = i + 2;
    let mut j = start;
    while at(chars, j).is_some_and(|c| c.is_ascii_uppercase()) {
        j += 1;
    }
    let word = collect_range(chars, start, j);
    if word.is_empty()
        || at(chars, j) != Some('_')
        || at(chars, j + 1) != Some('_')
        || !BEHAVIOR_SWITCHES.contains(&word.as_str())
    {
        return None;
    }
    Some((word, j + 2))
}
