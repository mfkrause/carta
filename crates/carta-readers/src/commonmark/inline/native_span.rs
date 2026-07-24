//! Pair literal `<span>`/`</span>` raw-inline HTML into span nodes after inline resolution.

use carta_ast::{Attr, Inline};

use super::super::scan::{char_at, scan_entity};

/// Pair literal `<span …>` / `</span>` raw-inline tags into [`Inline::Span`] nodes.
///
/// The inline phase first leaves both tags as raw inline HTML, so emphasis and links resolve around
/// them exactly as they would around any other tag. This pass then walks the resolved tree and,
/// at each nesting level independently, matches an opening tag with the nearest later closing tag,
/// wrapping the inlines between them in a span whose attributes come from the opening tag. Matching
/// stays within one level: a `<span>` that emphasis pulled inside an `Emph` only pairs with a
/// `</span>` that landed inside that same `Emph`. The content between a matched pair is itself
/// re-paired, so nested spans nest. Unmatched tags keep their raw-inline form.
///
/// Known limitation: when an emphasis run straddles exactly one of the two tags (the run opens
/// before a `<span>` and its closing marker sits just before the matching `</span>`), the two tags
/// can land at different levels and stay raw even though a span could have formed.
pub(super) fn pair_native_spans(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut input = inlines.into_iter().peekable();
    pair_spans_level(&mut input, false)
}

/// Pair spans within one nesting level. Pulls from `input` until it is drained, or (when
/// `stop_at_close` is set) until an unmatched closing `</span>` tag is reached, which is left
/// unconsumed for the caller to handle. Container inlines have their own children re-paired.
fn pair_spans_level(
    input: &mut std::iter::Peekable<std::vec::IntoIter<Inline>>,
    stop_at_close: bool,
) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    while let Some(item) = input.peek() {
        if let Inline::RawInline(format, text) = item
            && format.0 == "html"
        {
            match classify_span_tag(text) {
                SpanTag::Open(attr) => {
                    let _ = input.next();
                    let inner = pair_spans_level(input, true);
                    if matches!(input.peek(), Some(Inline::RawInline(f, t))
                        if f.0 == "html" && matches!(classify_span_tag(t), SpanTag::Close))
                    {
                        let _ = input.next();
                        out.push(Inline::Span(Box::new(attr), inner));
                    } else {
                        // No close at this level: opener reverts to raw, inner content rejoins.
                        out.push(Inline::RawInline(
                            carta_ast::Format("html".into()),
                            open_tag_raw(&attr).into(),
                        ));
                        out.extend(inner);
                    }
                    continue;
                }
                SpanTag::Close if stop_at_close => break,
                _ => {}
            }
        }
        if let Some(next) = input.next() {
            out.push(recurse_span_children(next));
        }
    }
    out
}

/// Re-pair spans inside the child lists of a container inline.
fn recurse_span_children(inline: Inline) -> Inline {
    match inline {
        Inline::Emph(c) => Inline::Emph(pair_native_spans(c)),
        Inline::Underline(c) => Inline::Underline(pair_native_spans(c)),
        Inline::Strong(c) => Inline::Strong(pair_native_spans(c)),
        Inline::Strikeout(c) => Inline::Strikeout(pair_native_spans(c)),
        Inline::Superscript(c) => Inline::Superscript(pair_native_spans(c)),
        Inline::Subscript(c) => Inline::Subscript(pair_native_spans(c)),
        Inline::SmallCaps(c) => Inline::SmallCaps(pair_native_spans(c)),
        Inline::Quoted(q, c) => Inline::Quoted(q, pair_native_spans(c)),
        Inline::Cite(cites, c) => Inline::Cite(cites, pair_native_spans(c)),
        Inline::Link(a, c, t) => Inline::Link(a, pair_native_spans(c), t),
        Inline::Image(a, c, t) => Inline::Image(a, pair_native_spans(c), t),
        Inline::Span(a, c) => Inline::Span(a, pair_native_spans(c)),
        other => other,
    }
}

/// The role of an HTML tag with respect to span pairing.
enum SpanTag {
    /// A literal `<span …>` opener, with its attributes parsed.
    Open(Attr),
    /// A literal `</span>` closer.
    Close,
    /// Any other tag, which plays no part in span pairing.
    Other,
}

/// Cheap pre-check before the char-by-char classification: does `raw` open with `<span` or `</span`
/// (case-insensitive)? Lets the common non-span inline tag bail out without allocating.
fn opens_span_tag(raw: &str) -> bool {
    let Some(after_lt) = raw.strip_prefix('<') else {
        return false;
    };
    let candidate = after_lt.strip_prefix('/').unwrap_or(after_lt);
    candidate
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("span"))
}

/// Classify a raw HTML tag string, parsing attributes for an opening `<span …>`. A self-closing
/// `<span/>` is `Other`: it has no content to wrap and stays raw.
fn classify_span_tag(raw: &str) -> SpanTag {
    if !opens_span_tag(raw) {
        return SpanTag::Other;
    }
    if char_at(raw, 1) == Some('/') {
        // `</span>` with optional trailing whitespace before `>`.
        let mut i = 2;
        if !matches_name(raw, &mut i, "span") {
            return SpanTag::Other;
        }
        while matches!(char_at(raw, i), Some(' ' | '\t' | '\n')) {
            i += 1;
        }
        if char_at(raw, i) == Some('>') && i + 1 == raw.len() {
            return SpanTag::Close;
        }
        return SpanTag::Other;
    }
    let mut i = 1;
    if !matches_name(raw, &mut i, "span") {
        return SpanTag::Other;
    }
    // A name character right after `span` means a different tag (`<spanner>`).
    if matches!(char_at(raw, i), Some(c) if c.is_ascii_alphanumeric() || c == '-') {
        return SpanTag::Other;
    }
    match parse_span_attributes(raw, i) {
        Some(attr) => SpanTag::Open(attr),
        None => SpanTag::Other,
    }
}

/// Match the literal `name` case-insensitively at `*i`, advancing `*i` past it on success. `name`
/// is ASCII, so its character offsets are byte offsets.
fn matches_name(text: &str, i: &mut usize, name: &str) -> bool {
    for (offset, expected) in name.chars().enumerate() {
        match char_at(text, *i + offset) {
            Some(c) if c.eq_ignore_ascii_case(&expected) => {}
            _ => return false,
        }
    }
    *i += name.len();
    true
}

/// Parse the attributes of an opening `<span …>` tag whose name ends at `start`, expecting the tag
/// to end with `>` (a trailing `/` makes it self-closing, which is rejected here). An `id` attribute
/// becomes the identifier and a `class` attribute splits into classes; only the first of each is
/// kept. Every other attribute becomes a key/value pair in source order; a valueless attribute
/// carries an empty value. Entity and numeric character references in values are decoded.
fn parse_span_attributes(text: &str, start: usize) -> Option<Attr> {
    let mut attr = Attr::default();
    let mut seen_class = false;
    let mut i = start;
    loop {
        let ws_start = i;
        while matches!(char_at(text, i), Some(' ' | '\t' | '\n')) {
            i += 1;
        }
        match char_at(text, i) {
            Some('>') if i + 1 == text.len() => return Some(attr),
            // A self-closing tag has no content to wrap.
            Some('/') => return None,
            _ => {}
        }
        // An attribute must be preceded by whitespace.
        if i == ws_start {
            return None;
        }
        let name_start = i;
        while matches!(
            char_at(text, i),
            Some(c) if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.')
        ) {
            i += 1;
        }
        if i == name_start {
            return None;
        }
        let name = text.get(name_start..i)?.to_owned();
        let mut value = String::new();
        // Optional `= value` with whitespace allowed around `=`.
        let mut after = i;
        while matches!(char_at(text, after), Some(' ' | '\t' | '\n')) {
            after += 1;
        }
        if char_at(text, after) == Some('=') {
            after += 1;
            while matches!(char_at(text, after), Some(' ' | '\t' | '\n')) {
                after += 1;
            }
            let (parsed, next) = read_attr_value(text, after)?;
            value = parsed;
            i = next;
        } else {
            i = after;
        }
        match name.as_str() {
            "id" => {
                if attr.id.is_empty() {
                    attr.id = value.into();
                }
            }
            "class" => {
                if !seen_class {
                    seen_class = true;
                    attr.classes = value.split_whitespace().map(Into::into).collect();
                }
            }
            _ => attr.attributes.push((name.into(), value.into())),
        }
    }
}

/// Read an HTML attribute value at `start`: a double- or single-quoted string, or an unquoted run.
/// Returns the decoded value and the index just past it. Character references inside the value are
/// decoded.
fn read_attr_value(text: &str, start: usize) -> Option<(String, usize)> {
    let quote = char_at(text, start);
    if matches!(quote, Some('"' | '\'')) {
        let quote = quote?;
        let mut i = start + 1;
        let mut out = String::new();
        loop {
            match char_at(text, i) {
                Some(c) if c == quote => return Some((out, i + 1)),
                Some('&') => {
                    if let Some((decoded, next)) = scan_entity(text, i) {
                        out.push_str(&decoded);
                        i = next;
                    } else {
                        out.push('&');
                        i += 1;
                    }
                }
                Some(c) => {
                    out.push(c);
                    i += c.len_utf8();
                }
                None => return None,
            }
        }
    }
    let mut i = start;
    let mut out = String::new();
    while let Some(c) = char_at(text, i) {
        if matches!(c, ' ' | '\t' | '\n' | '"' | '\'' | '=' | '<' | '>' | '`') {
            break;
        }
        if c == '&'
            && let Some((decoded, next)) = scan_entity(text, i)
        {
            out.push_str(&decoded);
            i = next;
            continue;
        }
        out.push(c);
        i += c.len_utf8();
    }
    if out.is_empty() {
        return None;
    }
    Some((out, i))
}

/// Reconstruct the raw `<span …>` opener for an opener that found no matching close, so it falls
/// back to literal raw inline HTML. The exact original spelling is not recovered; a normalized form
/// carrying the same attributes is emitted.
fn open_tag_raw(attr: &Attr) -> String {
    let mut s = String::from("<span");
    if !attr.id.is_empty() {
        s.push_str(" id=\"");
        s.push_str(&attr.id);
        s.push('"');
    }
    if !attr.classes.is_empty() {
        s.push_str(" class=\"");
        s.push_str(&attr.classes.join(" "));
        s.push('"');
    }
    for (k, v) in &attr.attributes {
        s.push(' ');
        s.push_str(k);
        s.push_str("=\"");
        s.push_str(v);
        s.push('"');
    }
    s.push('>');
    s
}
