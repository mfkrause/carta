//! Small builders and predicates shared by the inline scanner.

use carta_ast::{Attr, Inline};

pub(super) fn collect_str(chars: &[char]) -> String {
    chars.iter().collect()
}

/// Tokenizes text into `Str` words separated by `Space`, used for the literal fallback rendering of a
/// citation.
pub(super) fn plain_words(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(Inline::Space);
        }
        out.push(Inline::Str(word.into()));
    }
    out
}

pub(super) fn wrap_markup(marker: char, content: Vec<Inline>) -> Inline {
    match marker {
        '*' => Inline::Strong(content),
        '+' => Inline::Strikeout(content),
        // The only other marker routed here is `/`.
        _ => Inline::Emph(content),
    }
}

pub(super) fn verbatim_code(marker: char, inner: &[char]) -> Inline {
    // A newline inside verbatim collapses to a space.
    let text: String = inner
        .iter()
        .map(|&c| if c == '\n' { ' ' } else { c })
        .collect();
    let attr = if marker == '=' {
        Attr {
            classes: vec!["verbatim".into()],
            ..Attr::default()
        }
    } else {
        Attr::default()
    };
    Inline::Code(Box::new(attr), text.into())
}

pub(super) fn link(target: &str, desc: Vec<Inline>) -> Inline {
    Inline::Link(
        Box::default(),
        desc,
        Box::new(carta_ast::Target {
            url: target.into(),
            title: carta_ast::Text::default(),
        }),
    )
}

pub(super) fn image(target: &str, alt: Vec<Inline>) -> Inline {
    Inline::Image(
        Box::default(),
        alt,
        Box::new(carta_ast::Target {
            url: target.into(),
            title: carta_ast::Text::default(),
        }),
    )
}

/// Processes a link target: strips a `file:` prefix and leaves other targets untouched.
pub(super) fn process_target(raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix("file:") {
        return rest.to_owned();
    }
    raw.to_owned()
}

pub(super) fn is_image_target(target: &str) -> bool {
    const EXTS: [&str; 8] = [
        ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".bmp", ".tiff",
    ];
    let lower = target.to_ascii_lowercase();
    EXTS.iter().any(|e| lower.ends_with(e))
}

/// Whether an angle-bracketed string is a URI: it carries a scheme and no whitespace.
pub(super) fn is_uri(s: &str) -> bool {
    if s.chars().any(char::is_whitespace) {
        return false;
    }
    if s.contains("://") {
        return true;
    }
    match s.split_once(':') {
        Some((scheme, rest)) => {
            !scheme.is_empty()
                && !rest.is_empty()
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
        }
        None => false,
    }
}

pub(super) fn is_url_boundary(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => !c.is_alphanumeric(),
    }
}

pub(super) fn pre_ok(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => c.is_whitespace() || matches!(c, '-' | '(' | '{' | '\'' | '"'),
    }
}

pub(super) fn post_ok(next: Option<char>) -> bool {
    match next {
        None => true,
        Some(c) => {
            c.is_whitespace()
                || matches!(
                    c,
                    '-' | '.' | ',' | ':' | '!' | '?' | ';' | '"' | '\'' | ')' | '}' | '['
                )
        }
    }
}
