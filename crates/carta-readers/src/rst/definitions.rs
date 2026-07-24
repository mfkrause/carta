//! First-pass scan collecting document-global reference definitions.

use super::directives::{image_classes, image_parts, split_directive};
use super::inline_helpers::push_text;
use super::markers::{Explicit, classify_explicit, explicit_body, explicit_extent};
use super::{
    DEFAULT_ROLE, Parser, escape_uri, indent_of, indirect_referent, is_blank, line_at,
    normalize_name, preprocess,
};
use crate::heading_ids::IdRegistry;
use carta_ast::{Attr, Block, Inline};
use carta_core::Extensions;
use std::collections::BTreeMap;

// --- definitions (pass one) --------------------------------------------------------------------

#[derive(Default)]
pub(super) struct Definitions {
    /// Anonymous-target destinations, in document order.
    pub(super) anonymous: Vec<String>,
    /// Normalized substitution name to its definition.
    pub(super) substitutions: BTreeMap<String, Substitution>,
    /// Labeled footnote bodies, keyed by the label as written (`1`, `#name`).
    pub(super) footnotes: BTreeMap<String, Vec<String>>,
    /// Auto-numbered (`#`) footnote bodies, in document order.
    pub(super) auto_footnotes: Vec<Vec<String>>,
    /// Symbol (`*`) footnote bodies, in document order.
    pub(super) symbol_footnotes: Vec<Vec<String>>,
    /// Citations: original label and body, in document order.
    pub(super) citations: Vec<(String, Vec<String>)>,
}

#[derive(Clone)]
pub(super) enum Substitution {
    Replace(String),
    Image(String, Attr, Vec<Inline>),
}

/// A custom interpreted-text role declared by a `role` directive: an optional base role whose
/// formatting it inherits, the classes it adds, and the format or language its base needs (a `raw`
/// base takes a `:format:`, a `code` base a `:language:`).
#[derive(Clone, Default)]
pub(super) struct RoleDef {
    pub(super) base: Option<String>,
    pub(super) classes: Vec<String>,
    pub(super) format: Option<String>,
    pub(super) language: Option<String>,
}

/// The result of following a custom-role chain to the builtin role that renders it: the builtin
/// role name (empty for a plain baseless role), the classes accumulated along the chain, and the
/// format and language the chain declares.
#[derive(Default)]
pub(super) struct RoleChain {
    pub(super) base: String,
    pub(super) classes: Vec<String>,
    pub(super) format: Option<String>,
    pub(super) language: Option<String>,
}

/// Read and parse an included file, returning its blocks for splicing into the document. Returns
/// `None` when the file cannot be read.
pub(super) fn included_blocks(path: &str, ext: Extensions, depth: usize) -> Option<Vec<Block>> {
    let content = std::fs::read_to_string(path).ok()?;
    let lines = preprocess(&content);
    let defs = collect_definitions(&lines);
    let mut parser = Parser {
        defs: &defs,
        ext,
        heading_styles: Vec::new(),
        ids: IdRegistry::default(),
        auto_footnote: 0,
        symbol_footnote: 0,
        anonymous: 0,
        custom_roles: BTreeMap::new(),
        default_role: DEFAULT_ROLE.to_string(),
        include_depth: depth,
        active_substitutions: Vec::new(),
        deferred: BTreeMap::new(),
    };
    let mut blocks = parser.blocks(&lines);
    if let Some(div) = parser.citation_block() {
        blocks.push(div);
    }
    parser.resolve_deferred(&mut blocks);
    Some(blocks)
}

pub(super) fn collect_definitions(lines: &[String]) -> Definitions {
    let mut defs = Definitions::default();
    let mut i = 0;
    while i < lines.len() {
        let line = line_at(lines, i);
        if is_blank(line) {
            i += 1;
            continue;
        }
        let indent = indent_of(line);
        let trimmed = line.trim_start();
        if let Some(kind) = classify_explicit(trimmed) {
            let end = explicit_extent(lines, i, indent);
            record_definition(&mut defs, lines, i, end, indent, kind);
            i = end;
        } else {
            i += 1;
        }
    }
    defs
}

fn record_definition(
    defs: &mut Definitions,
    lines: &[String],
    start: usize,
    end: usize,
    indent: usize,
    kind: Explicit,
) {
    let first = line_at(lines, start).trim_start();
    match kind {
        Explicit::AnonymousTarget => {
            let url = parse_anonymous(first, lines, start, end, indent);
            defs.anonymous.push(url);
        }
        Explicit::Footnote(label) => {
            let body = footnote_body(lines, start, end, indent);
            if label == "#" {
                defs.auto_footnotes.push(body);
            } else if label == "*" {
                defs.symbol_footnotes.push(body);
            } else {
                defs.footnotes.insert(label, body);
            }
        }
        Explicit::Citation(label) => {
            let body = footnote_body(lines, start, end, indent);
            defs.citations.push((label, body));
        }
        Explicit::Substitution => {
            if let Some((name, subst)) = parse_substitution(first, lines, start, end, indent) {
                defs.substitutions.insert(normalize_name(&name), subst);
            }
        }
        // Targets register during tree construction (last name wins); none of these feed the first pass.
        Explicit::Target | Explicit::Directive(_) | Explicit::Comment => {}
    }
}

/// Parse a hyperlink target `_name: url` (the URL may continue across lines, joined without spaces).
pub(super) fn parse_target(
    first: &str,
    lines: &[String],
    start: usize,
    end: usize,
    indent: usize,
) -> Option<(String, String)> {
    let rest = first.strip_prefix("..").unwrap_or(first).trim_start();
    let rest = rest.strip_prefix('_')?;
    let (name, after) = split_target_name(rest)?;
    let mut url = after.trim().to_string();
    for i in start + 1..end {
        let line = line_at(lines, i);
        if !is_blank(line) && indent_of(line) > indent {
            url.push_str(line.trim());
        }
    }
    Some((name, url))
}

/// Split a target's name from its destination at the terminating colon, honoring a backtick-quoted
/// phrase name.
fn split_target_name(rest: &str) -> Option<(String, String)> {
    if let Some(after) = rest.strip_prefix('`') {
        let close = after.find('`')?;
        let name = &after[..close];
        let tail = after.get(close + 1..)?.trim_start();
        let tail = tail.strip_prefix(':')?;
        return Some((name.to_string(), tail.to_string()));
    }
    // Name runs to the first unescaped colon followed by a space or EOL; an escaped colon stays in the name.
    let (colon, after_colon) = unescaped_terminator(rest)?;
    let name = rest.get(..colon)?.replace("\\:", ":");
    let after = rest.get(after_colon..).unwrap_or("");
    Some((name, after.to_string()))
}

/// Find the colon that terminates a target name: the first `:` that is not backslash-escaped and is
/// followed by a space or the end of the line. Returns the colon's byte offset and the offset just
/// past it.
fn unescaped_terminator(rest: &str) -> Option<(usize, usize)> {
    let mut escaped = false;
    for (offset, ch) in rest.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            ':' => {
                let after = offset + ch.len_utf8();
                if rest
                    .get(after..)
                    .and_then(|t| t.chars().next())
                    .is_none_or(|c| c == ' ')
                {
                    return Some((offset, after));
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_anonymous(
    first: &str,
    lines: &[String],
    start: usize,
    end: usize,
    indent: usize,
) -> String {
    let rest = first.strip_prefix("..").map_or(first, str::trim_start);
    let rest = rest.trim_start_matches('_');
    let rest = rest.trim_start_matches(':');
    let mut url = rest.trim().to_string();
    for i in start + 1..end {
        let line = line_at(lines, i);
        if !is_blank(line) && indent_of(line) > indent {
            url.push_str(line.trim());
        }
    }
    if indirect_referent(&url).is_some() {
        url
    } else {
        escape_uri(&url)
    }
}

/// The body region of a footnote or citation: the text after the `.. [label]` marker, plus the
/// dedented continuation, which the second pass parses as block content.
fn footnote_body(lines: &[String], start: usize, end: usize, indent: usize) -> Vec<String> {
    let first = line_at(lines, start);
    let trimmed = first.trim_start();
    let prefix_len = indent + trimmed.find(']').map_or_else(|| trimmed.len(), |p| p + 1);
    explicit_body(lines, start, end, prefix_len)
}

fn parse_substitution(
    first: &str,
    lines: &[String],
    start: usize,
    end: usize,
    indent: usize,
) -> Option<(String, Substitution)> {
    let trimmed = first.strip_prefix("..").unwrap_or(first).trim_start();
    let rest = trimmed.strip_prefix('|')?;
    let close = rest.find('|')?;
    let name = rest.get(..close)?.to_string();
    let after = rest.get(close + 1..)?.trim_start();
    let coloncolon = after.find("::")?;
    let directive = after.get(..coloncolon)?.trim().to_lowercase();
    let arg_remainder = after.get(coloncolon + 2..).unwrap_or("").trim_start();
    let prefix_len = indent + (first.chars().count() - arg_remainder.chars().count());
    let body = explicit_body(lines, start, end, prefix_len);
    let (argument, options, _content) = split_directive(&body);
    match directive.as_str() {
        "replace" => Some((name, Substitution::Replace(argument))),
        "image" => {
            let (mut attr, mut alt, url) = image_parts(&argument, &options);
            attr.classes = image_classes(&options)
                .into_iter()
                .map(Into::into)
                .collect();
            // A substitution image with no explicit alt text falls back to the substitution name.
            if alt.is_empty() {
                push_text(&mut alt, &name);
            }
            Some((name, Substitution::Image(url, attr, alt)))
        }
        "unicode" => Some((name, Substitution::Replace(unicode_chars(&argument)))),
        "date" => Some((name, Substitution::Replace(format_date(argument.trim())))),
        _ => Some((name, Substitution::Replace(String::new()))),
    }
}

/// Decode the tokens of a `unicode::` substitution argument. A token written as a hexadecimal code
/// point (`0x`, `x`, `u`, `\x`, `\u`, `U+`, or an `&#x…;` character reference) becomes its
/// character; any other token, including a bare decimal number, stays as written. Tokens are joined
/// with a single space, and a standalone `..` ends the text.
fn unicode_chars(argument: &str) -> String {
    let mut tokens = Vec::new();
    for token in argument.split_whitespace() {
        if token == ".." {
            break;
        }
        tokens.push(decode_unicode_token(token));
    }
    tokens.join(" ")
}

fn decode_unicode_token(token: &str) -> String {
    if let Some(rest) = token.strip_prefix("&#x")
        && let Some(hex) = rest.strip_suffix(';')
        && let Some(ch) = code_point(hex)
    {
        return ch.to_string();
    }
    let hex = token
        .strip_prefix("U+")
        .or_else(|| token.strip_prefix("0x"))
        .or_else(|| token.strip_prefix("\\u"))
        .or_else(|| token.strip_prefix("\\x"))
        .or_else(|| token.strip_prefix('x'))
        .or_else(|| token.strip_prefix('u'));
    if let Some(hex) = hex
        && let Some(ch) = code_point(hex)
    {
        return ch.to_string();
    }
    token.to_string()
}

/// Parse a non-empty run of hexadecimal digits into its character, or `None` for empty or
/// out-of-range input.
fn code_point(hex: &str) -> Option<char> {
    if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
}

/// Render the current date with a strftime-style format string, defaulting to `%Y-%m-%d`. The date
/// is taken in UTC.
pub(super) fn format_date(format: &str) -> String {
    let format = if format.is_empty() {
        "%Y-%m-%d"
    } else {
        format
    };
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0);
    render_date(secs, format)
}

const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAY_NAMES: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// Expand a strftime-style format against the civil date and time of day at `secs` seconds past the
/// epoch (UTC). Unrecognized `%`-codes are emitted verbatim; `%%` yields a single percent.
pub(super) fn render_date(secs: i64, format: &str) -> String {
    let parts = DateParts::from_secs(secs);
    let mut out = String::new();
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some(spec) => {
                if let Some(value) = parts.field(spec) {
                    out.push_str(&value);
                } else {
                    out.push('%');
                    if spec != '%' {
                        out.push(spec);
                    }
                }
            }
            None => out.push('%'),
        }
    }
    out
}

fn pad2(n: i64) -> String {
    format!("{n:02}")
}

fn pad3(n: i64) -> String {
    format!("{n:03}")
}

fn space2(n: i64) -> String {
    format!("{n:2}")
}

/// `53` for ISO long years (those whose 1 January is a Thursday, or whose previous year's 1 January
/// is a Wednesday), `52` otherwise.
fn iso_weeks_in_year(year: i64) -> i64 {
    let dominical =
        |y: i64| (y + y.div_euclid(4) - y.div_euclid(100) + y.div_euclid(400)).rem_euclid(7);
    if dominical(year) == 4 || dominical(year - 1) == 3 {
        53
    } else {
        52
    }
}

/// The decomposed civil date and time of day for a moment, in UTC.
struct DateParts {
    year: i64,
    /// 1-12.
    month: i64,
    /// 1-31.
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
    /// 0 = Sunday … 6 = Saturday.
    weekday: i64,
    /// Day of the year, 1-366.
    yday: i64,
}

impl DateParts {
    fn from_secs(secs: i64) -> Self {
        let days = secs.div_euclid(86_400);
        let day_secs = secs.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        Self {
            year,
            month,
            day,
            hour: day_secs / 3600,
            minute: day_secs / 60 % 60,
            second: day_secs % 60,
            // 1970-01-01 was a Thursday (index 4).
            weekday: (days.rem_euclid(7) + 4).rem_euclid(7),
            yday: days - days_from_civil(year, 1, 1) + 1,
        }
    }

    /// ISO 8601 weekday: 1 = Monday … 7 = Sunday.
    fn iso_weekday(&self) -> i64 {
        if self.weekday == 0 { 7 } else { self.weekday }
    }

    /// Hour on a 12-hour clock, 1-12.
    fn hour12(&self) -> i64 {
        let h = self.hour % 12;
        if h == 0 { 12 } else { h }
    }

    fn meridiem(&self, upper: bool) -> &'static str {
        match (self.hour < 12, upper) {
            (true, true) => "AM",
            (true, false) => "am",
            (false, true) => "PM",
            (false, false) => "pm",
        }
    }

    /// Week of the year counting from the first Sunday (`%U`), 00-53.
    fn week_from_sunday(&self) -> i64 {
        (self.yday - 1 + 7 - self.weekday) / 7
    }

    /// Week of the year counting from the first Monday (`%W`), 00-53.
    fn week_from_monday(&self) -> i64 {
        (self.yday - 1 + 7 - (self.weekday + 6) % 7) / 7
    }

    /// ISO 8601 (week-numbering-year, week-of-year), the latter 01-53.
    fn iso_week(&self) -> (i64, i64) {
        let week = (self.yday + 10 - self.iso_weekday()) / 7;
        if week < 1 {
            (self.year - 1, iso_weeks_in_year(self.year - 1))
        } else if week > iso_weeks_in_year(self.year) {
            (self.year + 1, 1)
        } else {
            (self.year, week)
        }
    }

    /// The rendering of one strftime field, or `None` for an unrecognized code.
    fn field(&self, spec: char) -> Option<String> {
        let month_name = MONTH_NAMES
            .get(usize::try_from(self.month - 1).unwrap_or(0))
            .copied()
            .unwrap_or("");
        let weekday_name = WEEKDAY_NAMES
            .get(usize::try_from(self.weekday).unwrap_or(0))
            .copied()
            .unwrap_or("");
        Some(match spec {
            'Y' => self.year.to_string(),
            'y' => pad2(self.year.rem_euclid(100)),
            'C' => pad2(self.year.div_euclid(100)),
            'm' => pad2(self.month),
            'd' => pad2(self.day),
            'e' => space2(self.day),
            'H' => pad2(self.hour),
            'k' => space2(self.hour),
            'I' => pad2(self.hour12()),
            'l' => space2(self.hour12()),
            'M' => pad2(self.minute),
            'S' => pad2(self.second),
            'j' => pad3(self.yday),
            'p' => self.meridiem(true).to_string(),
            'P' => self.meridiem(false).to_string(),
            'u' => self.iso_weekday().to_string(),
            'w' => self.weekday.to_string(),
            'U' => pad2(self.week_from_sunday()),
            'W' => pad2(self.week_from_monday()),
            'V' => pad2(self.iso_week().1),
            'G' => self.iso_week().0.to_string(),
            'g' => pad2(self.iso_week().0.rem_euclid(100)),
            'B' => month_name.to_string(),
            'b' | 'h' => month_name.get(..3).unwrap_or(month_name).to_string(),
            'A' => weekday_name.to_string(),
            'a' => weekday_name.get(..3).unwrap_or(weekday_name).to_string(),
            'D' => format!(
                "{:02}/{:02}/{:02}",
                self.month,
                self.day,
                self.year.rem_euclid(100)
            ),
            'F' => format!("{}-{:02}-{:02}", self.year, self.month, self.day),
            'R' => format!("{:02}:{:02}", self.hour, self.minute),
            'T' => format!("{:02}:{:02}:{:02}", self.hour, self.minute, self.second),
            'r' => format!(
                "{:02}:{:02}:{:02} {}",
                self.hour12(),
                self.minute,
                self.second,
                self.meridiem(true)
            ),
            'n' => "\n".to_string(),
            't' => "\t".to_string(),
            _ => return None,
        })
    }
}

/// The civil (year, month, day) of a day count measured from the epoch, by the standard
/// days-to-civil conversion. `month` is 1-12 and `day` is 1-31.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// The day count from the epoch of a civil date, the inverse of `civil_from_days`.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
