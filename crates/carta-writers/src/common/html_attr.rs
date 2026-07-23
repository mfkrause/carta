//! Markup escaping and HTML attribute rendering. The XML escaper serves any writer that emits markup
//! metacharacters while the attribute renderers serve the HTML-family writers.

use carta_ast::Attr;

use super::clean_prefix_len;

/// Escape the XML/HTML metacharacters `&`, `<`, and `>` to their entities, and additionally `"` when
/// `escape_quotes` is set (as in an attribute value).
#[cfg_attr(
    not(any(
        feature = "commonmark",
        feature = "gfm",
        feature = "markdown",
        feature = "mediawiki"
    )),
    allow(dead_code)
)]
pub(crate) fn escape_xml(text: &str, escape_quotes: bool) -> String {
    let is_trigger =
        |byte: u8| matches!(byte, b'&' | b'<' | b'>') || (escape_quotes && byte == b'"');
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let clean = clean_prefix_len(rest, is_trigger);
        let Some((head, tail)) = rest.split_at_checked(clean) else {
            out.push_str(rest);
            break;
        };
        out.push_str(head);
        let mut chars = tail.chars();
        let Some(ch) = chars.next() else { break };
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if escape_quotes => out.push_str("&quot;"),
            other => out.push(other),
        }
        rest = chars.as_str();
    }
    out
}

/// Escape an HTML attribute value: `&`, `<`, `>`, and `"` to their entities.
#[cfg_attr(
    not(any(
        feature = "commonmark",
        feature = "gfm",
        feature = "markdown",
        feature = "mediawiki"
    )),
    allow(dead_code)
)]
pub(crate) fn escape_attr(text: &str) -> String {
    escape_xml(text, true)
}

/// Escape an HTML attribute value where both quote characters are entity-encoded: `&`, `<`, `>`,
/// and `"` to their named entities and `'` to `&#39;`. Link and image attribute values take this
/// form; `<div>` and `<span>` wrapper attributes keep the single quote literal via [`escape_attr`].
#[cfg_attr(
    not(any(feature = "commonmark", feature = "gfm", feature = "markdown")),
    allow(dead_code)
)]
pub(crate) fn escape_html_attr(text: &str) -> String {
    let is_trigger = |byte: u8| matches!(byte, b'&' | b'<' | b'>' | b'"' | b'\'');
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let clean = clean_prefix_len(rest, is_trigger);
        let Some((head, tail)) = rest.split_at_checked(clean) else {
            out.push_str(rest);
            break;
        };
        out.push_str(head);
        let mut chars = tail.chars();
        let Some(ch) = chars.next() else { break };
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
        rest = chars.as_str();
    }
    out
}

/// Render an [`Attr`] to an HTML attribute string (a leading space per attribute, empty when blank):
/// `id`, then `class`, then key/value pairs, with unrecognized keys `data-` prefixed.
#[cfg_attr(
    not(any(
        feature = "commonmark",
        feature = "gfm",
        feature = "markdown",
        feature = "mediawiki"
    )),
    allow(dead_code)
)]
pub(crate) fn render_html_attr(attr: &Attr) -> String {
    render_attr_tokens(attr, escape_attr)
}

/// As [`render_html_attr`], but entity-encoding both quote characters in each value: the escaping
/// a link or image tag's attributes take in HTML fragments embedded in text output.
#[cfg_attr(
    not(any(feature = "commonmark", feature = "gfm", feature = "markdown")),
    allow(dead_code)
)]
pub(crate) fn render_html_fragment_attr(attr: &Attr) -> String {
    render_attr_tokens(attr, escape_html_attr)
}

fn render_attr_tokens(attr: &Attr, escaper: fn(&str) -> String) -> String {
    let mut out = String::new();
    for token in html_attr_tokens(attr, escaper) {
        out.push(' ');
        out.push_str(&token);
    }
    out
}

/// The HTML attribute string as individual `name="value"` tokens, in the order [`render_html_attr`]
/// emits them, with each value run through `escaper`. Each token is one unbreakable unit, which
/// lets a caller fill an opening tag to a column width without splitting inside an attribute.
fn html_attr_tokens(attr: &Attr, escaper: fn(&str) -> String) -> Vec<String> {
    let mut tokens = Vec::new();
    if !attr.id.is_empty() {
        tokens.push(format!("id=\"{}\"", escaper(&attr.id)));
    }
    if !attr.classes.is_empty() {
        tokens.push(format!("class=\"{}\"", escaper(&attr.classes.join(" "))));
    }
    for (key, value) in &attr.attributes {
        let name = if is_known_attribute(key) {
            key.to_string()
        } else {
            format!("data-{key}")
        };
        tokens.push(format!("{name}=\"{}\"", escaper(value)));
    }
    tokens
}

/// Whether an attribute name is emitted verbatim in HTML output. Recognized names, the `data-`/`aria-`
/// prefixes, and a few namespaced names pass through; any other key/value attribute is `data-`
/// prefixed by the caller.
#[cfg_attr(
    not(any(
        feature = "commonmark",
        feature = "gfm",
        feature = "html",
        feature = "markdown",
        feature = "mediawiki"
    )),
    allow(dead_code)
)]
pub(crate) fn is_known_attribute(name: &str) -> bool {
    name.starts_with("data-")
        || name.starts_with("aria-")
        || matches!(name, "epub:type" | "xml:lang" | "xmlns")
        || HTML_ATTRIBUTES.binary_search(&name).is_ok()
}

/// HTML attribute names emitted verbatim; any other key/value attribute is `data-` prefixed.
const HTML_ATTRIBUTES: &[&str] = &[
    "abbr",
    "accept",
    "accept-charset",
    "accesskey",
    "action",
    "allow",
    "allowfullscreen",
    "allowpaymentrequest",
    "alt",
    "as",
    "async",
    "autocapitalize",
    "autocomplete",
    "autofocus",
    "autoplay",
    "charset",
    "checked",
    "cite",
    "class",
    "color",
    "cols",
    "colspan",
    "content",
    "contenteditable",
    "controls",
    "coords",
    "crossorigin",
    "data",
    "datetime",
    "decoding",
    "default",
    "defer",
    "dir",
    "dirname",
    "disabled",
    "download",
    "draggable",
    "enctype",
    "enterkeyhint",
    "fetchpriority",
    "for",
    "form",
    "formaction",
    "formenctype",
    "formmethod",
    "formnovalidate",
    "formtarget",
    "headers",
    "height",
    "hidden",
    "high",
    "href",
    "hreflang",
    "http-equiv",
    "id",
    "imagesizes",
    "imagesrcset",
    "inputmode",
    "integrity",
    "is",
    "ismap",
    "itemid",
    "itemprop",
    "itemref",
    "itemscope",
    "itemtype",
    "kind",
    "lang",
    "list",
    "loading",
    "loop",
    "low",
    "manifest",
    "max",
    "maxlength",
    "media",
    "method",
    "min",
    "minlength",
    "multiple",
    "muted",
    "name",
    "nomodule",
    "nonce",
    "novalidate",
    "onabort",
    "onafterprint",
    "onauxclick",
    "onbeforeprint",
    "onbeforeunload",
    "onblur",
    "oncancel",
    "oncanplay",
    "oncanplaythrough",
    "onchange",
    "onclick",
    "onclose",
    "oncontextmenu",
    "oncopy",
    "oncuechange",
    "oncut",
    "ondblclick",
    "ondrag",
    "ondragend",
    "ondragenter",
    "ondragexit",
    "ondragleave",
    "ondragover",
    "ondragstart",
    "ondrop",
    "ondurationchange",
    "onemptied",
    "onended",
    "onerror",
    "onfocus",
    "onhashchange",
    "oninput",
    "oninvalid",
    "onkeydown",
    "onkeypress",
    "onkeyup",
    "onlanguagechange",
    "onload",
    "onloadeddata",
    "onloadedmetadata",
    "onloadend",
    "onloadstart",
    "onmessage",
    "onmessageerror",
    "onmousedown",
    "onmouseenter",
    "onmouseleave",
    "onmousemove",
    "onmouseout",
    "onmouseover",
    "onmouseup",
    "onoffline",
    "ononline",
    "onpagehide",
    "onpageshow",
    "onpaste",
    "onpause",
    "onplay",
    "onplaying",
    "onpopstate",
    "onprogress",
    "onratechange",
    "onrejectionhandled",
    "onreset",
    "onresize",
    "onscroll",
    "onsecuritypolicyviolation",
    "onseeked",
    "onseeking",
    "onselect",
    "onstalled",
    "onstorage",
    "onsubmit",
    "onsuspend",
    "ontimeupdate",
    "ontoggle",
    "onunhandledrejection",
    "onunload",
    "onvolumechange",
    "onwaiting",
    "onwheel",
    "open",
    "optimum",
    "pattern",
    "ping",
    "placeholder",
    "playsinline",
    "poster",
    "preload",
    "readonly",
    "referrerpolicy",
    "rel",
    "required",
    "rev",
    "reversed",
    "role",
    "rows",
    "rowspan",
    "sandbox",
    "scope",
    "selected",
    "shape",
    "size",
    "sizes",
    "slot",
    "span",
    "spellcheck",
    "src",
    "srcdoc",
    "srclang",
    "srcset",
    "start",
    "step",
    "style",
    "tabindex",
    "target",
    "title",
    "translate",
    "type",
    "usemap",
    "value",
    "width",
    "wrap",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_xml_handles_metacharacters() {
        assert_eq!(escape_xml("a<b>&c", false), "a&lt;b&gt;&amp;c");
        assert_eq!(escape_xml("\"q\"", false), "\"q\"");
        assert_eq!(escape_xml("\"q\"", true), "&quot;q&quot;");
        assert_eq!(escape_attr("<\"&>"), "&lt;&quot;&amp;&gt;");
        assert_eq!(escape_attr("a'b"), "a'b");
        assert_eq!(escape_html_attr("<\"&>"), "&lt;&quot;&amp;&gt;");
        assert_eq!(escape_html_attr("a'b"), "a&#39;b");
    }

    #[test]
    fn html_attributes_table_is_sorted() {
        assert!(
            HTML_ATTRIBUTES.is_sorted(),
            "HTML_ATTRIBUTES must stay sorted for the binary search in is_known_attribute"
        );
    }

    #[test]
    fn known_attribute_recognition() {
        assert!(is_known_attribute("href"));
        assert!(is_known_attribute("colspan"));
        assert!(is_known_attribute("data-x"));
        assert!(is_known_attribute("aria-label"));
        assert!(is_known_attribute("epub:type"));
        assert!(is_known_attribute("xml:lang"));
        assert!(!is_known_attribute("wibble"));
    }

    #[test]
    fn render_html_attr_orders_and_prefixes() {
        let attr = Attr {
            id: "x<".into(),
            classes: vec!["a".into(), "b".into()],
            attributes: vec![
                ("href".into(), "/p?q=1&r=2".into()),
                ("wibble".into(), "v".into()),
            ],
        };
        assert_eq!(
            render_html_attr(&attr),
            " id=\"x&lt;\" class=\"a b\" href=\"/p?q=1&amp;r=2\" data-wibble=\"v\""
        );
        assert_eq!(render_html_attr(&Attr::default()), "");
    }

    #[test]
    fn render_html_attr_variants_split_on_the_single_quote() {
        let attr = Attr {
            id: "a'b".into(),
            classes: vec!["c'd".into()],
            attributes: vec![("k".into(), "v'w".into())],
        };
        assert_eq!(
            render_html_attr(&attr),
            " id=\"a'b\" class=\"c'd\" data-k=\"v'w\""
        );
        assert_eq!(
            render_html_fragment_attr(&attr),
            " id=\"a&#39;b\" class=\"c&#39;d\" data-k=\"v&#39;w\""
        );
        assert_eq!(render_html_fragment_attr(&Attr::default()), "");
    }
}
