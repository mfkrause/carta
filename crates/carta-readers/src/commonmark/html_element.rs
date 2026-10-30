//! Recognition of block-level HTML elements whose inner content is parsed as markdown: scanning an
//! open tag (name, attributes, extent) and locating its matching close tag. Pure functions over the
//! raw line text.
use carta_ast::Attr;

/// Literal HTML content that can continue across physical lines.
#[derive(Clone, Copy, Debug)]
pub(super) enum HtmlLiteral {
    Comment,
    ProcessingInstruction,
    Cdata,
    Pre,
    Script,
    Style,
    Textarea,
}

/// A recognized open tag at the start of a line.
pub(super) struct OpenTag {
    /// The lowercased tag name.
    pub(super) tag: String,
    /// Attributes parsed from the tag (`id`, `class`, and other key/values).
    pub(super) attr: Attr,
    /// Byte length of the whole tag, up to and including the closing `>`.
    pub(super) len: usize,
    /// Whether the tag closes itself (`<div/>`), so it opens no element to balance.
    pub(super) self_closing: bool,
}

/// A located close tag within a line.
pub(super) struct CloseTag {
    /// Byte offset where `</` begins.
    pub(super) start: usize,
    /// Byte offset just past the closing `>`.
    pub(super) end: usize,
}

/// Block-level tag names whose elements carry parsed markdown content. Inline tags (`em`, `span`,
/// `a`, …) and unrecognized names are left for the inline phase as raw HTML.
const BLOCK_TAGS: &[&str] = &[
    "address",
    "article",
    "aside",
    "base",
    "basefont",
    "blockquote",
    "body",
    "caption",
    "center",
    "col",
    "colgroup",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "frame",
    "frameset",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hr",
    "html",
    "iframe",
    "legend",
    "li",
    "link",
    "main",
    "menu",
    "menuitem",
    "nav",
    "noframes",
    "ol",
    "optgroup",
    "option",
    "p",
    "param",
    "search",
    "section",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "title",
    "tr",
    "track",
    "ul",
];

fn is_block_tag(name: &str) -> bool {
    BLOCK_TAGS.contains(&name)
}

/// If `s` begins with a recognized block-level HTML open tag, return its name, attributes, and
/// byte extent. A self-closing tag (`<div/>`) parses as an ordinary open tag here.
pub(super) fn parse_open_tag(s: &str) -> Option<OpenTag> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'<') {
        return None;
    }
    let mut i = 1;
    let name_start = i;
    if !bytes.get(i).is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    i += 1;
    while bytes
        .get(i)
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-')
    {
        i += 1;
    }
    let name = s.get(name_start..i)?.to_ascii_lowercase();
    if !is_block_tag(&name) {
        return None;
    }
    let mut attr = Attr::default();
    loop {
        let after_ws = skip_ws(bytes, i);
        // A `>` (optionally preceded by a self-closing `/`) ends the tag.
        let self_closing = bytes.get(after_ws) == Some(&b'/');
        let close = if self_closing { after_ws + 1 } else { after_ws };
        if bytes.get(close) == Some(&b'>') {
            return Some(OpenTag {
                tag: name,
                attr,
                len: close + 1,
                self_closing,
            });
        }
        // An attribute must be separated from the name (or a previous attribute) by whitespace.
        if after_ws == i {
            return None;
        }
        i = read_attribute(bytes, after_ws, &mut attr)?;
    }
}

/// Read one `name[=value]` attribute starting at `start`, folding it into `attr`, and return the
/// index just past it. `id` sets the identifier, `class` adds whitespace-separated classes (the
/// first `class` wins), and any other name becomes a key/value pair in source order.
fn read_attribute(bytes: &[u8], start: usize, attr: &mut Attr) -> Option<usize> {
    let mut i = start;
    let name_start = i;
    if !bytes
        .get(i)
        .is_some_and(|b| b.is_ascii_alphabetic() || matches!(b, b'_' | b':'))
    {
        return None;
    }
    i += 1;
    while bytes
        .get(i)
        .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-'))
    {
        i += 1;
    }
    let name = ascii_lower(bytes.get(name_start..i)?);
    let probe = skip_ws(bytes, i);
    let mut value = String::new();
    let mut end = i;
    if bytes.get(probe) == Some(&b'=') {
        let (val, next) = read_value(bytes, probe + 1)?;
        value = val;
        end = next;
    }
    match name.as_str() {
        "id" => attr.id = value.into(),
        "class" => {
            if attr.classes.is_empty() {
                attr.classes = value.split_whitespace().map(Into::into).collect();
            }
        }
        _ => attr.attributes.push((name.into(), value.into())),
    }
    Some(end)
}

/// Read an attribute value (quoted or bare) starting at `start`, returning it and the index just
/// past it. A started-but-unterminated value is malformed.
fn read_value(bytes: &[u8], start: usize) -> Option<(String, usize)> {
    let i = skip_ws(bytes, start);
    match bytes.get(i) {
        Some(quote @ (b'"' | b'\'')) => {
            let quote = *quote;
            let value_start = i + 1;
            let mut j = value_start;
            while bytes.get(j).is_some_and(|b| *b != quote) {
                j += 1;
            }
            if bytes.get(j) != Some(&quote) {
                return None;
            }
            Some((bytes_to_string(bytes.get(value_start..j)?), j + 1))
        }
        Some(_) => {
            let value_start = i;
            let mut j = i;
            while bytes.get(j).is_some_and(|b| {
                !matches!(b, b' ' | b'\t' | b'"' | b'\'' | b'=' | b'<' | b'>' | b'`')
            }) {
                j += 1;
            }
            if j == value_start {
                return None;
            }
            Some((bytes_to_string(bytes.get(value_start..j)?), j))
        }
        None => None,
    }
}

/// Locates the close tag that balances an open element, ignoring nested tags and literal content.
pub(super) fn find_close_tag(s: &str, tag: &str) -> Option<CloseTag> {
    let (_, end) = scan_depth(s, tag, 1, &mut None);
    let end = end?;
    let start = s.get(..end)?.rfind('<')?;
    Some(CloseTag { start, end })
}

/// If a close tag for `tag` begins at `start`, return the index just past its `>`.
fn close_tag_at(bytes: &[u8], start: usize, tag: &str) -> Option<usize> {
    let name_start = start + 2;
    let mut i = name_start;
    while bytes
        .get(i)
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-')
    {
        i += 1;
    }
    let name = ascii_lower(bytes.get(name_start..i)?);
    if name != tag {
        return None;
    }
    i = skip_ws(bytes, i);
    (bytes.get(i) == Some(&b'>')).then_some(i + 1)
}

/// If `s` begins with a block-level close tag (`</div>`, optional whitespace before `>`), return
/// its byte length. A bare close tag at a line start stands alone as a raw block.
pub(super) fn parse_close_tag(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'<') || bytes.get(1) != Some(&b'/') {
        return None;
    }
    let name_start = 2;
    let mut i = name_start;
    while bytes
        .get(i)
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-')
    {
        i += 1;
    }
    let name = s.get(name_start..i)?.to_ascii_lowercase();
    if !is_block_tag(&name) && !matches!(name.as_str(), "pre" | "script" | "style" | "textarea") {
        return None;
    }
    i = skip_ws(bytes, i);
    (bytes.get(i) == Some(&b'>')).then_some(i + 1)
}

/// Counts nested tags and returns the end of the tag that closes the outer element.
/// Pass `literal` between lines to keep literal HTML content out of the tag count.
pub(super) fn scan_depth(
    s: &str,
    tag: &str,
    mut depth: usize,
    literal: &mut Option<HtmlLiteral>,
) -> (usize, Option<usize>) {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if let Some(active) = literal {
            let (terminator, ignore_case) = match active {
                HtmlLiteral::Comment => ("-->", false),
                HtmlLiteral::ProcessingInstruction => ("?>", false),
                HtmlLiteral::Cdata => ("]]>", false),
                HtmlLiteral::Pre => ("</pre>", true),
                HtmlLiteral::Script => ("</script>", true),
                HtmlLiteral::Style => ("</style>", true),
                HtmlLiteral::Textarea => ("</textarea>", true),
            };
            let rest = s.get(i..).unwrap_or("");
            let end = if ignore_case {
                rest.as_bytes()
                    .windows(terminator.len())
                    .position(|candidate| candidate.eq_ignore_ascii_case(terminator.as_bytes()))
            } else {
                rest.find(terminator)
            };
            let Some(end) = end else {
                break;
            };
            i += end + terminator.len();
            *literal = None;
            continue;
        }
        if bytes.get(i) == Some(&b'<') {
            let rest = s.get(i..).unwrap_or("");
            let opening = if rest.starts_with("<!--") {
                Some(HtmlLiteral::Comment)
            } else if rest.starts_with("<?") {
                Some(HtmlLiteral::ProcessingInstruction)
            } else if rest.starts_with("<![CDATA[") {
                Some(HtmlLiteral::Cdata)
            } else {
                None
            };
            if let Some(opening) = opening {
                if let Some(end) = super::scan::scan_html_tag_end(s, i) {
                    i = end;
                    continue;
                }
                *literal = Some(opening);
                break;
            }
            if let Some(name) = super::html_block::raw_text_tag(rest) {
                *literal = match name {
                    "pre" => Some(HtmlLiteral::Pre),
                    "script" => Some(HtmlLiteral::Script),
                    "style" => Some(HtmlLiteral::Style),
                    "textarea" => Some(HtmlLiteral::Textarea),
                    _ => None,
                };
                i = super::scan::scan_html_tag_end(s, i).unwrap_or(bytes.len());
                continue;
            }
            if let Some(open) = parse_open_tag(rest) {
                if open.tag == tag && !open.self_closing {
                    depth += 1;
                }
                i += open.len;
                continue;
            }
            if let Some(end) = close_tag_at(bytes, i, tag) {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return (0, Some(end));
                }
                i = end;
                continue;
            }
            if let Some(end) = super::scan::scan_html_tag_end(s, i) {
                i = end;
                continue;
            }
        }
        i += 1;
    }
    (depth, None)
}

fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
    while bytes.get(i).is_some_and(|b| matches!(b, b' ' | b'\t')) {
        i += 1;
    }
    i
}

fn ascii_lower(bytes: &[u8]) -> String {
    bytes_to_string(bytes).to_ascii_lowercase()
}

fn bytes_to_string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
