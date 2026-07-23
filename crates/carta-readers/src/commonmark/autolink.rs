//! Bare-URI autolinking: a post-pass that turns plain URLs, `www.` hosts, and email addresses found
//! in ordinary text into `Link` inlines.
//!
//! A URL begins with a lowercase `http://`, `https://`, or `ftp://` scheme, or a `www.` host (which
//! is given an `http://` destination); both must sit at a non-alphanumeric boundary. The run extends
//! to the next space or `<`, with parentheses balanced and an unbalanced `)` or a `]` ending it;
//! trailing punctuation and a trailing entity reference are then dropped. A scheme URL's authority
//! must be a usable domain (at least two dot-separated labels, none ending in `-`/`_`); a `www.` host
//! is taken as-is. An email address is a local part of `[A-Za-z0-9._+-]` and an `@`, followed by a
//! domain, and is given a `mailto:` destination. Links are not nested, so an existing `Link` is left
//! untouched rather than rescanned.
//!
//! In the Markdown dialect each produced link carries a `uri` or `email` class, an email's domain
//! need only be a single non-empty label, and bare `www.` hosts are not linked; the strict dialect
//! leaves the link unclassed, requires a dotted email domain, and links `www.` hosts.

use carta_ast::{Attr, Inline, Target};

use super::scan::char_at;

/// Whether a matched autolink is a URL or an email address; selects its dialect class.
#[derive(Clone, Copy)]
enum Kind {
    Uri,
    Email,
}

impl Kind {
    fn class(self) -> &'static str {
        match self {
            Self::Uri => "uri",
            Self::Email => "email",
        }
    }
}

/// Rewrite every text `Str` in `inlines`, recursing through inline containers, so that bare URIs,
/// `www.` hosts, and email addresses become links. Code, math, and raw inlines are left untouched.
/// In the Markdown dialect (`markdown`) each link is classed and an email domain may be a single
/// label.
pub(crate) fn autolink_inlines(inlines: &mut Vec<Inline>, markdown: bool) {
    let taken = std::mem::take(inlines);
    let mut out = Vec::with_capacity(taken.len());
    for inline in taken {
        match inline {
            // Every match requires one of these literal substrings: a scheme URL needs `://` (the
            // scheme set is exactly `https://`/`http://`/`ftp://`), a `www.` host needs `www.`, and
            // an email needs `@`. A token containing none of them cannot autolink, so it passes
            // through unscanned. If a new scheme without `//` is ever added, extend this trigger set.
            Inline::Str(s) => {
                if s.contains("://") || s.contains('@') || s.contains("www.") {
                    split_text(&s, markdown, &mut out);
                } else {
                    out.push(Inline::Str(s));
                }
            }
            Inline::Emph(mut v) => out.push(Inline::Emph(recurse(&mut v, markdown))),
            Inline::Underline(mut v) => out.push(Inline::Underline(recurse(&mut v, markdown))),
            Inline::Strong(mut v) => out.push(Inline::Strong(recurse(&mut v, markdown))),
            Inline::Strikeout(mut v) => out.push(Inline::Strikeout(recurse(&mut v, markdown))),
            Inline::Superscript(mut v) => out.push(Inline::Superscript(recurse(&mut v, markdown))),
            Inline::Subscript(mut v) => out.push(Inline::Subscript(recurse(&mut v, markdown))),
            Inline::SmallCaps(mut v) => out.push(Inline::SmallCaps(recurse(&mut v, markdown))),
            Inline::Quoted(q, mut v) => out.push(Inline::Quoted(q, recurse(&mut v, markdown))),
            Inline::Span(a, mut v) => out.push(Inline::Span(a, recurse(&mut v, markdown))),
            Inline::Image(a, mut v, t) => out.push(Inline::Image(a, recurse(&mut v, markdown), t)),
            other => out.push(other),
        }
    }
    *inlines = out;
}

fn recurse(inlines: &mut Vec<Inline>, markdown: bool) -> Vec<Inline> {
    autolink_inlines(inlines, markdown);
    std::mem::take(inlines)
}

/// A matched autolink: the half-open `start..end` span within the text, the link destination, and
/// whether it is a URL or an email.
struct Match {
    start: usize,
    end: usize,
    href: String,
    kind: Kind,
}

/// Scan one text token, emitting `Str` for the gaps and `Link` for each autolink found.
fn split_text(text: &str, markdown: bool, out: &mut Vec<Inline>) {
    let len = text.len();
    let mut i = 0;
    let mut emit_from = 0;
    while i < len {
        // A bare `www.` host (no scheme) autolinks only in the strict dialect; the Markdown dialect
        // links scheme URLs and emails alone.
        let found = match_url(text, i)
            .or_else(|| (!markdown).then(|| match_www(text, i)).flatten())
            .or_else(|| match_email(text, i, emit_from, markdown));
        if let Some(m) = found {
            push_text(text, emit_from, m.start, out);
            if let Some(span) = text.get(m.start..m.end) {
                let attr = if markdown {
                    Attr {
                        id: carta_ast::Text::default(),
                        classes: vec![m.kind.class().into()],
                        attributes: Vec::new(),
                    }
                } else {
                    Attr::default()
                };
                // The markdown dialect percent-encodes the destination's unsafe characters; the
                // GitHub dialect keeps the matched text verbatim.
                let url = if markdown {
                    super::scan::escape_uri(&m.href)
                } else {
                    m.href
                };
                out.push(Inline::Link(
                    Box::new(attr),
                    vec![Inline::Str(span.into())],
                    Box::new(Target {
                        url: url.into(),
                        title: carta_ast::Text::default(),
                    }),
                ));
            }
            emit_from = m.end;
            i = m.end;
        } else {
            i += char_at(text, i).map_or(1, char::len_utf8);
        }
    }
    push_text(text, emit_from, len, out);
}

/// The character ending just before byte offset `at`, or `None` at the start of `text`.
fn char_before(text: &str, at: usize) -> Option<char> {
    text.get(..at).and_then(|head| head.chars().next_back())
}

fn push_text(text: &str, a: usize, b: usize, out: &mut Vec<Inline>) {
    if let Some(slice) = text.get(a..b)
        && !slice.is_empty()
    {
        out.push(Inline::Str(slice.into()));
    }
}

fn match_url(text: &str, i: usize) -> Option<Match> {
    if alnum_before(text, i) {
        return None;
    }
    let scheme_len = url_scheme_len(text, i)?;
    let content_start = i + scheme_len;
    let scan_end = forward_scan(text, i);
    if !valid_host(text.get(content_start..scan_end)?) {
        return None;
    }
    let end = trim_trailing(text, content_start, scan_end);
    if end <= content_start {
        return None;
    }
    let href = text.get(i..end)?.to_owned();
    Some(Match {
        start: i,
        end,
        href,
        kind: Kind::Uri,
    })
}

/// Whether `rest` opens with a usable domain. Starting at the first character, labels of
/// alphanumerics, `-`, and `_` are read greedily and joined by single dots; the domain is the longest
/// such prefix whose labels are non-empty and end in neither `-` nor `_`. It is usable when that
/// prefix holds at least two labels. Whatever follows the domain (port, path, an extra dot) is not
/// examined here — it stays part of the link.
fn valid_host(rest: &str) -> bool {
    let mut labels = 0;
    let mut i = 0;
    loop {
        let start = i;
        while char_at(rest, i).is_some_and(is_label_char) {
            i += char_at(rest, i).map_or(1, char::len_utf8);
        }
        if i == start || matches!(char_before(rest, i), Some('-' | '_')) {
            break;
        }
        labels += 1;
        if char_at(rest, i) != Some('.') {
            break;
        }
        i += 1;
    }
    labels >= 2
}

fn is_label_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_')
}

fn match_www(text: &str, i: usize) -> Option<Match> {
    if alnum_before(text, i) || !text.get(i..).is_some_and(|rest| rest.starts_with("www.")) {
        return None;
    }
    let content_start = i + 4;
    let scan_end = forward_scan(text, i);
    if !valid_host(text.get(i..scan_end)?) {
        return None;
    }
    let end = trim_trailing(text, content_start, scan_end);
    if end <= content_start {
        return None;
    }
    let label = text.get(i..end)?;
    let href = format!("http://{label}");
    Some(Match {
        start: i,
        end,
        href,
        kind: Kind::Uri,
    })
}

/// Match an email address centered on the `@` at `at`. The local part extends left over
/// `[A-Za-z0-9._+-]` (but no earlier than `lower`, the start of the not-yet-emitted text), and the
/// domain extends right over `[A-Za-z0-9._-]` and must end on an alphanumeric with no empty label.
/// The strict dialect additionally requires the domain to be dotted; the Markdown dialect accepts a
/// single-label domain such as `5@home`.
fn match_email(text: &str, at: usize, lower: usize, markdown: bool) -> Option<Match> {
    if char_at(text, at) != Some('@') {
        return None;
    }
    let mut start = at;
    while start > lower {
        match char_before(text, start) {
            Some(c) if is_local_char(c) => start -= c.len_utf8(),
            _ => break,
        }
    }
    if start == at {
        return None;
    }
    let mut end = at + 1;
    while char_at(text, end).is_some_and(is_domain_char) {
        end += char_at(text, end).map_or(1, char::len_utf8);
    }
    while end > at + 1 && char_before(text, end) == Some('.') {
        end -= 1;
    }
    let domain = text.get(at + 1..end)?;
    let ends_alnum = domain
        .chars()
        .next_back()
        .is_some_and(|c| c.is_ascii_alphanumeric());
    let dotted_ok = markdown || domain.contains('.');
    if !dotted_ok || !ends_alnum || domain.contains("..") {
        return None;
    }
    let label = text.get(start..end)?.to_owned();
    let href = format!("mailto:{label}");
    Some(Match {
        start,
        end,
        href,
        kind: Kind::Email,
    })
}

/// Walk the URL run forward from `from`, stopping at whitespace or `<`, at an unbalanced `)`, or at a
/// `]` outside any parenthesis. Returns the index just past the last character of the run.
fn forward_scan(text: &str, from: usize) -> usize {
    let mut depth: i32 = 0;
    let mut j = from;
    while let Some(c) = char_at(text, j) {
        if c.is_whitespace() || c == '<' {
            break;
        }
        match c {
            '(' => depth += 1,
            ')' | ']' if depth == 0 => break,
            ')' => depth -= 1,
            _ => {}
        }
        j += c.len_utf8();
    }
    j
}

/// Drop trailing punctuation from a URL run, never trimming below `min` (the start of the host).
/// A trailing `;` takes its preceding `&entity;` with it when one is present.
fn trim_trailing(text: &str, min: usize, mut end: usize) -> usize {
    while end > min {
        match char_before(text, end) {
            Some('!' | '"' | '\'' | '*' | ',' | '.' | ':' | '?' | '_' | '~') => end -= 1,
            Some(';') => {
                let mut j = end - 1;
                while j > min && char_before(text, j).is_some_and(is_entity_char) {
                    j -= char_before(text, j).map_or(1, char::len_utf8);
                }
                end = if j > min && char_before(text, j) == Some('&') {
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

fn url_scheme_len(text: &str, i: usize) -> Option<usize> {
    ["https://", "http://", "ftp://"]
        .into_iter()
        .find(|scheme| text.get(i..).is_some_and(|rest| rest.starts_with(scheme)))
        .map(str::len)
}

fn alnum_before(text: &str, i: usize) -> bool {
    char_before(text, i).is_some_and(char::is_alphanumeric)
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

    /// Autolink a single text token in the strict dialect, reporting each link as `(label, href)`.
    fn links(text: &str) -> Vec<(String, String)> {
        classed_links(text, false)
            .into_iter()
            .map(|(label, href, _)| (label, href))
            .collect()
    }

    /// Autolink a single text token, reporting each link as `(label, href, classes)`.
    fn classed_links(text: &str, markdown: bool) -> Vec<(String, String, Vec<String>)> {
        let mut inlines = vec![Inline::Str(text.to_owned().into())];
        autolink_inlines(&mut inlines, markdown);
        inlines
            .iter()
            .filter_map(|inline| match inline {
                Inline::Link(attr, label, target) => {
                    let text: String = label
                        .iter()
                        .map(|i| match i {
                            Inline::Str(s) => s.as_str(),
                            _ => "",
                        })
                        .collect();
                    Some((
                        text,
                        target.url.to_string(),
                        attr.classes.iter().map(ToString::to_string).collect(),
                    ))
                }
                _ => None,
            })
            .collect()
    }

    /// Autolink a single text token, returning the resulting inlines.
    fn autolinked(text: &str, markdown: bool) -> Vec<Inline> {
        let mut inlines = vec![Inline::Str(text.to_owned().into())];
        autolink_inlines(&mut inlines, markdown);
        inlines
    }

    fn host(s: &str) -> bool {
        valid_host(s)
    }

    fn scan(s: &str) -> usize {
        forward_scan(s, 0)
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
        let url = "http://e.com/p.,";
        assert_eq!(trim_trailing(url, 7, url.len()), 14); // drops the trailing '.' and ','
        let ent = "http://e.com/p&amp;";
        assert_eq!(trim_trailing(ent, 7, ent.len()), 14); // a trailing '&entity;' goes whole
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
            Box::default(),
            vec![Inline::Str("http://example.com".to_owned().into())],
            Box::new(Target {
                url: "http://example.com".to_owned().into(),
                title: carta_ast::Text::default(),
            }),
        );
        let mut inlines = vec![inner.clone()];
        autolink_inlines(&mut inlines, false);
        assert_eq!(inlines, vec![inner]);
    }

    #[test]
    fn code_is_left_untouched() {
        let code = Inline::Code(Box::default(), "http://example.com".to_owned().into());
        let mut inlines = vec![code.clone()];
        autolink_inlines(&mut inlines, false);
        assert_eq!(inlines, vec![code]);
    }

    #[test]
    fn markdown_dialect_classes_links_and_accepts_single_label_email() {
        // A URL is tagged `uri`, an email `email`, and a single-label domain links in the dialect.
        assert_eq!(
            classed_links("see http://example.com and 5@home now", true),
            vec![
                (
                    "http://example.com".to_owned(),
                    "http://example.com".to_owned(),
                    vec!["uri".to_owned()]
                ),
                (
                    "5@home".to_owned(),
                    "mailto:5@home".to_owned(),
                    vec!["email".to_owned()]
                ),
            ]
        );
        // The strict dialect leaves links unclassed and never links a single-label email domain.
        assert!(classed_links("ping 5@home now", false).is_empty());
        assert_eq!(
            classed_links("at www.example.com today", false),
            vec![(
                "www.example.com".to_owned(),
                "http://www.example.com".to_owned(),
                Vec::new()
            )]
        );
    }

    #[test]
    fn trigger_gate_links_url_and_email_in_both_dialects() {
        for markdown in [false, true] {
            assert_eq!(
                links_of(&autolinked("see https://example.com/x", markdown)),
                vec![(
                    "https://example.com/x".to_owned(),
                    "https://example.com/x".to_owned()
                )],
                "url should link (markdown={markdown})"
            );
            assert_eq!(
                links_of(&autolinked("a@b.com", markdown)),
                vec![("a@b.com".to_owned(), "mailto:a@b.com".to_owned())],
                "email should link (markdown={markdown})"
            );
        }
    }

    #[test]
    fn trigger_gate_respects_www_dialect_split() {
        assert_eq!(
            links_of(&autolinked("www.example.com", false)),
            vec![(
                "www.example.com".to_owned(),
                "http://www.example.com".to_owned()
            )]
        );
        // The `www.` trigger is present in both dialects, but the markdown dialect does not link
        // bare `www.` hosts, so the token must survive as a single unchanged `Str`.
        assert_eq!(
            autolinked("www.example.com", true),
            vec![Inline::Str("www.example.com".to_owned().into())]
        );
    }

    #[test]
    fn trigger_gate_passes_plain_token_through_unchanged() {
        assert_eq!(
            autolinked("nothing-here", false),
            vec![Inline::Str("nothing-here".to_owned().into())]
        );
    }

    #[test]
    fn trigger_gate_scans_but_does_not_link_non_matching_trigger_token() {
        // Contains `://` and `@` triggers so the gate does not skip it, yet nothing matches.
        assert_eq!(
            autolinked("not:a//url @ alone", false),
            vec![Inline::Str("not:a//url @ alone".to_owned().into())]
        );
    }

    /// Report each `Link` in `inlines` as `(label, href)`.
    fn links_of(inlines: &[Inline]) -> Vec<(String, String)> {
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
                    Some((text, target.url.to_string()))
                }
                _ => None,
            })
            .collect()
    }
}
