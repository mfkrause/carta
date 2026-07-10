//! Inlining external resources into a finished HTML document to make it self-contained.
//!
//! A rendered page references its images, stylesheets, and scripts by URL. This pass rewrites those
//! references in place so the document carries every resource within itself: an image becomes a
//! `data:` URI, a linked stylesheet or external script becomes an inline `<style>`/`<script>` element,
//! and a `url(...)` inside CSS becomes a `data:` URI too. The bytes for each reference come from a
//! caller-supplied resolver — reading a file, fetching a URL, consulting a bag — so this module holds
//! no I/O: it only finds the references and splices the resolved bytes back in.
//!
//! The pass runs over the final markup, after line wrapping, so a reference that spans a wrapped line
//! is inlined exactly where it sits and the surrounding layout is left untouched.

use carta_core::media::base64_encode;

/// A resolved resource: its bytes and, when the resolver could determine one, its MIME type.
#[derive(Debug, Clone)]
pub struct Resource {
    /// The resource's raw bytes.
    pub bytes: Vec<u8>,
    /// The resource's MIME type, when known.
    pub mime: Option<String>,
}

/// Inline every external resource an HTML document references, producing a self-contained page.
///
/// Each referenced URL is offered to `resolve`, which returns the resource's bytes and MIME type, or
/// `None` to leave the reference as written (an unresolved or unreachable resource stays external).
/// Images and other media are inlined as `data:` URIs; a linked stylesheet becomes an inline `<style>`
/// and an external script an inline `<script>`; `url(...)` references inside `<style>` blocks and
/// inlined stylesheets are inlined too. A reference that is already a `data:` URI, a fragment, or
/// empty is left alone.
pub fn inline_resources(html: &str, mut resolve: impl FnMut(&str) -> Option<Resource>) -> String {
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while i < html.len() {
        let Some(rel) = html.get(i..).and_then(|rest| rest.find('<')) else {
            out.push_str(sub(html, i, html.len()));
            break;
        };
        let lt = i + rel;
        out.push_str(sub(html, i, lt));
        i = handle_tag(html, lt, &mut out, &mut resolve);
    }
    out
}

/// Handle the markup beginning at the `<` at `lt`, appending the (possibly rewritten) result to `out`
/// and returning the index just past what was consumed. A construct this pass does not touch —
/// a comment, a declaration, a plain tag — is copied through verbatim.
fn handle_tag(
    html: &str,
    lt: usize,
    out: &mut String,
    resolve: &mut impl FnMut(&str) -> Option<Resource>,
) -> usize {
    // Comments, declarations, and processing instructions are copied to their `>` untouched.
    if let Some(after) = markup_prefix_end(html, lt) {
        out.push_str(sub(html, lt, after));
        return after;
    }
    let Some(tag) = parse_start_tag(html, lt) else {
        // A lone `<` that opens no tag is literal text.
        out.push('<');
        return lt + 1;
    };
    match tag.name.as_str() {
        // Style and script hold raw text (their body is not parsed as markup), so both consume through
        // their closing tag. A stylesheet link and a sourced script are replaced whole; other cases
        // rewrite in place.
        "style" => inline_style_element(html, &tag, out, resolve),
        "script" => inline_script_element(html, &tag, out, resolve),
        "link" if is_stylesheet_link(&tag) => inline_link_element(html, &tag, out, resolve),
        _ => {
            rewrite_media_tag(html, &tag, out, resolve);
            tag.end
        }
    }
}

/// The attributes whose value names a resource to inline, per element. Each names a single URL;
/// `srcset`, being a responsive candidate list rather than a lone reference, is deliberately left
/// alone so its descriptors and multiple sources survive untouched.
fn resource_attrs(element: &str) -> &'static [&'static str] {
    match element {
        "img" => &["src", "data-src", "poster"],
        "video" | "audio" => &["src", "poster"],
        "source" | "input" | "embed" | "track" | "iframe" => &["src"],
        _ => &[],
    }
}

/// Rewrite the resource-bearing attributes of a media element in place, splicing each resolved
/// reference back as a `data:` URI while leaving the rest of the tag — its spacing, attribute order,
/// and every other attribute — byte-for-byte as it was. A media element additionally gains the
/// assistive-technology annotation of [`aria_prefix`], since an inlined graphic carries no external
/// source to describe it.
fn rewrite_media_tag(
    html: &str,
    tag: &StartTag,
    out: &mut String,
    resolve: &mut impl FnMut(&str) -> Option<Resource>,
) {
    let wanted = resource_attrs(&tag.name);
    let bears_resource = tag
        .attrs
        .iter()
        .any(|attr| wanted.contains(&attr.name.as_str()));
    if !bears_resource {
        // Not a media element, or one carrying none of its resource attributes (an `<img>` with only a
        // `srcset`, say): copy it through verbatim, annotation and all.
        out.push_str(sub(html, tag.lt, tag.end));
        return;
    }
    // Collect the value replacements, in source order, then splice them across the original tag text.
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    // The assistive-technology annotation is inserted right after the element name, before the first
    // attribute, so it precedes everything the tag already carries.
    let name_end = tag.lt + 1 + tag.name.len();
    let prefix = aria_prefix(tag);
    if !prefix.is_empty() {
        edits.push((name_end, name_end, prefix));
    }
    for attr in &tag.attrs {
        let Some((start, end)) = attr.value_span else {
            continue;
        };
        if !wanted.contains(&attr.name.as_str()) {
            continue;
        }
        let value = sub(html, start, end);
        let replaced = inline_url("", value, resolve).unwrap_or_else(|| value.to_owned());
        if replaced != value {
            edits.push((start, end, replaced));
        }
    }
    splice(html, tag.lt, tag.end, &edits, out);
}

/// The assistive-technology attributes a media element gains when its resources are inlined, ready to
/// insert right after the element name: `role="img"` unless the element already carries a `role`, and
/// an `aria-label` mirroring its `alt` attribute (value and all, including a valueless one) when it has
/// one. Empty when the element needs neither.
fn aria_prefix(tag: &StartTag) -> String {
    let mut prefix = String::new();
    if !tag.has_attr("role") {
        prefix.push_str(" role=\"img\"");
    }
    if let Some(alt) = tag.attr_entry("alt") {
        match &alt.value {
            Some(value) => {
                prefix.push_str(" aria-label=\"");
                prefix.push_str(value);
                prefix.push('"');
            }
            None => prefix.push_str(" aria-label"),
        }
    }
    prefix
}

/// Resolve one URL to a `data:` URI, or `None` to leave it as written. A reference that is empty, a
/// fragment, or already a `data:` URI is left alone; anything else is joined against `base` and offered
/// to the resolver.
fn inline_url(
    base: &str,
    url: &str,
    resolve: &mut impl FnMut(&str) -> Option<Resource>,
) -> Option<String> {
    if skip_reference(url) {
        return None;
    }
    let target = join_url(base, url);
    let resource = resolve(&target)?;
    Some(data_uri(resource.mime.as_deref(), &resource.bytes))
}

/// A `data:` URI carrying `bytes` inline: the MIME type — or `application/octet-stream` when none is
/// known — then the bytes as unbroken base64.
fn data_uri(mime: Option<&str>, bytes: &[u8]) -> String {
    let mime = mime.unwrap_or("application/octet-stream");
    format!("data:{mime};base64,{}", base64_encode(bytes))
}

/// A `<style>` element: emit its start tag unchanged, then its CSS body with every `url(...)` inlined,
/// then its closing tag. Returns the index past `</style>`.
fn inline_style_element(
    html: &str,
    tag: &StartTag,
    out: &mut String,
    resolve: &mut impl FnMut(&str) -> Option<Resource>,
) -> usize {
    let (body, end) = raw_text_body(html, tag.end, "style");
    out.push_str(sub(html, tag.lt, tag.end));
    out.push_str(&inline_css("", body, resolve));
    out.push_str(sub(html, tag.end + body.len(), end));
    end
}

/// A `<script>` element: when it sources an external file, replace the whole element with an inline
/// script carrying the fetched code (keeping only a `type` attribute, as a self-contained script needs
/// no `src`); otherwise leave it untouched. Returns the index past `</script>`.
fn inline_script_element(
    html: &str,
    tag: &StartTag,
    out: &mut String,
    resolve: &mut impl FnMut(&str) -> Option<Resource>,
) -> usize {
    let (_body, end) = raw_text_body(html, tag.end, "script");
    match tag
        .attr("src")
        .and_then(|src| inline_text_resource(src, resolve))
    {
        Some(code) => {
            out.push_str("<script");
            if let Some(kind) = tag.attr("type") {
                out.push_str(" type=\"");
                out.push_str(kind);
                out.push('"');
            }
            out.push('>');
            out.push_str(&code);
            out.push_str("</script>");
        }
        None => out.push_str(sub(html, tag.lt, end)),
    }
    end
}

/// A `<link rel="stylesheet">`: replace it with an inline `<style>` carrying the fetched stylesheet,
/// its own `url(...)` references inlined relative to the stylesheet's location. A stylesheet that will
/// not resolve is left as the original link. Returns the index past the link tag.
fn inline_link_element(
    html: &str,
    tag: &StartTag,
    out: &mut String,
    resolve: &mut impl FnMut(&str) -> Option<Resource>,
) -> usize {
    match tag
        .attr("href")
        .filter(|href| !skip_reference(href))
        .and_then(|href| Some((href, inline_text_resource(href, resolve)?)))
    {
        Some((href, css)) => {
            out.push_str("<style type=\"text/css\">");
            out.push_str(&inline_css(href, &css, resolve));
            out.push_str("</style>");
        }
        None => out.push_str(sub(html, tag.lt, tag.end)),
    }
    tag.end
}

/// Fetch a text resource (a stylesheet or script) and decode it as UTF-8, replacing any invalid bytes.
fn inline_text_resource(
    url: &str,
    resolve: &mut impl FnMut(&str) -> Option<Resource>,
) -> Option<String> {
    let resource = resolve(url)?;
    Some(String::from_utf8_lossy(&resource.bytes).into_owned())
}

/// Rewrite every `url(...)` in a CSS body to a `data:` URI, resolving each relative reference against
/// `base` — the location of the stylesheet the CSS came from, or empty for a `<style>` block that sits
/// in the document itself.
fn inline_css(base: &str, css: &str, resolve: &mut impl FnMut(&str) -> Option<Resource>) -> String {
    let mut out = String::with_capacity(css.len());
    let mut i = 0;
    while i < css.len() {
        let Some(rel) = css.get(i..).and_then(|rest| find_ci(rest, "url(")) else {
            out.push_str(sub(css, i, css.len()));
            break;
        };
        let open = i + rel + "url(".len();
        let Some(close_rel) = css.get(open..).and_then(|rest| rest.find(')')) else {
            out.push_str(sub(css, i, css.len()));
            break;
        };
        let close = open + close_rel;
        out.push_str(sub(css, i, open));
        let raw = sub(css, open, close);
        let url = unquote(raw.trim());
        match inline_url(base, url, resolve) {
            Some(data) => out.push_str(&data),
            None => out.push_str(raw),
        }
        out.push(')');
        i = close + 1;
    }
    out
}

/// A parsed HTML start tag: its lowercased element name, its attributes with their value spans, and
/// the byte range it occupies in the source (`lt`..`end`).
struct StartTag {
    name: String,
    attrs: Vec<TagAttr>,
    lt: usize,
    end: usize,
}

impl StartTag {
    /// The value of the named attribute, if it carries one.
    fn attr(&self, name: &str) -> Option<&str> {
        self.attr_entry(name).and_then(|attr| attr.value.as_deref())
    }

    /// The named attribute, whether or not it carries a value.
    fn attr_entry(&self, name: &str) -> Option<&TagAttr> {
        self.attrs.iter().find(|attr| attr.name == name)
    }

    /// Whether the tag carries the named attribute (with or without a value).
    fn has_attr(&self, name: &str) -> bool {
        self.attr_entry(name).is_some()
    }
}

/// One attribute of a start tag: its lowercased name, its value text, and the absolute byte span of
/// that value's inner content (without the surrounding quotes) in the source.
struct TagAttr {
    name: String,
    value: Option<String>,
    value_span: Option<(usize, usize)>,
}

/// Whether a `<link>` requests a stylesheet — its `rel` names `stylesheet` among space-separated tokens.
fn is_stylesheet_link(tag: &StartTag) -> bool {
    tag.attr("rel").is_some_and(|rel| {
        rel.split_whitespace()
            .any(|token| token.eq_ignore_ascii_case("stylesheet"))
    })
}

/// If the `<` at `lt` opens a comment, a `<!...>` declaration, or a `<?...?>` instruction, the index
/// just past its terminator; otherwise `None`. These are copied through without interpretation.
fn markup_prefix_end(html: &str, lt: usize) -> Option<usize> {
    let rest = html.get(lt..)?;
    if let Some(after) = rest.strip_prefix("<!--") {
        let close = after.find("-->").map_or(html.len(), |p| lt + 4 + p + 3);
        return Some(close);
    }
    if rest.starts_with("<!") || rest.starts_with("<?") {
        let close = rest
            .get(2..)
            .and_then(|r| r.find('>'))
            .map_or(html.len(), |p| lt + 2 + p + 1);
        return Some(close);
    }
    None
}

/// Parse the start tag whose `<` sits at `lt`. Returns `None` when the character does not open a
/// well-formed start tag (a stray `<`, or an end tag `</…>`), leaving the caller to treat it as text.
fn parse_start_tag(html: &str, lt: usize) -> Option<StartTag> {
    let bytes = html.as_bytes();
    let mut p = lt + 1;
    // A name must start with an ASCII letter; anything else (including `/`) is not a start tag here.
    if !matches!(bytes.get(p), Some(b) if b.is_ascii_alphabetic()) {
        return None;
    }
    let name_start = p;
    while matches!(bytes.get(p), Some(b) if b.is_ascii_alphanumeric() || *b == b'-' || *b == b':') {
        p += 1;
    }
    let name = sub(html, name_start, p).to_ascii_lowercase();

    let mut attrs = Vec::new();
    loop {
        p = skip_whitespace(bytes, p);
        match bytes.get(p) {
            None => return None,
            Some(b'>') => {
                return Some(StartTag {
                    name,
                    attrs,
                    lt,
                    end: p + 1,
                });
            }
            Some(b'/') => {
                if matches!(bytes.get(p + 1), Some(b'>')) {
                    return Some(StartTag {
                        name,
                        attrs,
                        lt,
                        end: p + 2,
                    });
                }
                p += 1;
            }
            Some(_) => {
                let attr_start = p;
                while matches!(bytes.get(p), Some(b) if !b.is_ascii_whitespace() && *b != b'=' && *b != b'>' && *b != b'/')
                {
                    p += 1;
                }
                if p == attr_start {
                    // No progress on a character that starts no name (a stray `=` or quote): step over
                    // it so the scan cannot stall.
                    p += 1;
                    continue;
                }
                let attr_name = sub(html, attr_start, p).to_ascii_lowercase();
                let after_name = skip_whitespace(bytes, p);
                let (value, value_span, next) = if matches!(bytes.get(after_name), Some(b'=')) {
                    read_value(html, skip_whitespace(bytes, after_name + 1))
                } else {
                    (None, None, p)
                };
                attrs.push(TagAttr {
                    name: attr_name,
                    value,
                    value_span,
                });
                p = next;
            }
        }
    }
}

/// Read an attribute value starting at `p`: a quoted string (returning its inner span), or an unquoted
/// run up to whitespace or `>`. Returns the value text, its inner byte span, and the index past it.
fn read_value(html: &str, p: usize) -> (Option<String>, Option<(usize, usize)>, usize) {
    let bytes = html.as_bytes();
    match bytes.get(p) {
        Some(&quote @ (b'"' | b'\'')) => {
            let start = p + 1;
            let mut q = start;
            while matches!(bytes.get(q), Some(&b) if b != quote) {
                q += 1;
            }
            let value = sub(html, start, q).to_owned();
            let end = if bytes.get(q).is_some() { q + 1 } else { q };
            (Some(value), Some((start, q)), end)
        }
        Some(_) => {
            let start = p;
            let mut q = start;
            while matches!(bytes.get(q), Some(b) if !b.is_ascii_whitespace() && *b != b'>') {
                q += 1;
            }
            (Some(sub(html, start, q).to_owned()), Some((start, q)), q)
        }
        None => (None, None, p),
    }
}

/// The raw-text body of an element that began at `start`, up to (but not including) its case-insensitive
/// closing tag, together with the index just past that closing tag. A missing close runs to end of input.
fn raw_text_body<'a>(html: &'a str, start: usize, element: &str) -> (&'a str, usize) {
    let closer = format!("</{element}");
    let rest = sub(html, start, html.len());
    match find_ci(rest, &closer) {
        Some(rel) => {
            let body_end = start + rel;
            let after_name = body_end + closer.len();
            let close_end = html
                .get(after_name..)
                .and_then(|r| r.find('>'))
                .map_or(html.len(), |p| after_name + p + 1);
            (sub(html, start, body_end), close_end)
        }
        None => (rest, html.len()),
    }
}

/// Copy `html[lt..end]` to `out`, applying each `(start, end, replacement)` edit — non-overlapping and
/// in ascending order — to the value spans within it.
fn splice(html: &str, lt: usize, end: usize, edits: &[(usize, usize, String)], out: &mut String) {
    let mut cursor = lt;
    for (start, stop, replacement) in edits {
        if *start < cursor || *stop > end {
            continue;
        }
        out.push_str(sub(html, cursor, *start));
        out.push_str(replacement);
        cursor = *stop;
    }
    out.push_str(sub(html, cursor, end));
}

/// Whether a reference names nothing to fetch: empty, a page fragment, or an inline `data:` payload.
fn skip_reference(url: &str) -> bool {
    let url = url.trim();
    url.is_empty() || url.starts_with('#') || url.starts_with("data:")
}

/// Resolve a possibly relative reference against the location of the document it appears in. An
/// absolute URL (with a scheme or a protocol-relative `//`) stands as written; a root-relative path is
/// joined to the base's origin; anything else is joined to the base's directory. An empty base leaves a
/// relative reference untouched, so the resolver reads it against its own working directory.
fn join_url(base: &str, url: &str) -> String {
    if base.is_empty() || url.contains("://") || url.starts_with("//") {
        return url.to_owned();
    }
    if let Some(path) = url.strip_prefix('/') {
        if let Some(origin) = origin_of(base) {
            return format!("{origin}/{path}");
        }
        return url.to_owned();
    }
    let Some((dir, _)) = base.rsplit_once('/') else {
        return url.to_owned();
    };
    format!("{dir}/{url}")
}

/// The scheme-and-host origin of an absolute URL (`https://host`), for joining a root-relative path.
fn origin_of(base: &str) -> Option<&str> {
    let scheme_end = base.find("://")?;
    let after = scheme_end + 3;
    let host_len = base.get(after..)?.find('/').unwrap_or(base.len() - after);
    base.get(..after + host_len)
}

/// Strip one pair of matching quotes from a CSS `url()` argument, if present.
fn unquote(value: &str) -> &str {
    let bytes = value.as_bytes();
    match (bytes.first(), bytes.last()) {
        (Some(&a @ (b'"' | b'\'')), Some(&b)) if a == b && value.len() >= 2 => {
            sub(value, 1, value.len() - 1)
        }
        _ => value,
    }
}

/// Advance past ASCII whitespace from `p`.
fn skip_whitespace(bytes: &[u8], mut p: usize) -> usize {
    while matches!(bytes.get(p), Some(b) if b.is_ascii_whitespace()) {
        p += 1;
    }
    p
}

/// The byte offset of `needle` in `haystack`, compared ASCII-case-insensitively.
fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let (hay, need) = (haystack.as_bytes(), needle.as_bytes());
    if need.is_empty() {
        return Some(0);
    }
    hay.windows(need.len())
        .position(|window| window.eq_ignore_ascii_case(need))
}

/// A panic-free substring: `s[start..end]`, or `""` if the range is not a valid boundary pair. The
/// callers derive their bounds from byte scans over ASCII delimiters, so the fallback is never taken.
fn sub(s: &str, start: usize, end: usize) -> &str {
    s.get(start..end).unwrap_or("")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]
    use super::{Resource, inline_resources};

    /// A resolver that hands back fixed bytes and MIME for a known set of references and `None` for
    /// anything else, recording which references it was asked for.
    fn resolver<'a>(
        table: &'a [(&'static str, &'static [u8], Option<&'static str>)],
    ) -> impl FnMut(&str) -> Option<Resource> + 'a {
        move |url: &str| {
            table
                .iter()
                .find(|(name, _, _)| *name == url)
                .map(|(_, bytes, mime)| Resource {
                    bytes: bytes.to_vec(),
                    mime: mime.map(str::to_owned),
                })
        }
    }

    #[test]
    fn img_src_becomes_a_data_uri_and_gains_the_aria_annotation() {
        let html = r#"<p><img src="pic.png" alt="x" /></p>"#;
        let out = inline_resources(html, resolver(&[("pic.png", b"PNGX", Some("image/png"))]));
        assert_eq!(
            out,
            r#"<p><img role="img" aria-label="x" src="data:image/png;base64,UE5HWA==" alt="x" /></p>"#
        );
    }

    #[test]
    fn img_without_alt_gains_role_only() {
        let html = r#"<img src="pic.png">"#;
        let out = inline_resources(html, resolver(&[("pic.png", b"PNGX", Some("image/png"))]));
        assert_eq!(
            out,
            r#"<img role="img" src="data:image/png;base64,UE5HWA==">"#
        );
    }

    #[test]
    fn img_keeping_an_existing_role_only_gains_the_aria_label() {
        let html = r#"<img role="button" src="pic.png" alt="y">"#;
        let out = inline_resources(html, resolver(&[("pic.png", b"X", Some("image/png"))]));
        assert_eq!(
            out,
            r#"<img aria-label="y" role="button" src="data:image/png;base64,WA==" alt="y">"#
        );
    }

    #[test]
    fn unresolved_and_data_src_keep_their_value_but_gain_role() {
        // The src value is left as written (unresolved, or already a data: URI), but an inlined graphic
        // is still annotated. A non-media `<a>` is untouched, fragment href and all.
        let html =
            r##"<img src="gone.png"><img src="data:image/png;base64,AAAA"><a href="#top">x</a>"##;
        let out = inline_resources(html, resolver(&[]));
        assert_eq!(
            out,
            r##"<img role="img" src="gone.png"><img role="img" src="data:image/png;base64,AAAA"><a href="#top">x</a>"##
        );
    }

    #[test]
    fn other_attributes_and_spacing_are_preserved_byte_for_byte() {
        // The annotation is inserted after the name; the src value changes; id, the odd spacing, and
        // the alt attribute stay put.
        let html = "<img   id=\"a\"  src='pic.png'   alt=\"y\">";
        let out = inline_resources(html, resolver(&[("pic.png", b"hi", Some("image/gif"))]));
        assert_eq!(
            out,
            "<img role=\"img\" aria-label=\"y\"   id=\"a\"  src='data:image/gif;base64,aGk='   alt=\"y\">"
        );
    }

    #[test]
    fn image_with_only_a_srcset_is_left_untouched() {
        // A responsive candidate list is not a lone reference: it is left alone, and an image that
        // carries no resource attribute the pass handles gains no annotation.
        let html = r#"<img srcset="a.png 1x, b.png 2x">"#;
        let out = inline_resources(
            html,
            resolver(&[
                ("a.png", b"A", Some("image/png")),
                ("b.png", b"B", Some("image/png")),
            ]),
        );
        assert_eq!(out, html);
    }

    #[test]
    fn style_block_inlines_css_url() {
        let html = "<style>.a{background:url(pic.png)}</style>";
        let out = inline_resources(html, resolver(&[("pic.png", b"PNGX", Some("image/png"))]));
        assert_eq!(
            out,
            "<style>.a{background:url(data:image/png;base64,UE5HWA==)}</style>"
        );
    }

    #[test]
    fn quoted_css_url_is_inlined_unquoted() {
        let html = r#"<style>.a{background:url("pic.png")}</style>"#;
        let out = inline_resources(html, resolver(&[("pic.png", b"PNGX", Some("image/png"))]));
        assert_eq!(
            out,
            "<style>.a{background:url(data:image/png;base64,UE5HWA==)}</style>"
        );
    }

    #[test]
    fn stylesheet_link_becomes_inline_style_with_resolved_urls() {
        // The stylesheet's own url() resolves relative to the stylesheet's directory.
        let html = r#"<link rel="stylesheet" href="css/site.css">"#;
        let out = inline_resources(
            html,
            resolver(&[
                (
                    "css/site.css",
                    b".a{background:url(bg.png)}",
                    Some("text/css"),
                ),
                ("css/bg.png", b"PNGX", Some("image/png")),
            ]),
        );
        assert_eq!(
            out,
            r#"<style type="text/css">.a{background:url(data:image/png;base64,UE5HWA==)}</style>"#
        );
    }

    #[test]
    fn script_src_becomes_inline_script_keeping_only_type() {
        let html = r#"<script defer src="app.js" type="text/javascript" id="z"></script>"#;
        let out = inline_resources(
            html,
            resolver(&[("app.js", b"console.log(1)", Some("text/javascript"))]),
        );
        assert_eq!(
            out,
            r#"<script type="text/javascript">console.log(1)</script>"#
        );
    }

    #[test]
    fn script_without_type_drops_all_attributes() {
        let html = r#"<script src="app.js"></script>"#;
        let out = inline_resources(html, resolver(&[("app.js", b"x", None)]));
        assert_eq!(out, "<script>x</script>");
    }

    #[test]
    fn inline_script_body_is_left_alone() {
        let html = "<script>var a = 1 < 2;</script>";
        let out = inline_resources(html, resolver(&[]));
        assert_eq!(out, html);
    }

    #[test]
    fn comment_content_is_not_interpreted() {
        let html = r#"<!-- <img src="pic.png"> --><img src="pic.png">"#;
        let out = inline_resources(html, resolver(&[("pic.png", b"X", Some("image/png"))]));
        assert_eq!(
            out,
            r#"<!-- <img src="pic.png"> --><img role="img" src="data:image/png;base64,WA==">"#
        );
    }

    #[test]
    fn unresolved_stylesheet_link_is_left_as_the_link() {
        let html = r#"<link rel="stylesheet" href="missing.css">"#;
        let out = inline_resources(html, resolver(&[]));
        assert_eq!(out, html);
    }

    #[test]
    fn non_stylesheet_link_is_untouched() {
        let html = r#"<link rel="icon" href="fav.png">"#;
        let out = inline_resources(html, resolver(&[("fav.png", b"X", Some("image/png"))]));
        assert_eq!(out, html);
    }

    #[test]
    fn absolute_css_url_is_resolved_against_its_own_origin() {
        let html = r#"<link rel="stylesheet" href="https://h.example/a/site.css">"#;
        let out = inline_resources(
            html,
            resolver(&[
                (
                    "https://h.example/a/site.css",
                    b"a{background:url(/img/bg.png)}",
                    Some("text/css"),
                ),
                ("https://h.example/img/bg.png", b"PNGX", Some("image/png")),
            ]),
        );
        assert_eq!(
            out,
            r#"<style type="text/css">a{background:url(data:image/png;base64,UE5HWA==)}</style>"#
        );
    }
}
