//! Links, media, footnotes, verbatim spans, angle-bracket constructs, and page macros.

use carta_ast::{Attr, Format, Inline, Target};

use super::Ctx;
use super::blocks::parse_blocks_str;
use super::helpers::{find_subsequence, is_blank, matches_at};
use super::inline::scan_slice;

/// Split text into `Str` words separated by single whitespace tokens, with no markup interpretation.
fn tokenize_text(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut word = String::new();
    for c in text.chars() {
        if c.is_whitespace() {
            if !word.is_empty() {
                out.push(Inline::Str(std::mem::take(&mut word).into()));
            }
            let token = if c == '\n' {
                Inline::SoftBreak
            } else {
                Inline::Space
            };
            if !matches!(out.last(), Some(Inline::Space | Inline::SoftBreak)) {
                out.push(token);
            }
        } else {
            word.push(c);
        }
    }
    if !word.is_empty() {
        out.push(Inline::Str(word.into()));
    }
    out
}

/// Parse a `[[target|label]]` link, returning the link node and its end index. A bracket pair whose
/// target side (the text before the first `|`) is entirely empty is not a link; the opener stays
/// literal.
pub(super) fn parse_link(chars: &[char], start: usize) -> Option<(Inline, usize)> {
    let close = find_subsequence(chars, start + 2, "]]")?;
    let inner: String = chars.get(start + 2..close).unwrap_or(&[]).iter().collect();
    let (raw_target, label) = match inner.split_once('|') {
        Some((t, l)) => (t, Some(l.to_string())),
        None => (inner.as_str(), None),
    };
    if raw_target.is_empty() {
        return None;
    }
    let target = raw_target.trim().to_string();
    let (url, display) = classify_link_target(&target);
    // An explicit but empty label falls back to the target's auto-display text.
    let label_inlines = match label {
        Some(text) if !text.trim().is_empty() => tokenize_text(text.trim()),
        _ => vec![Inline::Str(display.into())],
    };
    Some((
        Inline::Link(
            Box::default(),
            label_inlines,
            Box::new(Target {
                url: url.into(),
                title: carta_ast::Text::default(),
            }),
        ),
        close + 2,
    ))
}

/// Resolve a link target to its destination URL and auto-display text.
fn classify_link_target(target: &str) -> (String, String) {
    if target.starts_with("\\\\") || is_external(target) {
        (target.to_string(), target.to_string())
    } else if let Some((prefix, rest)) = target.split_once('>') {
        (interwiki_url(prefix, rest), rest.to_string())
    } else {
        (resolve_id(target), display_id(target))
    }
}

/// Parse a `{{image?query|caption}}` media reference into an image, or, when the query opts out of
/// embedding, a link.
pub(super) fn parse_media(chars: &[char], start: usize) -> Option<(Inline, usize)> {
    let close = find_subsequence(chars, start + 2, "}}")?;
    let inner: String = chars.get(start + 2..close).unwrap_or(&[]).iter().collect();
    let end = close + 2;

    let leading_space = inner.starts_with(char::is_whitespace);
    let (spec, caption) = match inner.split_once('|') {
        Some((s, c)) => (s, Some(c)),
        None => (inner.as_str(), None),
    };
    // A brace pair whose source side (before the first `|`) is empty is not a media reference.
    if spec.is_empty() {
        return None;
    }
    let trailing_space = spec.ends_with(char::is_whitespace);
    let mut classes = Vec::new();
    if let Some(class) = media_align(leading_space, trailing_space) {
        classes.push(class.into());
    }

    let spec = spec.trim();
    let (id, query) = match spec.split_once('?') {
        Some((i, q)) => (i, Some(q)),
        None => (spec, None),
    };
    let url = if is_external(id) {
        id.to_string()
    } else {
        resolve_id(id)
    };
    // An explicit but empty caption falls back to the source's auto-display text.
    let alt = match caption {
        Some(text) if !text.trim().is_empty() => tokenize_text(text.trim()),
        _ if is_external(id) => vec![Inline::Str(id.into())],
        _ => vec![Inline::Str(display_id(id).into())],
    };
    let target = Target {
        url: url.into(),
        title: carta_ast::Text::default(),
    };

    let node = match query {
        Some(q) if q.contains("linkonly") => Inline::Link(
            Box::new(Attr {
                classes,
                ..Default::default()
            }),
            alt,
            Box::new(target),
        ),
        Some(q) => {
            let (width, height) = parse_size(q);
            let mut attributes = Vec::new();
            if let Some(w) = width {
                attributes.push(("width".to_string(), w));
            }
            if let Some(h) = height {
                attributes.push(("height".to_string(), h));
            }
            attributes.push(("query".to_string(), format!("?{q}")));
            Inline::Image(
                Box::new(Attr {
                    classes,
                    attributes: attributes
                        .into_iter()
                        .map(|(k, v)| (k.into(), v.into()))
                        .collect(),
                    ..Default::default()
                }),
                alt,
                Box::new(target),
            )
        }
        None => Inline::Image(
            Box::new(Attr {
                classes,
                ..Default::default()
            }),
            alt,
            Box::new(target),
        ),
    };
    Some((node, end))
}

/// The alignment class for a media reference, from whether its braces carry interior padding.
fn media_align(leading: bool, trailing: bool) -> Option<&'static str> {
    match (leading, trailing) {
        (true, true) => Some("align-center"),
        (false, true) => Some("align-left"),
        (true, false) => Some("align-right"),
        (false, false) => None,
    }
}

/// Parse the leading `width` and optional `xheight` of a media query into pixel strings.
fn parse_size(query: &str) -> (Option<String>, Option<String>) {
    let chars: Vec<char> = query.chars().collect();
    let mut i = 0;
    let mut width = String::new();
    while let Some(&c) = chars.get(i) {
        if c.is_ascii_digit() {
            width.push(c);
            i += 1;
        } else {
            break;
        }
    }
    if width.is_empty() {
        return (None, None);
    }
    let mut height = String::new();
    if matches!(chars.get(i), Some('x' | 'X')) {
        i += 1;
        while let Some(&c) = chars.get(i) {
            if c.is_ascii_digit() {
                height.push(c);
                i += 1;
            } else {
                break;
            }
        }
    }
    let height = if height.is_empty() {
        None
    } else {
        Some(height)
    };
    (Some(width), height)
}

/// Whether a target names an external destination: a known scheme followed by `://`.
fn is_external(s: &str) -> bool {
    match s.find("://") {
        Some(idx) => {
            let scheme = s.get(..idx).unwrap_or("");
            !scheme.is_empty()
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
                && crate::url_schemes::is_scheme(&scheme.to_lowercase())
        }
        None => false,
    }
}

/// Resolve a page identifier to a site-relative URL. A namespaced id becomes a slash path, rooted
/// unless it is relative (a leading `.`); an id with no namespace is left untouched.
fn resolve_id(id: &str) -> String {
    if !id.contains(':') {
        return id.to_string();
    }
    if let Some(rest) = id.strip_prefix('.') {
        return rest.trim_start_matches('.').replace(':', "/");
    }
    let replaced = id.replace(':', "/");
    if replaced.starts_with('/') {
        replaced
    } else {
        format!("/{replaced}")
    }
}

/// The display text for a bare page identifier: the segment after the last namespace separator.
fn display_id(id: &str) -> String {
    match id.rsplit_once(':') {
        Some((_, last)) => last.to_string(),
        None => id.to_string(),
    }
}

/// Map an interwiki shortcut and its tail to a destination URL.
fn interwiki_url(prefix: &str, rest: &str) -> String {
    match prefix {
        "wp" => format!("https://en.wikipedia.org/wiki/{rest}"),
        "wpfr" => format!("https://fr.wikipedia.org/wiki/{rest}"),
        "wpde" => format!("https://de.wikipedia.org/wiki/{rest}"),
        "wpes" => format!("https://es.wikipedia.org/wiki/{rest}"),
        "wpjp" => format!("https://jp.wikipedia.org/wiki/{rest}"),
        "wppl" => format!("https://pl.wikipedia.org/wiki/{rest}"),
        "doku" => format!("https://www.dokuwiki.org/{rest}"),
        "phpfn" => format!("https://secure.php.net/{rest}"),
        "callto" => format!("callto://{rest}"),
        other => format!("{other}>{rest}"),
    }
}

/// Parse a `((…))` footnote into a note holding the block content of its body. A body that is empty
/// or only whitespace is not a footnote, so the opener stays literal.
pub(super) fn parse_footnote(
    chars: &[char],
    begin: usize,
    ctx: Ctx,
    depth: usize,
) -> Option<(Inline, usize)> {
    let close = find_subsequence(chars, begin + 2, "))")?;
    let inner: String = chars.get(begin + 2..close).unwrap_or(&[]).iter().collect();
    if inner.trim().is_empty() {
        return None;
    }
    Some((
        Inline::Note(parse_blocks_str(&inner, ctx, depth + 1)),
        close + 2,
    ))
}

/// Parse a `%%…%%` no-formatting span: its content is taken verbatim as text. Like the emphasis
/// markers, the opener needs a non-whitespace character after it and the closer one before it, so a
/// `%%` adjacent to a space stays literal.
pub(super) fn parse_nowiki_pct(chars: &[char], begin: usize) -> Option<(Vec<Inline>, usize)> {
    if chars.get(begin + 2).is_none_or(|c| c.is_whitespace()) {
        return None;
    }
    let mut j = begin + 2;
    while j < chars.len() {
        if chars.get(j) == Some(&'%')
            && chars.get(j + 1) == Some(&'%')
            && j > begin + 2
            && chars.get(j - 1).is_some_and(|c| !c.is_whitespace())
        {
            let inner: String = chars.get(begin + 2..j).unwrap_or(&[]).iter().collect();
            return Some((tokenize_text(&inner), j + 2));
        }
        j += 1;
    }
    None
}

/// Parse an angle-bracket inline construct: the markup spans, a verbatim span, raw HTML/PHP, or an
/// email address.
pub(super) fn parse_angle(
    chars: &[char],
    begin: usize,
    ctx: Ctx,
    depth: usize,
) -> Option<(Vec<Inline>, usize)> {
    // A span tag with a blank interior is not markup; the opener stays literal text.
    if let Some((inner, end)) = tag_region(chars, begin, "<sub>", "</sub>")
        && !is_blank(&inner)
    {
        return Some((
            vec![Inline::Subscript(scan_slice(&inner, ctx, depth + 1))],
            end,
        ));
    }
    if let Some((inner, end)) = tag_region(chars, begin, "<sup>", "</sup>")
        && !is_blank(&inner)
    {
        return Some((
            vec![Inline::Superscript(scan_slice(&inner, ctx, depth + 1))],
            end,
        ));
    }
    if let Some((inner, end)) = tag_region(chars, begin, "<del>", "</del>")
        && !is_blank(&inner)
    {
        return Some((
            vec![Inline::Strikeout(scan_slice(&inner, ctx, depth + 1))],
            end,
        ));
    }
    if let Some((inner, end)) = tag_region(chars, begin, "<nowiki>", "</nowiki>") {
        let text: String = inner.iter().collect();
        return Some((tokenize_text(&text), end));
    }
    if let Some((inner, end)) = tag_region(chars, begin, "<html>", "</html>") {
        let text: String = inner.iter().collect();
        return Some((
            vec![Inline::RawInline(Format("html".into()), text.into())],
            end,
        ));
    }
    if let Some((inner, end)) = tag_region(chars, begin, "<php>", "</php>") {
        let text: String = inner.iter().collect();
        return Some((
            vec![Inline::RawInline(
                Format("html".into()),
                format!("<?php {text} ?>").into(),
            )],
            end,
        ));
    }
    angle_email(chars, begin).map(|(node, end)| (vec![node], end))
}

/// The interior characters and end index of an `open…close` tag region starting at `start`.
fn tag_region(chars: &[char], start: usize, open: &str, close: &str) -> Option<(Vec<char>, usize)> {
    if !matches_at(chars, start, open) {
        return None;
    }
    let content_start = start + open.chars().count();
    let close_at = find_subsequence(chars, content_start, close)?;
    let inner = chars.get(content_start..close_at).unwrap_or(&[]).to_vec();
    Some((inner, close_at + close.chars().count()))
}

/// Parse `<local@domain>` into a `mailto:` link.
fn angle_email(chars: &[char], start: usize) -> Option<(Inline, usize)> {
    if chars.get(start) != Some(&'<') {
        return None;
    }
    let mut j = start + 1;
    while let Some(&c) = chars.get(j) {
        if c == '>' {
            break;
        }
        if c.is_whitespace() || c == '<' {
            return None;
        }
        j += 1;
    }
    if chars.get(j) != Some(&'>') {
        return None;
    }
    let inner: String = chars.get(start + 1..j).unwrap_or(&[]).iter().collect();
    let (local, domain) = inner.split_once('@')?;
    if local.is_empty() || !domain.contains('.') || domain.starts_with('.') || domain.ends_with('.')
    {
        return None;
    }
    let url = format!("mailto:{inner}");
    Some((
        Inline::Link(
            Box::default(),
            vec![Inline::Str(inner.into())],
            Box::new(Target {
                url: url.into(),
                title: carta_ast::Text::default(),
            }),
        ),
        j + 1,
    ))
}

/// Recognise a dropped page macro (`~~NOTOC~~`, `~~NOCACHE~~`), returning its end index.
pub(super) fn parse_macro(chars: &[char], start: usize) -> Option<usize> {
    for token in ["~~NOTOC~~", "~~NOCACHE~~"] {
        if matches_at(chars, start, token) {
            return Some(start + token.chars().count());
        }
    }
    None
}
