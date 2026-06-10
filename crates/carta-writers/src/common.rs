//! Shared helpers for the text-oriented writers: the default fill column, the greedy line-filling
//! engine, column-width measurement, list-tightness, ordered-list numerals and delimiter wrapping,
//! the smart-quote glyphs, URI-scheme recognition, and HTML attribute and entity helpers.
//!
//! Each consumer is behind its own writer feature, so which helpers are live depends on the enabled
//! features: a build with only one writer leaves the others' helpers unreferenced. That is expected
//! for this toolbox, so unused-item warnings are allowed here rather than gated per item.
#![allow(dead_code)]

use carta_ast::{Attr, Block, Inline, ListNumberDelim, ListNumberStyle, QuoteType};

/// Column at which inline content is wrapped: the default fill width.
pub(crate) const FILL_COLUMN: usize = 72;

/// The open and close smart-quote glyphs for a quote kind.
pub(crate) fn quote_marks(kind: &QuoteType) -> (char, char) {
    match kind {
        QuoteType::SingleQuote => ('\u{2018}', '\u{2019}'),
        QuoteType::DoubleQuote => ('\u{201c}', '\u{201d}'),
    }
}

/// A unit of inline content awaiting line filling: an unbreakable text run, a breakable space, or a
/// forced line break.
#[derive(Debug, Clone)]
pub(crate) enum Piece {
    Text(String),
    Space,
    Hard,
}

/// Greedily fill inline pieces to `width` columns: a breakable space becomes a line break when
/// keeping the next word on the current line would exceed the fill column. Consecutive text runs (no
/// intervening space) stay together; runs of spaces collapse; leading and trailing spaces on a line
/// are dropped.
pub(crate) fn fill(pieces: &[Piece], width: usize) -> String {
    fill_offset(pieces, width, 0)
}

/// Like [`fill`], but the first line is laid out as if `initial` columns were already consumed (the
/// hanging-marker layout, where a leading marker shifts the first line's wrap point but leaves
/// continuation lines at the margin).
pub(crate) fn fill_offset(pieces: &[Piece], width: usize, initial: usize) -> String {
    let width = width.max(1);
    let mut out = String::new();
    let mut column = initial;
    let mut at_line_start = initial == 0;
    let mut pending_space = false;
    // Consecutive text pieces (no intervening space or break) form one unbreakable word, gathered
    // here as borrowed runs and placed only once its full width is known.
    let mut word: Vec<&str> = Vec::new();
    let mut word_width = 0;
    for piece in pieces {
        match piece {
            Piece::Text(text) => {
                word.push(text);
                word_width += display_width(text);
            }
            Piece::Space => {
                place_word(
                    &mut out,
                    &mut column,
                    &mut at_line_start,
                    pending_space,
                    &word,
                    word_width,
                    width,
                );
                word.clear();
                word_width = 0;
                pending_space = true;
            }
            Piece::Hard => {
                place_word(
                    &mut out,
                    &mut column,
                    &mut at_line_start,
                    pending_space,
                    &word,
                    word_width,
                    width,
                );
                word.clear();
                word_width = 0;
                if !at_line_start {
                    out.push('\n');
                    column = 0;
                    at_line_start = true;
                }
                pending_space = false;
            }
        }
    }
    place_word(
        &mut out,
        &mut column,
        &mut at_line_start,
        pending_space,
        &word,
        word_width,
        width,
    );
    out.trim_end_matches('\n').to_owned()
}

/// Place a gathered word onto the current line, inserting a line break in place of the preceding
/// space when keeping the word would overflow `width`. A no-op for an empty word.
fn place_word(
    out: &mut String,
    column: &mut usize,
    at_line_start: &mut bool,
    pending_space: bool,
    word: &[&str],
    word_width: usize,
    width: usize,
) {
    if word.is_empty() {
        return;
    }
    if *at_line_start {
        *at_line_start = false;
    } else if pending_space && *column + 1 + word_width > width {
        out.push('\n');
        *column = 0;
        *at_line_start = false;
    } else if pending_space {
        out.push(' ');
        *column += 1;
    }
    for part in word {
        out.push_str(part);
    }
    *column += word_width;
}

/// Apply `first` to the first line and `rest` to each non-empty later line, leaving blank lines
/// (block separators) unprefixed. This produces a hanging indent: a list marker plus continuation
/// indent, or a uniform block-quote / code prefix.
pub(crate) fn indent_block(body: &str, first: &str, rest: &str) -> String {
    let mut out = String::new();
    for (index, line) in body.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if index == 0 {
            out.push_str(first);
            out.push_str(line);
        } else if !line.is_empty() {
            out.push_str(rest);
            out.push_str(line);
        }
    }
    out
}

/// Whether a list is tight: every item is empty or opens with a [`Block::Plain`].
pub(crate) fn list_is_tight(items: &[Vec<Block>]) -> bool {
    items
        .iter()
        .all(|item| matches!(item.first(), None | Some(Block::Plain(_))))
}

/// A text writer that gathers footnotes inline and emits them as a trailing section. Each note is
/// referenced by a numbered `[n]` marker; its body is rendered offset so the marker shifts only the
/// first line's wrap point. The format supplies how a block and a marker-offset leading paragraph
/// render; the marker numbering and slot bookkeeping are shared here.
pub(crate) trait NotesHost {
    /// The accumulated note bodies, indexed by note number minus one.
    fn notes(&mut self) -> &mut Vec<String>;

    /// Render a block at the given fill width.
    fn render_block(&mut self, block: &Block, width: usize) -> String;

    /// Render a leading paragraph's text with its first line beginning `initial` columns in.
    fn render_offset_paragraph(
        &mut self,
        inlines: &[Inline],
        width: usize,
        initial: usize,
    ) -> String;

    /// Record a footnote: reserve its slot before rendering (so nested notes number after it), fill
    /// the slot with the assembled body, and return the inline `[n]` marker.
    fn record_note(&mut self, blocks: &[Block]) -> String {
        let index = self.notes().len();
        self.notes().push(String::new());
        let marker = format!("[{}]", index + 1);
        let field = marker.chars().count() + 1;
        let body = self.note_body(blocks, field);
        // The body shares the marker's line only when it opens with a paragraph; a leading block of
        // any other kind (a code block, a list) begins on the line below the marker.
        let starts_inline = matches!(blocks.first(), Some(Block::Plain(_) | Block::Para(_)));
        let rendered = if body.is_empty() {
            marker.clone()
        } else if starts_inline {
            format!("{marker} {body}")
        } else {
            format!("{marker}\n{body}")
        };
        if let Some(slot) = self.notes().get_mut(index) {
            *slot = rendered;
        }
        marker
    }

    /// Render a footnote's body: the first block's opening line is offset by the marker width, every
    /// later block and continuation line sits at the margin.
    fn note_body(&mut self, blocks: &[Block], initial: usize) -> String {
        let rendered = blocks
            .iter()
            .enumerate()
            .map(|(position, block)| {
                let is_plain = matches!(block, Block::Plain(_));
                let text = if position == 0 {
                    self.note_block_offset(block, FILL_COLUMN, initial)
                } else {
                    self.render_block(block, FILL_COLUMN)
                };
                (is_plain, text)
            })
            .collect();
        join_loose(rendered)
    }

    /// Render a block whose first line begins `initial` columns in. Only a leading paragraph wraps,
    /// so the offset is meaningful for it alone; other block kinds render at the margin.
    fn note_block_offset(&mut self, block: &Block, width: usize, initial: usize) -> String {
        match block {
            Block::Plain(inlines) | Block::Para(inlines) => {
                self.render_offset_paragraph(inlines, width, initial)
            }
            other => self.render_block(other, width),
        }
    }
}

/// Append a gathered footnote section to a rendered body, separated by a blank line, and trim the
/// trailing newlines. With no notes this just trims the body.
pub(crate) fn append_notes(body: String, notes: &[String]) -> String {
    let mut out = body;
    if !notes.is_empty() {
        let section = notes.join("\n\n");
        out = if out.is_empty() {
            section
        } else {
            format!("{out}\n\n{section}")
        };
    }
    out.trim_end_matches('\n').to_owned()
}

/// Whether a list is loose — at least one item carries a top-level paragraph. A loose list's items
/// are separated with a blank line and each item's blocks are laid out with blank lines; a tight
/// list uses single newlines throughout.
pub(crate) fn is_loose(items: &[Vec<Block>]) -> bool {
    !list_is_tight(items)
}

/// The separator between two list items at the given layout density: a blank line when loose, a
/// single newline when tight.
pub(crate) fn item_separator(loose: bool) -> &'static str {
    if loose { "\n\n" } else { "\n" }
}

/// Join already-rendered blocks with the document's default blank-line spacing, dropping blocks that
/// produced no output. A [`Block::Plain`] contributes only a single newline (not a blank line)
/// before the next visible block when an empty block falls between them.
pub(crate) fn join_loose(rendered: Vec<(bool, String)>) -> String {
    let mut out = String::new();
    let mut previous_was_plain: Option<bool> = None;
    let mut empty_since_previous = false;
    for (is_plain, text) in rendered {
        if text.is_empty() {
            if previous_was_plain.is_some() {
                empty_since_previous = true;
            }
            continue;
        }
        if let Some(was_plain) = previous_was_plain {
            if was_plain && empty_since_previous {
                out.push('\n');
            } else {
                out.push_str("\n\n");
            }
        }
        out.push_str(&text);
        previous_was_plain = Some(is_plain);
        empty_since_previous = false;
    }
    out
}

/// Wrap an ordered-list numeral in its delimiter: `n.`, `n)`, or `(n)`.
pub(crate) fn wrap_delim(numeral: &str, delim: &ListNumberDelim) -> String {
    match delim {
        ListNumberDelim::DefaultDelim | ListNumberDelim::Period => format!("{numeral}."),
        ListNumberDelim::OneParen => format!("{numeral})"),
        ListNumberDelim::TwoParens => format!("({numeral})"),
    }
}

/// Display width of a string in columns, summed over its characters.
pub(crate) fn display_width(text: &str) -> usize {
    text.chars().map(char_width).sum()
}

/// Display width of a character: zero for common combining marks and controls, two for wide East
/// Asian characters, one otherwise. A self-contained column-width approximation.
pub(crate) fn char_width(ch: char) -> usize {
    let code = ch as u32;
    if is_control(code) {
        return 0;
    }
    if code < 0x0300 {
        return 1;
    }
    if is_zero_width(code) {
        return 0;
    }
    if is_wide(code) { 2 } else { 1 }
}

/// C0 controls, DEL, and C1 controls occupy no display columns.
fn is_control(code: u32) -> bool {
    code < 0x20 || (0x7F..=0x9F).contains(&code)
}

fn is_zero_width(code: u32) -> bool {
    matches!(code,
        0x0300..=0x036F
        | 0x0483..=0x0489
        | 0x0591..=0x05BD
        | 0x0610..=0x061A
        | 0x064B..=0x065F
        | 0x0670
        | 0x06D6..=0x06DC
        | 0x06DF..=0x06E4
        | 0x0E31
        | 0x0E34..=0x0E3A
        | 0x1AB0..=0x1AFF
        | 0x1DC0..=0x1DFF
        | 0x200B..=0x200F
        | 0x20D0..=0x20FF
        | 0xFE00..=0xFE0F
        | 0xFE20..=0xFE2F
    )
}

/// Whether a character occupies two display columns: the wide and fullwidth East Asian ranges.
pub(crate) fn is_wide(code: u32) -> bool {
    matches!(code,
        0x1100..=0x115F
        | 0x2329 | 0x232A
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE19
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1B000..=0x1B2FF
        | 0x1F200..=0x1F2FF
        | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x3FFFD
    )
}

/// Convert a zero-based item offset to the signed step added to a list's start number, saturating an
/// out-of-range offset rather than overflowing.
pub(crate) fn offset_as_i32(offset: usize) -> i32 {
    i32::try_from(offset).unwrap_or(i32::MAX)
}

/// The leading marker for an ordered-list item: its number in the list's numeral style, wrapped in
/// the list's delimiter.
pub(crate) fn ordered_marker(
    number: i32,
    style: &ListNumberStyle,
    delim: &ListNumberDelim,
) -> String {
    wrap_delim(&numeral(number, style), delim)
}

/// Render a number in a list's numeral style.
pub(crate) fn numeral(number: i32, style: &ListNumberStyle) -> String {
    match style {
        ListNumberStyle::DefaultStyle | ListNumberStyle::Decimal | ListNumberStyle::Example => {
            number.to_string()
        }
        ListNumberStyle::LowerAlpha => alpha(number, false),
        ListNumberStyle::UpperAlpha => alpha(number, true),
        ListNumberStyle::LowerRoman => roman(number, false),
        ListNumberStyle::UpperRoman => roman(number, true),
    }
}

/// Bijective base-26 alphabetic numeral (1 -> a, 26 -> z, 27 -> aa). Non-positive input falls back
/// to the decimal form, which cannot be expressed as a letter.
pub(crate) fn alpha(number: i32, upper: bool) -> String {
    if number < 1 {
        return number.to_string();
    }
    let base = if upper { b'A' } else { b'a' };
    let mut value = number;
    let mut letters = Vec::new();
    while value > 0 {
        let remainder = (value - 1) % 26;
        letters.push(base + u8::try_from(remainder).unwrap_or(0));
        value = (value - 1) / 26;
    }
    letters.reverse();
    String::from_utf8(letters).unwrap_or_else(|_| number.to_string())
}

/// Roman numeral for a positive number; non-positive input falls back to the decimal form.
pub(crate) fn roman(number: i32, upper: bool) -> String {
    const UNITS: [(i32, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    if number < 1 {
        return number.to_string();
    }
    let mut remaining = number;
    let mut out = String::new();
    for (value, symbol) in UNITS {
        while remaining >= value {
            out.push_str(symbol);
            remaining -= value;
        }
    }
    if upper { out.to_uppercase() } else { out }
}

/// Look up a key/value attribute by key, returning its value.
pub(crate) fn attribute_value<'a>(attr: &'a Attr, key: &str) -> Option<&'a str> {
    attr.attributes
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

/// Whether a string is syntactically a URI scheme: an ASCII letter followed by ASCII letters,
/// digits, or any of `+`, `-`, `.`.
pub(crate) fn is_uri_scheme(scheme: &str) -> bool {
    let mut chars = scheme.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
}

/// Whether `scheme` names a registered URI scheme (compared case-insensitively), per the
/// [`URI_SCHEMES`] registry. Used to decide whether an address may render as a bare autolink.
pub(crate) fn is_known_scheme(scheme: &str) -> bool {
    let lowered = scheme.to_ascii_lowercase();
    URI_SCHEMES.binary_search(&lowered.as_str()).is_ok()
}

/// Registered URI schemes, lowercased and sorted for binary search. Drawn from the IANA scheme
/// registry; the autolink-capable writers share this one set so their recognition cannot drift.
const URI_SCHEMES: &[&str] = &[
    "aaa",
    "aaas",
    "about",
    "acap",
    "acct",
    "acr",
    "adiumxtra",
    "admin",
    "afp",
    "afs",
    "aim",
    "app",
    "appdata",
    "apt",
    "attachment",
    "aw",
    "barion",
    "beshare",
    "bitcoin",
    "bitcoincash",
    "blob",
    "bolo",
    "browserext",
    "bzr",
    "callto",
    "cap",
    "chrome",
    "chrome-extension",
    "cid",
    "coap",
    "coaps",
    "com-eventbrite-attendee",
    "content",
    "crid",
    "cvs",
    "data",
    "dav",
    "dict",
    "did",
    "dis",
    "dlna-playcontainer",
    "dlna-playsingle",
    "dns",
    "dntp",
    "doi",
    "dtn",
    "dvb",
    "ed2k",
    "ethereum",
    "example",
    "facetime",
    "fax",
    "feed",
    "feedready",
    "file",
    "filesystem",
    "finger",
    "fish",
    "ftp",
    "geo",
    "gg",
    "git",
    "gizmoproject",
    "go",
    "gopher",
    "graph",
    "gtalk",
    "h323",
    "ham",
    "hcp",
    "http",
    "https",
    "hxxp",
    "hxxps",
    "hydrazone",
    "iax",
    "icap",
    "icon",
    "im",
    "imap",
    "info",
    "iotdisco",
    "ipn",
    "ipp",
    "ipps",
    "irc",
    "irc6",
    "ircs",
    "iris",
    "iris.beep",
    "iris.lwz",
    "iris.xpc",
    "iris.xpcs",
    "isostore",
    "itms",
    "jabber",
    "jar",
    "jms",
    "keyparc",
    "lastfm",
    "ldap",
    "ldaps",
    "lvlt",
    "magnet",
    "mailserver",
    "mailto",
    "maps",
    "market",
    "matrix",
    "message",
    "mid",
    "mms",
    "modem",
    "monero",
    "mongodb",
    "moz",
    "ms-access",
    "ms-browser-extension",
    "ms-drive-to",
    "ms-enrollment",
    "ms-excel",
    "ms-gamebarservices",
    "ms-getoffice",
    "ms-help",
    "ms-infopath",
    "ms-media-stream-id",
    "ms-officeapp",
    "ms-powerpoint",
    "ms-project",
    "ms-publisher",
    "ms-search-repair",
    "ms-secondary-screen-controller",
    "ms-secondary-screen-setup",
    "ms-settings",
    "ms-settings-airplanemode",
    "ms-settings-bluetooth",
    "ms-settings-camera",
    "ms-settings-cellular",
    "ms-settings-cloudstorage",
    "ms-settings-connectabledevices",
    "ms-settings-displays-topology",
    "ms-settings-emailandaccounts",
    "ms-settings-language",
    "ms-settings-location",
    "ms-settings-lock",
    "ms-settings-nfctransactions",
    "ms-settings-notifications",
    "ms-settings-power",
    "ms-settings-privacy",
    "ms-settings-proximity",
    "ms-settings-screenrotation",
    "ms-settings-wifi",
    "ms-settings-workplace",
    "ms-spd",
    "ms-sttoverlay",
    "ms-transit-to",
    "ms-virtualtouchpad",
    "ms-visio",
    "ms-walk-to",
    "ms-whiteboard",
    "ms-whiteboard-cmd",
    "ms-word",
    "msnim",
    "msrp",
    "msrps",
    "mtqp",
    "mumble",
    "mupdate",
    "mvn",
    "mvrp",
    "news",
    "nfs",
    "ni",
    "nih",
    "nntp",
    "notes",
    "ocf",
    "oid",
    "onenote",
    "onenote-cmd",
    "opaquelocktoken",
    "pack",
    "palm",
    "paparazzi",
    "payto",
    "pkcs11",
    "platform",
    "pop",
    "pres",
    "prospero",
    "proxy",
    "psyc",
    "pwid",
    "qb",
    "query",
    "redis",
    "rediss",
    "reload",
    "res",
    "resource",
    "rmi",
    "rsync",
    "rtmfp",
    "rtmp",
    "rtsp",
    "rtsps",
    "rtspu",
    "secondlife",
    "service",
    "session",
    "sftp",
    "sgn",
    "shttp",
    "sieve",
    "sip",
    "sips",
    "skype",
    "smb",
    "sms",
    "smtp",
    "snews",
    "snmp",
    "soap.beep",
    "soap.beeps",
    "soldat",
    "spotify",
    "ssh",
    "steam",
    "stun",
    "stuns",
    "submit",
    "svn",
    "tag",
    "teamspeak",
    "tel",
    "teliaeid",
    "telnet",
    "tftp",
    "things",
    "thismessage",
    "tip",
    "tn3270",
    "tool",
    "turn",
    "turns",
    "tv",
    "udp",
    "unreal",
    "urn",
    "ut2004",
    "v-event",
    "vemmi",
    "ventrilo",
    "view-source",
    "vnc",
    "wais",
    "webcal",
    "wpid",
    "ws",
    "wss",
    "wtai",
    "wyciwyg",
    "xcon",
    "xcon-userid",
    "xfire",
    "xmlrpc.beep",
    "xmlrpc.beeps",
    "xmpp",
    "xri",
    "ymsgr",
    "z39.50",
    "z39.50r",
    "z39.50s",
];

/// Whether `text` is made up solely of URI-permitted characters with every `%` introducing a
/// two-digit hex escape. ASCII alphanumerics and the unreserved, sub-delimiter, and generic-delimiter
/// punctuation are permitted; non-ASCII characters are permitted only when `allow_non_ascii` is set.
pub(crate) fn is_percent_escaped_uri(text: &str, allow_non_ascii: bool) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    while let Some(&ch) = chars.get(index) {
        if ch == '%' {
            let two_hex = chars.get(index + 1).is_some_and(char::is_ascii_hexdigit)
                && chars.get(index + 2).is_some_and(char::is_ascii_hexdigit);
            if !two_hex {
                return false;
            }
            index += 3;
            continue;
        }
        if !is_uri_char(ch, allow_non_ascii) {
            return false;
        }
        index += 1;
    }
    true
}

fn is_uri_char(ch: char, allow_non_ascii: bool) -> bool {
    if !ch.is_ascii() {
        return allow_non_ascii;
    }
    ch.is_ascii_alphanumeric()
        || matches!(
            ch,
            '-' | '.'
                | '_'
                | '~'
                | ':'
                | '/'
                | '?'
                | '#'
                | '@'
                | '!'
                | '$'
                | '&'
                | '\''
                | '('
                | ')'
                | '*'
                | '+'
                | ','
                | ';'
                | '='
        )
}

/// Escape the XML/HTML metacharacters `&`, `<`, and `>` to their entities, and additionally `"` when
/// `escape_quotes` is set (as in an attribute value).
pub(crate) fn escape_xml(text: &str, escape_quotes: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if escape_quotes => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
}

/// Escape an HTML attribute value: `&`, `<`, `>`, and `"` to their entities.
pub(crate) fn escape_attr(text: &str) -> String {
    escape_xml(text, true)
}

/// Resolves each table cell's true starting column within one row group, accounting for cells from
/// earlier rows that still cover columns through their row span. Create one tracker per group of
/// rows a span can extend over (a table head, a body's own head rows, a body's rows, a foot).
#[cfg(any(feature = "html", feature = "mediawiki"))]
#[derive(Debug)]
pub(crate) struct RowSpanGrid {
    /// Per column, how many upcoming rows a span opened in an earlier row still covers.
    pending: Vec<i32>,
}

#[cfg(any(feature = "html", feature = "mediawiki"))]
impl RowSpanGrid {
    pub(crate) fn new(columns: usize) -> Self {
        Self {
            pending: vec![0; columns],
        }
    }

    /// Place one row's cells: each cell lands on the first column not covered from above and
    /// occupies its column span, and its row span is recorded for the rows that follow. Returns
    /// each cell paired with its starting column.
    pub(crate) fn place<'cells>(
        &mut self,
        cells: &'cells [carta_ast::Cell],
    ) -> Vec<(usize, &'cells carta_ast::Cell)> {
        let covered: Vec<usize> = self
            .pending
            .iter()
            .enumerate()
            .filter(|(_, rows)| **rows > 0)
            .map(|(column, _)| column)
            .collect();
        let mut placed = Vec::with_capacity(cells.len());
        let mut column = 0_usize;
        for cell in cells {
            while self.pending.get(column).copied().unwrap_or(0) > 0 {
                column = column.saturating_add(1);
            }
            placed.push((column, cell));
            let col_span = usize::try_from(cell.col_span).unwrap_or(1).max(1);
            let end = column.saturating_add(col_span);
            if self.pending.len() < end {
                self.pending.resize(end, 0);
            }
            for slot in self.pending.iter_mut().take(end).skip(column) {
                *slot = cell.row_span.saturating_sub(1).max(0);
            }
            column = end;
        }
        for column in covered {
            if let Some(rows) = self.pending.get_mut(column) {
                *rows -= 1;
            }
        }
        placed
    }
}

/// Render an [`Attr`] to an HTML attribute string (a leading space per attribute, empty when blank):
/// `id`, then `class`, then key/value pairs, with unrecognized keys `data-` prefixed.
pub(crate) fn render_html_attr(attr: &Attr) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    if !attr.id.is_empty() {
        let _ = write!(out, " id=\"{}\"", escape_attr(&attr.id));
    }
    if !attr.classes.is_empty() {
        let _ = write!(out, " class=\"{}\"", escape_attr(&attr.classes.join(" ")));
    }
    for (key, value) in &attr.attributes {
        let name = if is_known_attribute(key) {
            key.clone()
        } else {
            format!("data-{key}")
        };
        let _ = write!(out, " {name}=\"{}\"", escape_attr(value));
    }
    out
}

/// Whether an attribute name is emitted verbatim in HTML output. Recognized names, the `data-`/`aria-`
/// prefixes, and a few namespaced names pass through; any other key/value attribute is `data-`
/// prefixed by the caller.
pub(crate) fn is_known_attribute(name: &str) -> bool {
    name.starts_with("data-")
        || name.starts_with("aria-")
        || matches!(name, "epub:type" | "xml:lang" | "xmlns")
        || HTML_ATTRIBUTES.contains(&name)
}

/// HTML attribute names emitted verbatim; any other key/value attribute is `data-` prefixed.
const HTML_ATTRIBUTES: &[&str] = &[
    "abbr",
    "accept",
    "accept-charset",
    "accesskey",
    "action",
    "allow",
    "alt",
    "async",
    "autocapitalize",
    "autocomplete",
    "autofocus",
    "autoplay",
    "charset",
    "checked",
    "cite",
    "class",
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
    "id",
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
    "max",
    "maxlength",
    "media",
    "method",
    "min",
    "minlength",
    "multiple",
    "muted",
    "name",
    "nonce",
    "novalidate",
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

/// The inline content of a block, or an empty slice for a block that carries none directly.
#[cfg(any(feature = "plain", feature = "rst"))]
pub(crate) fn block_inlines(block: &Block) -> &[Inline] {
    match block {
        Block::Plain(inlines) | Block::Para(inlines) => inlines,
        _ => &[],
    }
}

/// Every row of every body, intermediate head rows included, in document order.
#[cfg(any(feature = "plain", feature = "rst"))]
pub(crate) fn body_rows(table: &carta_ast::Table) -> Vec<&carta_ast::Row> {
    table
        .bodies
        .iter()
        .flat_map(|body| body.head.iter().chain(body.body.iter()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_width_classifies_columns() {
        assert_eq!(char_width('a'), 1);
        assert_eq!(char_width('é'), 1);
        assert_eq!(char_width('Ї'), 1);
        assert_eq!(char_width('\n'), 0);
        assert_eq!(char_width('\t'), 0);
        assert_eq!(char_width('\u{7F}'), 0);
        assert_eq!(char_width('\u{85}'), 0);
        assert_eq!(char_width('\u{0301}'), 0);
        assert_eq!(char_width('\u{200B}'), 0);
        assert_eq!(char_width('\u{4E00}'), 2);
        assert_eq!(char_width('\u{FF21}'), 2);
        assert_eq!(char_width('\u{1F600}'), 2);
    }

    #[test]
    fn display_width_sums_characters() {
        assert_eq!(display_width(""), 0);
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("a\u{4E00}b"), 4);
        assert_eq!(display_width("e\u{0301}"), 1);
    }

    #[test]
    fn escape_xml_handles_metacharacters() {
        assert_eq!(escape_xml("a<b>&c", false), "a&lt;b&gt;&amp;c");
        assert_eq!(escape_xml("\"q\"", false), "\"q\"");
        assert_eq!(escape_xml("\"q\"", true), "&quot;q&quot;");
        assert_eq!(escape_attr("<\"&>"), "&lt;&quot;&amp;&gt;");
    }

    #[test]
    fn percent_escaped_uri_validates_escapes_and_charset() {
        assert!(is_percent_escaped_uri("abc", false));
        assert!(is_percent_escaped_uri("a/b?c#d", false));
        assert!(is_percent_escaped_uri("a%20b", false));
        assert!(!is_percent_escaped_uri("a%2", false));
        assert!(!is_percent_escaped_uri("a%zz", false));
        assert!(!is_percent_escaped_uri("a b", false));
        assert!(!is_percent_escaped_uri("café", false));
        assert!(is_percent_escaped_uri("café", true));
    }

    #[test]
    fn uri_scheme_recognition() {
        assert!(!is_uri_scheme(""));
        assert!(is_uri_scheme("http"));
        assert!(is_uri_scheme("x+y-z.w"));
        assert!(!is_uri_scheme("1abc"));
        assert!(!is_uri_scheme("ab cd"));
    }

    #[test]
    fn numeral_renders_every_style() {
        assert_eq!(numeral(5, &ListNumberStyle::Decimal), "5");
        assert_eq!(numeral(5, &ListNumberStyle::DefaultStyle), "5");
        assert_eq!(numeral(5, &ListNumberStyle::Example), "5");
        assert_eq!(numeral(1, &ListNumberStyle::LowerAlpha), "a");
        assert_eq!(numeral(27, &ListNumberStyle::LowerAlpha), "aa");
        assert_eq!(numeral(1, &ListNumberStyle::UpperAlpha), "A");
        assert_eq!(numeral(28, &ListNumberStyle::UpperAlpha), "AB");
        assert_eq!(numeral(4, &ListNumberStyle::LowerRoman), "iv");
        assert_eq!(numeral(9, &ListNumberStyle::LowerRoman), "ix");
        assert_eq!(numeral(2024, &ListNumberStyle::UpperRoman), "MMXXIV");
    }

    #[test]
    fn numeral_non_positive_falls_back_to_decimal() {
        assert_eq!(alpha(0, false), "0");
        assert_eq!(alpha(-3, true), "-3");
        assert_eq!(roman(0, false), "0");
        assert_eq!(roman(-1, true), "-1");
    }

    #[test]
    fn wrap_delim_and_marker() {
        assert_eq!(wrap_delim("3", &ListNumberDelim::Period), "3.");
        assert_eq!(wrap_delim("3", &ListNumberDelim::DefaultDelim), "3.");
        assert_eq!(wrap_delim("3", &ListNumberDelim::OneParen), "3)");
        assert_eq!(wrap_delim("3", &ListNumberDelim::TwoParens), "(3)");
        assert_eq!(
            ordered_marker(2, &ListNumberStyle::LowerRoman, &ListNumberDelim::OneParen),
            "ii)"
        );
    }

    #[test]
    fn offset_conversion_saturates() {
        assert_eq!(offset_as_i32(0), 0);
        assert_eq!(offset_as_i32(7), 7);
        assert_eq!(offset_as_i32(usize::MAX), i32::MAX);
    }

    #[test]
    fn quote_marks_per_kind() {
        assert_eq!(
            quote_marks(&QuoteType::SingleQuote),
            ('\u{2018}', '\u{2019}')
        );
        assert_eq!(
            quote_marks(&QuoteType::DoubleQuote),
            ('\u{201c}', '\u{201d}')
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
    fn attribute_value_lookup() {
        let attr = Attr {
            attributes: vec![("k".into(), "v".into())],
            ..Attr::default()
        };
        assert_eq!(attribute_value(&attr, "k"), Some("v"));
        assert_eq!(attribute_value(&attr, "missing"), None);
    }

    #[test]
    fn fill_wraps_at_column_boundary() {
        let pieces = vec![
            Piece::Text("hello".into()),
            Piece::Space,
            Piece::Text("world".into()),
        ];
        assert_eq!(fill(&pieces, 72), "hello world");
        assert_eq!(fill(&pieces, 8), "hello\nworld");
    }

    #[test]
    fn fill_collapses_spaces_and_keeps_runs_together() {
        let pieces = vec![
            Piece::Space,
            Piece::Text("ab".into()),
            Piece::Text("cd".into()),
            Piece::Space,
            Piece::Space,
            Piece::Text("ef".into()),
            Piece::Space,
        ];
        assert_eq!(fill(&pieces, 72), "abcd ef");
    }

    #[test]
    fn fill_honors_hard_break() {
        let pieces = vec![
            Piece::Text("a".into()),
            Piece::Hard,
            Piece::Text("b".into()),
        ];
        assert_eq!(fill(&pieces, 72), "a\nb");
    }

    #[test]
    fn fill_offset_shifts_first_line_wrap() {
        let pieces = vec![
            Piece::Text("aa".into()),
            Piece::Space,
            Piece::Text("bb".into()),
        ];
        assert_eq!(fill_offset(&pieces, 6, 3), "aa\nbb");
        assert_eq!(fill_offset(&pieces, 8, 3), "aa bb");
    }

    #[test]
    fn indent_block_applies_hanging_prefixes() {
        assert_eq!(indent_block("a\nb\n\nc", "- ", "  "), "- a\n  b\n\n  c");
    }

    #[test]
    fn tightness_and_separators() {
        let tight = vec![vec![Block::Plain(vec![])], vec![]];
        let loose = vec![vec![Block::Para(vec![])]];
        assert!(list_is_tight(&tight));
        assert!(!is_loose(&tight));
        assert!(is_loose(&loose));
        assert_eq!(item_separator(true), "\n\n");
        assert_eq!(item_separator(false), "\n");
    }

    #[test]
    fn join_loose_spaces_blocks() {
        let rendered = vec![
            (false, "A".to_owned()),
            (false, String::new()),
            (false, "B".to_owned()),
        ];
        assert_eq!(join_loose(rendered), "A\n\nB");
        let plain_then_empty = vec![
            (true, "x".to_owned()),
            (false, String::new()),
            (true, "y".to_owned()),
        ];
        assert_eq!(join_loose(plain_then_empty), "x\ny");
    }

    #[test]
    fn append_notes_sections() {
        assert_eq!(append_notes("body\n".to_owned(), &[]), "body");
        assert_eq!(
            append_notes("body".to_owned(), &["[1] note".to_owned()]),
            "body\n\n[1] note"
        );
        assert_eq!(
            append_notes(String::new(), &["[1] note".to_owned()]),
            "[1] note"
        );
    }
}
