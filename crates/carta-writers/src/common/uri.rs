//! URI-scheme recognition and percent-escaping helpers for autolink-capable writers.

/// Whether a string is syntactically a URI scheme: an ASCII letter followed by ASCII letters,
/// digits, or any of `+`, `-`, `.`.
#[cfg_attr(
    not(any(feature = "asciidoc", feature = "mediawiki", feature = "rst")),
    allow(dead_code)
)]
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
#[cfg_attr(
    not(any(
        feature = "commonmark",
        feature = "gfm",
        feature = "man",
        feature = "markdown",
        feature = "mediawiki",
        feature = "plain",
        feature = "rst"
    )),
    allow(dead_code)
)]
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
#[cfg_attr(
    not(any(
        feature = "commonmark",
        feature = "gfm",
        feature = "markdown",
        feature = "mediawiki",
        feature = "plain"
    )),
    allow(dead_code)
)]
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

/// Percent-encode the characters a link destination cannot carry literally: ASCII whitespace and the
/// delimiters `< > | " { } [ ] ^` and the backtick. Every other byte passes through unchanged —
/// including a literal `%`, so an existing `%XX` sequence is preserved rather than doubled — as does
/// all non-ASCII text. The transform is idempotent: applying it twice yields the same result.
#[cfg_attr(not(feature = "rtf"), allow(dead_code))]
pub(crate) fn escape_uri(url: &str) -> String {
    fn hex(nibble: u8) -> char {
        char::from_digit(u32::from(nibble), 16)
            .unwrap_or('0')
            .to_ascii_uppercase()
    }
    let mut out = String::with_capacity(url.len());
    for ch in url.chars() {
        if ch.is_ascii_whitespace()
            || matches!(
                ch,
                '<' | '>' | '|' | '"' | '{' | '}' | '[' | ']' | '^' | '`'
            )
        {
            let byte = ch as u8;
            out.push('%');
            out.push(hex(byte >> 4));
            out.push(hex(byte & 0x0f));
        } else {
            out.push(ch);
        }
    }
    out
}

/// Whether a string is a bare URI eligible to stand alone (as an angle-bracket autolink in
/// `CommonMark`, a bare run in plain text or MediaWiki): it opens with a recognized scheme and every
/// character is valid in a percent-escaped URI.
#[cfg_attr(
    not(any(
        feature = "commonmark",
        feature = "gfm",
        feature = "markdown",
        feature = "plain"
    )),
    allow(dead_code)
)]
pub(crate) fn is_uri(text: &str) -> bool {
    let Some(colon) = text.find(':') else {
        return false;
    };
    text.get(..colon).is_some_and(is_known_scheme) && is_percent_escaped_uri(text, true)
}

/// Decode the `%XX` percent-escapes in `url`, returning the decoded string, or `None` when an escape
/// is truncated or malformed or the decoded bytes are not valid UTF-8.
pub(crate) fn percent_decode(url: &str) -> Option<String> {
    let bytes = url.as_bytes();
    let mut decoded: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'%' {
            let high = bytes.get(index + 1).copied().and_then(hex_digit)?;
            let low = bytes.get(index + 2).copied().and_then(hex_digit)?;
            decoded.push(high << 4 | low);
            index += 3;
        } else {
            decoded.push(byte);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Whether a single-`Str` link label is the visible form of the bare URL `url`: equal to the URL
/// itself, or to the URL with its percent-escapes decoded. A link of this shape is a bare URL; the
/// caller pairs this with the format's own URI test where the autolink form is reserved for genuine
/// URIs, and renders the encoded `url`, not the decoded label.
#[cfg_attr(
    not(any(
        feature = "asciidoc",
        feature = "commonmark",
        feature = "dokuwiki",
        feature = "gfm",
        feature = "latex",
        feature = "man",
        feature = "markdown",
        feature = "mediawiki",
        feature = "org",
        feature = "plain",
        feature = "rst"
    )),
    allow(dead_code)
)]
pub(crate) fn label_matches_url(label: &str, url: &str) -> bool {
    label == url || percent_decode(url).as_deref() == Some(label)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn escape_uri_hexes_only_unsafe_ascii() {
        assert_eq!(escape_uri("http://e.com/a b"), "http://e.com/a%20b");
        assert_eq!(escape_uri("a^b|c"), "a%5Eb%7Cc");
        assert_eq!(escape_uri("<>[]{}\"`"), "%3C%3E%5B%5D%7B%7D%22%60");
        // Sub-delims, percent, backslash, tilde and all non-ASCII pass through unchanged.
        assert_eq!(
            escape_uri("a+b@c:d/e#f?g&h%20~café"),
            "a+b@c:d/e#f?g&h%20~café"
        );
        // Idempotent: a second pass leaves an already-escaped string alone.
        assert_eq!(escape_uri(&escape_uri("a b^c")), escape_uri("a b^c"));
    }

    #[test]
    fn is_uri_requires_scheme_and_valid_charset() {
        assert!(is_uri("https://example.com/path"));
        assert!(is_uri("mailto:user@example.com"));
        assert!(!is_uri("example.com")); // no scheme
        assert!(!is_uri("notascheme:value")); // scheme not recognized
        assert!(!is_uri("http://e.com/a b")); // unescaped space
        assert!(is_uri("http://e.com/café")); // non-ASCII is permitted
    }

    #[test]
    fn uri_scheme_recognition() {
        assert!(!is_uri_scheme(""));
        assert!(is_uri_scheme("http"));
        assert!(is_uri_scheme("x+y-z.w"));
        assert!(!is_uri_scheme("1abc"));
        assert!(!is_uri_scheme("ab cd"));
    }
}
