//! HTML block recognition per `CommonMark` §4.6: classify the seven HTML block types at a line
//! start and decide where each one ends. Pure functions over the raw line text; the block phase
//! drives them and owns the surrounding open-block bookkeeping.

/// If `rest` (the line from its first non-space) begins an HTML block, return its type (1–7). The
/// cursor positions itself at the first non-space before calling. Type 7 cannot interrupt a
/// paragraph.
pub(super) fn classify(rest: &str, can_interrupt_paragraph: bool) -> Option<u8> {
    if !rest.starts_with('<') {
        return None;
    }
    if rest.starts_with("<!--") {
        return Some(2);
    }
    if rest.starts_with("<?") {
        return Some(3);
    }
    if rest.starts_with("<![CDATA[") {
        return Some(5);
    }
    if rest
        .strip_prefix("<!")
        .is_some_and(|after| after.starts_with(|c: char| c.is_ascii_alphabetic()))
    {
        return Some(4);
    }
    let lower = rest.to_ascii_lowercase();
    for tag in ["script", "pre", "style", "textarea"] {
        if let Some(after) = lower.strip_prefix('<').and_then(|r| r.strip_prefix(tag))
            && (after.is_empty() || after.starts_with([' ', '\t', '>']))
        {
            return Some(1);
        }
    }
    if is_type6_start(rest) {
        return Some(6);
    }
    if can_interrupt_paragraph
        && let Some(len) = scan_complete_tag(rest)
        && rest.get(len..).is_some_and(|tail| tail.trim().is_empty())
    {
        return Some(7);
    }
    None
}

/// Whether `line` satisfies the end condition for an HTML block of the given type. Types 6 and 7
/// end at a blank line instead and are handled by the caller.
pub(super) fn closes(kind: u8, line: &str) -> bool {
    match kind {
        1 => {
            let lower = line.to_ascii_lowercase();
            ["</script>", "</pre>", "</style>", "</textarea>"]
                .iter()
                .any(|needle| lower.contains(needle))
        }
        2 => line.contains("-->"),
        3 => line.contains("?>"),
        4 => line.contains('>'),
        5 => line.contains("]]>"),
        _ => false,
    }
}

/// Tag names that begin an HTML block of type 6 (terminated by a blank line).
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

/// Whether the bytes at `s` begin an HTML block of type 6 (`<`/`</` + block tag name + boundary).
fn is_type6_start(s: &str) -> bool {
    let after = s
        .strip_prefix("</")
        .or_else(|| s.strip_prefix('<'))
        .unwrap_or("");
    let name_len = after.bytes().take_while(u8::is_ascii_alphanumeric).count();
    let Some(name) = after.get(..name_len) else {
        return false;
    };
    if name.is_empty() {
        return false;
    }
    let tail = after.get(name_len..).unwrap_or("");
    let boundary = tail.is_empty() || tail.starts_with([' ', '\t', '>']) || tail.starts_with("/>");
    boundary && BLOCK_TAGS.contains(&name.to_ascii_lowercase().as_str())
}

/// Length in bytes of a complete HTML open or closing tag at the start of `s`, if any. Used for
/// HTML block type 7, which requires a complete tag spanning the rest of the line.
fn scan_complete_tag(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'<') {
        return None;
    }
    if bytes.get(1) == Some(&b'/') {
        let mut index = 2;
        if !bytes.get(index).is_some_and(u8::is_ascii_alphabetic) {
            return None;
        }
        index += 1;
        while bytes
            .get(index)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-')
        {
            index += 1;
        }
        while bytes.get(index).is_some_and(|b| matches!(b, b' ' | b'\t')) {
            index += 1;
        }
        return (bytes.get(index) == Some(&b'>')).then_some(index + 1);
    }
    let mut index = 1;
    if !bytes.get(index).is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    index += 1;
    while bytes
        .get(index)
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-')
    {
        index += 1;
    }
    loop {
        let mut whitespace = 0;
        while bytes.get(index).is_some_and(|b| matches!(b, b' ' | b'\t')) {
            index += 1;
            whitespace += 1;
        }
        let name_ok = bytes
            .get(index)
            .is_some_and(|b| b.is_ascii_alphabetic() || matches!(b, b'_' | b':'));
        if whitespace == 0 || !name_ok {
            index -= whitespace;
            break;
        }
        index += 1;
        while bytes
            .get(index)
            .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-'))
        {
            index += 1;
        }
        index = scan_optional_attribute_value(bytes, index)?;
    }
    while bytes.get(index).is_some_and(|b| matches!(b, b' ' | b'\t')) {
        index += 1;
    }
    if bytes.get(index) == Some(&b'/') {
        index += 1;
    }
    (bytes.get(index) == Some(&b'>')).then_some(index + 1)
}

/// Consume an optional `= value` attribute tail; returns the new index, or `None` if a value is
/// started but malformed (unterminated quote / empty unquoted value).
fn scan_optional_attribute_value(bytes: &[u8], start: usize) -> Option<usize> {
    let mut probe = start;
    while bytes.get(probe).is_some_and(|b| matches!(b, b' ' | b'\t')) {
        probe += 1;
    }
    if bytes.get(probe) != Some(&b'=') {
        return Some(start);
    }
    probe += 1;
    while bytes.get(probe).is_some_and(|b| matches!(b, b' ' | b'\t')) {
        probe += 1;
    }
    match bytes.get(probe) {
        Some(quote @ (b'"' | b'\'')) => {
            let quote = *quote;
            probe += 1;
            while bytes.get(probe).is_some_and(|b| *b != quote) {
                probe += 1;
            }
            (bytes.get(probe) == Some(&quote)).then(|| probe + 1)
        }
        Some(_) => {
            let value_start = probe;
            while bytes.get(probe).is_some_and(|b| {
                !matches!(b, b' ' | b'\t' | b'"' | b'\'' | b'=' | b'<' | b'>' | b'`')
            }) {
                probe += 1;
            }
            (probe > value_start).then_some(probe)
        }
        None => None,
    }
}
