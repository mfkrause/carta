//! Bracketed links, images, bare autolinks, and the URL-scheme tables they recognise.

use carta_ast::{Attr, Inline, Target};

use super::inline::{find_within, inlines_with, non_space};
use super::shared::{is_space, matches_at, slice_to_string};

/// URL prefixes accepted as a bracketed link target. Schemes other than `mailto:` require `://`.
const LINK_URL_PREFIXES: &[&str] = &[
    "https://", "http://", "ftp://", "file://", "news://", "nntp://", "irc://", "mailto:",
];
/// URL prefixes accepted as a bare (unbracketed) autolink. `file://` is not autolinked.
const BARE_URL_PREFIXES: &[&str] = &[
    "https://", "http://", "ftp://", "news://", "nntp://", "irc://", "mailto:",
];

pub(super) fn parse_link(
    chars: &[char],
    i: usize,
    hi: usize,
    depth: usize,
    budget: &mut usize,
) -> Option<(Inline, usize)> {
    let close = find_within(chars, i + 1..hi, budget, |c| c == ']')?;
    let pipes: Vec<usize> = (i + 1..close)
        .filter(|&k| chars.get(k) == Some(&'|'))
        .collect();
    // A third `|`-segment is allowed only when it names a smart-link style, which becomes a class on
    // the link; any other third segment, or a fourth, is not a link.
    let (label_range, target_start, target_end, smart_class) = match pipes.as_slice() {
        [] => (None, i + 1, close, None),
        [p] => (Some((i + 1, *p)), p + 1, close, None),
        [p1, p2] => {
            let third = slice_to_string(chars, p2 + 1, close);
            if third != "smart-link" && third != "smart-card" {
                return None;
            }
            (Some((i + 1, *p1)), p1 + 1, *p2, Some(third))
        }
        _ => return None,
    };
    let has_pipe = label_range.is_some();
    let target = slice_to_string(chars, target_start, target_end);

    let (url, class, default_label) = classify_link_target(&target, has_pipe)?;

    let label = match label_range {
        Some((ls, le)) if le > ls => inlines_with(chars, ls, le, false, depth + 1),
        _ => vec![Inline::Str(default_label.into())],
    };
    let mut classes: Vec<String> = class.into_iter().map(str::to_string).collect();
    classes.extend(smart_class);
    let attr = Attr {
        id: carta_ast::Text::default(),
        classes: classes.into_iter().map(Into::into).collect(),
        attributes: Vec::new(),
    };
    Some((
        Inline::Link(
            Box::new(attr),
            label,
            Box::new(Target {
                url: url.into(),
                title: carta_ast::Text::default(),
            }),
        ),
        close + 1,
    ))
}

fn classify_link_target(
    target: &str,
    has_pipe: bool,
) -> Option<(String, Option<&'static str>, String)> {
    if target.starts_with('#') {
        return Some((target.to_string(), None, target.to_string()));
    }
    if target.starts_with('~') {
        return Some((target.to_string(), Some("user-account"), target.to_string()));
    }
    if let Some(rest) = target.strip_prefix('^') {
        if has_pipe {
            return None;
        }
        return Some((rest.to_string(), Some("attachment"), rest.to_string()));
    }
    if has_url_prefix(target, LINK_URL_PREFIXES) {
        let label = target
            .strip_prefix("mailto:")
            .map_or_else(|| target.to_string(), str::to_string);
        return Some((target.to_string(), None, label));
    }
    None
}

pub(super) fn parse_image(
    chars: &[char],
    i: usize,
    hi: usize,
    budget: &mut usize,
) -> Option<(Inline, usize)> {
    if !non_space(chars, i + 1) {
        return None;
    }
    let close = find_within(chars, i + 1..hi, budget, |c| c == '!')?;
    let content = slice_to_string(chars, i + 1, close);
    let (src, props) = match content.split_once('|') {
        Some((s, p)) => (s.to_string(), Some(p.to_string())),
        None => (content, None),
    };
    if src.is_empty() {
        return None;
    }

    let (attr, title) = match props {
        Some(props) => image_properties(&props)?,
        None => (Attr::default(), String::new()),
    };
    Some((
        Inline::Image(
            Box::new(attr),
            Vec::new(),
            Box::new(Target {
                url: src.into(),
                title: title.into(),
            }),
        ),
        close + 1,
    ))
}

/// Parses the property list that follows the `|` in an image, returning its attributes and title, or
/// `None` when the list is malformed (which disqualifies the image). Leading whitespace on the whole
/// list disqualifies it; `thumbnail` is accepted only as the sole property and only with no
/// surrounding whitespace. Otherwise every property is `key=value`: a key carries no whitespace and
/// loses only the whitespace introduced after a separating comma, while a value is kept verbatim so
/// its surrounding whitespace is preserved. A `title` property is the image's title rather than an
/// attribute.
fn image_properties(props: &str) -> Option<(Attr, String)> {
    if props.starts_with(is_space) {
        return None;
    }
    if props == "thumbnail" {
        return Some((
            Attr {
                id: carta_ast::Text::default(),
                classes: vec!["thumbnail".into()],
                attributes: Vec::new(),
            },
            String::new(),
        ));
    }
    let mut attributes = Vec::new();
    let mut title = String::new();
    for (idx, raw) in props.split(',').enumerate() {
        let part = if idx == 0 {
            raw
        } else {
            raw.trim_start_matches(is_space)
        };
        let (key, value) = part.split_once('=')?;
        if key.is_empty() || key.contains(is_space) {
            return None;
        }
        if key == "title" {
            title = value.to_string();
        } else {
            attributes.push((key.to_string(), value.to_string()));
        }
    }
    Some((
        Attr {
            id: carta_ast::Text::default(),
            classes: Vec::new(),
            attributes: attributes
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        },
        title,
    ))
}

/// If a bare autolink starts at `i`, returns the index just past its URL run. A scheme matches
/// only in lower case. The run extends to the first whitespace or URL terminator.
pub(super) fn match_bare_url(chars: &[char], i: usize, hi: usize) -> Option<usize> {
    if !BARE_URL_PREFIXES.iter().any(|p| matches_at(chars, i, p)) {
        return None;
    }
    let mut end = i;
    while end < hi
        && chars
            .get(end)
            .is_some_and(|&c| !is_space(c) && !is_url_terminator(c))
    {
        end += 1;
    }
    Some(end)
}

/// Characters that end a bare autolink run.
fn is_url_terminator(c: char) -> bool {
    matches!(c, '|' | ']' | '}' | '<' | '>' | '"' | '[' | '{' | '`')
}

/// Whether `s` begins with one of `prefixes`. A scheme matches only in lower case.
fn has_url_prefix(s: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|p| s.starts_with(p))
}
