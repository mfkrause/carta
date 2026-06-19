//! Bare-URI autolinking: a post-pass that turns plain URLs, `www.` hosts, and email addresses found
//! in ordinary text into `Link` inlines.
//!
//! A URL begins with a lowercase `http://`, `https://`, or `ftp://` scheme, or a `www.` host (which
//! is given an `http://` destination); both must sit at a non-alphanumeric boundary. The run extends
//! to the next space or `<`, with parentheses balanced and an unbalanced `)` or a `]` ending it;
//! trailing punctuation and a trailing entity reference are then dropped. A scheme URL's authority
//! must be a usable domain (at least two dot-separated labels, none ending in `-`/`_`); a `www.` host
//! is taken as-is. An email address is a local part of `[A-Za-z0-9._+-]` and an `@`, followed by a
//! dotted domain, and is given a `mailto:` destination. Links are not nested, so an existing `Link`
//! is left untouched rather than rescanned.

use carta_ast::{Attr, Inline, Target};

/// Rewrite every text `Str` in `inlines`, recursing through inline containers, so that bare URIs,
/// `www.` hosts, and email addresses become links. Code, math, and raw inlines are left untouched.
pub(crate) fn autolink_inlines(inlines: &mut Vec<Inline>) {
    let taken = std::mem::take(inlines);
    let mut out = Vec::with_capacity(taken.len());
    for inline in taken {
        match inline {
            Inline::Str(s) => split_text(&s, &mut out),
            Inline::Emph(mut v) => out.push(Inline::Emph(recurse(&mut v))),
            Inline::Underline(mut v) => out.push(Inline::Underline(recurse(&mut v))),
            Inline::Strong(mut v) => out.push(Inline::Strong(recurse(&mut v))),
            Inline::Strikeout(mut v) => out.push(Inline::Strikeout(recurse(&mut v))),
            Inline::Superscript(mut v) => out.push(Inline::Superscript(recurse(&mut v))),
            Inline::Subscript(mut v) => out.push(Inline::Subscript(recurse(&mut v))),
            Inline::SmallCaps(mut v) => out.push(Inline::SmallCaps(recurse(&mut v))),
            Inline::Quoted(q, mut v) => out.push(Inline::Quoted(q, recurse(&mut v))),
            Inline::Span(a, mut v) => out.push(Inline::Span(a, recurse(&mut v))),
            Inline::Image(a, mut v, t) => out.push(Inline::Image(a, recurse(&mut v), t)),
            other => out.push(other),
        }
    }
    *inlines = out;
}

fn recurse(inlines: &mut Vec<Inline>) -> Vec<Inline> {
    autolink_inlines(inlines);
    std::mem::take(inlines)
}

/// A matched autolink: the half-open `start..end` span within the text and the link destination.
struct Match {
    start: usize,
    end: usize,
    href: String,
}

/// Scan one text token, emitting `Str` for the gaps and `Link` for each autolink found.
fn split_text(text: &str, out: &mut Vec<Inline>) {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut emit_from = 0;
    while i < len {
        let found = match_url(&chars, i)
            .or_else(|| match_www(&chars, i))
            .or_else(|| match_email(&chars, i, emit_from));
        if let Some(m) = found {
            push_text(&chars, emit_from, m.start, out);
            if let Some(span) = chars.get(m.start..m.end) {
                let label: String = span.iter().collect();
                out.push(Inline::Link(
                    Attr::default(),
                    vec![Inline::Str(label)],
                    Target {
                        url: m.href,
                        title: String::new(),
                    },
                ));
            }
            emit_from = m.end;
            i = m.end;
        } else {
            i += 1;
        }
    }
    push_text(&chars, emit_from, len, out);
}

fn push_text(chars: &[char], a: usize, b: usize, out: &mut Vec<Inline>) {
    if let Some(slice) = chars.get(a..b)
        && !slice.is_empty()
    {
        out.push(Inline::Str(slice.iter().collect()));
    }
}

fn match_url(chars: &[char], i: usize) -> Option<Match> {
    if alnum_before(chars, i) {
        return None;
    }
    let scheme_len = url_scheme_len(chars, i)?;
    let content_start = i + scheme_len;
    let scan_end = forward_scan(chars, i);
    if !valid_host(chars.get(content_start..scan_end)?) {
        return None;
    }
    let end = trim_trailing(chars, content_start, scan_end);
    if end <= content_start {
        return None;
    }
    let href: String = chars.get(i..end)?.iter().collect();
    Some(Match {
        start: i,
        end,
        href,
    })
}

/// Whether `rest` opens with a usable domain. Starting at the first character, labels of
/// alphanumerics, `-`, and `_` are read greedily and joined by single dots; the domain is the longest
/// such prefix whose labels are non-empty and end in neither `-` nor `_`. It is usable when that
/// prefix holds at least two labels. Whatever follows the domain (port, path, an extra dot) is not
/// examined here — it stays part of the link.
fn valid_host(rest: &[char]) -> bool {
    let mut labels = 0;
    let mut i = 0;
    loop {
        let start = i;
        while rest.get(i).is_some_and(|&c| is_label_char(c)) {
            i += 1;
        }
        if i == start || matches!(rest.get(i - 1), Some('-' | '_')) {
            break;
        }
        labels += 1;
        if rest.get(i) != Some(&'.') {
            break;
        }
        i += 1;
    }
    labels >= 2
}

fn is_label_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_')
}

fn match_www(chars: &[char], i: usize) -> Option<Match> {
    if alnum_before(chars, i) || !starts_with(chars, i, "www.") {
        return None;
    }
    let content_start = i + 4;
    let scan_end = forward_scan(chars, i);
    if !valid_host(chars.get(i..scan_end)?) {
        return None;
    }
    let end = trim_trailing(chars, content_start, scan_end);
    if end <= content_start {
        return None;
    }
    let label: String = chars.get(i..end)?.iter().collect();
    let href = format!("http://{label}");
    Some(Match {
        start: i,
        end,
        href,
    })
}

/// Match an email address centered on the `@` at `at`. The local part extends left over
/// `[A-Za-z0-9._+-]` (but no earlier than `lower`, the start of the not-yet-emitted text), and the
/// domain extends right over `[A-Za-z0-9._-]` and must hold at least one non-empty dotted label and
/// end on an alphanumeric.
fn match_email(chars: &[char], at: usize, lower: usize) -> Option<Match> {
    if chars.get(at) != Some(&'@') {
        return None;
    }
    let mut start = at;
    while start > lower {
        match chars.get(start - 1) {
            Some(&c) if is_local_char(c) => start -= 1,
            _ => break,
        }
    }
    if start == at {
        return None;
    }
    let mut end = at + 1;
    while chars.get(end).is_some_and(|&c| is_domain_char(c)) {
        end += 1;
    }
    while end > at + 1 && chars.get(end - 1) == Some(&'.') {
        end -= 1;
    }
    let domain = chars.get(at + 1..end)?;
    let ends_alnum = domain.last().is_some_and(char::is_ascii_alphanumeric);
    if !domain.contains(&'.') || !ends_alnum || domain.windows(2).any(|w| matches!(w, ['.', '.'])) {
        return None;
    }
    let label: String = chars.get(start..end)?.iter().collect();
    let href = format!("mailto:{label}");
    Some(Match { start, end, href })
}

/// Walk the URL run forward from `from`, stopping at whitespace or `<`, at an unbalanced `)`, or at a
/// `]` outside any parenthesis. Returns the index just past the last character of the run.
fn forward_scan(chars: &[char], from: usize) -> usize {
    let mut depth: i32 = 0;
    let mut j = from;
    while let Some(&c) = chars.get(j) {
        if c.is_whitespace() || c == '<' {
            break;
        }
        match c {
            '(' => depth += 1,
            ')' | ']' if depth == 0 => break,
            ')' => depth -= 1,
            _ => {}
        }
        j += 1;
    }
    j
}

/// Drop trailing punctuation from a URL run, never trimming below `min` (the start of the host).
/// A trailing `;` takes its preceding `&entity;` with it when one is present.
fn trim_trailing(chars: &[char], min: usize, mut end: usize) -> usize {
    while end > min {
        match chars.get(end - 1) {
            Some('!' | '"' | '\'' | '*' | ',' | '.' | ':' | '?' | '_' | '~') => end -= 1,
            Some(';') => {
                let mut j = end - 1;
                while j > min && chars.get(j - 1).is_some_and(|&c| is_entity_char(c)) {
                    j -= 1;
                }
                end = if j > min && chars.get(j - 1) == Some(&'&') {
                    j - 1
                } else {
                    end - 1
                };
            }
            _ => break,
        }
    }
    end
}

fn url_scheme_len(chars: &[char], i: usize) -> Option<usize> {
    ["https://", "http://", "ftp://"]
        .into_iter()
        .find(|scheme| starts_with(chars, i, scheme))
        .map(str::len)
}

fn starts_with(chars: &[char], i: usize, pat: &str) -> bool {
    pat.chars()
        .enumerate()
        .all(|(offset, pc)| chars.get(i + offset) == Some(&pc))
}

fn alnum_before(chars: &[char], i: usize) -> bool {
    i.checked_sub(1)
        .and_then(|p| chars.get(p))
        .is_some_and(|c| c.is_alphanumeric())
}

fn is_local_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-')
}

fn is_domain_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
}

fn is_entity_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '#'
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Autolink a single text token and report each produced link as `(label, href)`.
    fn links(text: &str) -> Vec<(String, String)> {
        let mut inlines = vec![Inline::Str(text.to_owned())];
        autolink_inlines(&mut inlines);
        inlines
            .iter()
            .filter_map(|inline| match inline {
                Inline::Link(_, label, target) => {
                    let text: String = label
                        .iter()
                        .map(|i| match i {
                            Inline::Str(s) => s.as_str(),
                            _ => "",
                        })
                        .collect();
                    Some((text, target.url.clone()))
                }
                _ => None,
            })
            .collect()
    }

    fn host(s: &str) -> bool {
        valid_host(&s.chars().collect::<Vec<_>>())
    }

    fn scan(s: &str) -> usize {
        forward_scan(&s.chars().collect::<Vec<_>>(), 0)
    }

    #[test]
    fn valid_host_needs_two_well_formed_labels() {
        assert!(host("example.com"));
        assert!(host("a.b.c.com"));
        assert!(host("-a.com")); // a leading hyphen is fine
        assert!(host("ex_ample.com")); // an interior underscore is fine
        assert!(!host("localhost")); // a single label is not a domain
        assert!(!host("a-.com")); // a label may not end in '-'
        assert!(!host("example.com_")); // ...nor in '_'
        assert!(!host(".com")); // an empty leading label
        assert!(!host("a..com")); // an empty interior label cuts the prefix to one label
    }

    #[test]
    fn valid_host_reads_only_the_leading_domain() {
        // The domain is a prefix; trailing junk past two good labels does not invalidate it.
        assert!(host("a.com..post"));
        assert!(host("example.com:8080/path"));
        assert!(host("example.com./x")); // a trailing dot then path
    }

    #[test]
    fn forward_scan_balances_parens_and_stops_at_boundaries() {
        assert_eq!(scan("http://e.com/a b"), 14); // stops at the space
        assert_eq!(scan("http://e.com/a(b)c)"), 18); // closes the balanced pair, stops at the loose ')'
        assert_eq!(scan("http://e.com]x"), 12); // a ']' at depth zero ends the run
        assert_eq!(scan("http://e.com<x"), 12); // so does a '<'
    }

    #[test]
    fn trim_trailing_drops_punctuation_and_entities() {
        let chars: Vec<char> = "http://e.com/p.,".chars().collect();
        assert_eq!(trim_trailing(&chars, 7, chars.len()), 14); // drops the trailing '.' and ','
        let ent: Vec<char> = "http://e.com/p&amp;".chars().collect();
        assert_eq!(trim_trailing(&ent, 7, ent.len()), 14); // a trailing '&entity;' goes whole
    }

    #[test]
    fn bare_url_www_and_email_become_links() {
        assert_eq!(
            links("see http://example.com/p?q=1 now"),
            vec![(
                "http://example.com/p?q=1".to_owned(),
                "http://example.com/p?q=1".to_owned()
            )]
        );
        assert_eq!(
            links("at www.example.com today"),
            vec![(
                "www.example.com".to_owned(),
                "http://www.example.com".to_owned()
            )]
        );
        assert_eq!(
            links("mail me@example.com please"),
            vec![(
                "me@example.com".to_owned(),
                "mailto:me@example.com".to_owned()
            )]
        );
    }

    #[test]
    fn trailing_sentence_punctuation_is_excluded() {
        assert_eq!(
            links("read http://example.com.")
                .first()
                .map(|l| l.1.clone()),
            Some("http://example.com".to_owned())
        );
    }

    #[test]
    fn invalid_domains_do_not_link() {
        assert!(links("ping http://localhost/here").is_empty());
        assert!(links("ping http://a..b.com here").is_empty());
    }

    #[test]
    fn existing_links_are_not_rescanned() {
        // A link is never nested inside another link: a pre-formed Link is passed through verbatim.
        let inner = Inline::Link(
            Attr::default(),
            vec![Inline::Str("http://example.com".to_owned())],
            Target {
                url: "http://example.com".to_owned(),
                title: String::new(),
            },
        );
        let mut inlines = vec![inner.clone()];
        autolink_inlines(&mut inlines);
        assert_eq!(inlines, vec![inner]);
    }

    #[test]
    fn code_is_left_untouched() {
        let code = Inline::Code(Attr::default(), "http://example.com".to_owned());
        let mut inlines = vec![code.clone()];
        autolink_inlines(&mut inlines);
        assert_eq!(inlines, vec![code]);
    }
}
