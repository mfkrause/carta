//! A focused YAML parser for document metadata blocks.
//!
//! It covers the subset that frontmatter uses: block mappings and sequences, flow collections
//! (`[a, b]`, `{k: v}`), plain/single-quoted/double-quoted scalars, and literal (`|`) and folded
//! (`>`) block scalars, plus `#` comments. Scalar *flavor* is retained ([`Scalar`]) because type
//! resolution — booleans, null, and number canonicalization — applies only to unquoted plain
//! scalars; the conversion to metadata happens in [`super::frontmatter`].
//!
//! The grammar handled here is the one document front matter needs, not all of YAML; anchors,
//! aliases, tags, and multi-document streams are out of scope (see `docs/PORTING.md`).

/// A parsed YAML value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Yaml {
    Mapping(Vec<(String, Yaml)>),
    Sequence(Vec<Yaml>),
    Scalar(Scalar),
}

/// A scalar together with the source syntax that produced it, which decides how it is typed.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Scalar {
    /// An unquoted scalar, eligible for boolean / null / number resolution.
    Plain(String),
    /// A quoted scalar; always a string, never a boolean or number.
    Quoted(String),
    /// A literal or folded block scalar. Its text may keep a trailing newline, which later decides
    /// block-versus-inline rendering.
    Block(String),
}

/// The top-level shape of a metadata block's content.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TopLevel {
    /// A mapping (possibly empty): its entries become metadata keys.
    Mapping(Vec<(String, Yaml)>),
    /// Valid content that is not a mapping (a sequence or a bare scalar). Front matter leaves such a
    /// block in the body rather than treating it as metadata.
    NotMapping,
}

/// Maximum nesting depth for block and flow collections. Input nested past this is rejected rather
/// than recursing without bound; real document metadata never approaches it.
const MAX_NESTING_DEPTH: usize = 512;

/// Parse the text between metadata fences. `Err` marks malformed input — a hard failure the caller
/// surfaces as an error; `Ok` carries the top-level classification.
pub(crate) fn parse(content: &str) -> Result<TopLevel, ()> {
    let mut reader = Reader::new(content);
    reader.skip_ignorable();
    let Some(first) = reader.peek() else {
        return Ok(TopLevel::Mapping(Vec::new()));
    };
    let indent = indent_of(first);
    let body = slice_from(first, indent);
    if is_sequence_entry(body) {
        return Ok(TopLevel::NotMapping);
    }
    if key_colon(body).is_some() {
        return Ok(TopLevel::Mapping(parse_mapping(&mut reader, indent, 0)?));
    }
    Ok(TopLevel::NotMapping)
}

/// A line cursor over the block's content. Lines are kept raw (blanks and comments included) so that
/// block scalars can capture them verbatim; structural parsing skips them via [`skip_ignorable`].
struct Reader<'a> {
    lines: Vec<&'a str>,
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(content: &'a str) -> Self {
        Reader {
            lines: content.split('\n').collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<&'a str> {
        self.lines.get(self.pos).copied()
    }

    fn advance(&mut self) {
        self.pos += 1;
    }

    /// Skip blank and whole-line comment lines, the only lines a structural parse may discard.
    fn skip_ignorable(&mut self) {
        while let Some(line) = self.peek() {
            if is_blank(line) || is_comment(line) {
                self.advance();
            } else {
                break;
            }
        }
    }
}

fn is_blank(line: &str) -> bool {
    line.bytes().all(|b| b == b' ')
}

fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with('#')
}

fn indent_of(line: &str) -> usize {
    line.bytes().take_while(|&b| b == b' ').count()
}

/// `line[start..]` without panicking on a non-boundary index.
fn slice_from(line: &str, start: usize) -> &str {
    line.get(start..).unwrap_or("")
}

/// True if `body` (already past its indentation) opens a block sequence entry: a `-` followed by a
/// space or end of line.
fn is_sequence_entry(body: &str) -> bool {
    body == "-" || body.starts_with("- ")
}

/// The byte offset of the `:` that separates a mapping key from its value — a colon followed by a
/// space or the end of the line, not inside quotes or flow brackets — if `body` opens a mapping
/// entry.
fn key_colon(body: &str) -> Option<usize> {
    let bytes = body.as_bytes();
    let mut i = 0;
    let mut quote: Option<u8> = None;
    while let Some(&b) = bytes.get(i) {
        match quote {
            Some(q) => {
                if b == q {
                    quote = None;
                }
            }
            None => match b {
                b'\'' | b'"' => quote = Some(b),
                b'[' | b'{' => return None,
                b':' if matches!(bytes.get(i + 1), None | Some(b' ')) => return Some(i),
                b'#' if i > 0 && bytes.get(i - 1) == Some(&b' ') => return None,
                _ => {}
            },
        }
        i += 1;
    }
    None
}

/// Parse a block mapping whose entries are indented exactly `indent`.
fn parse_mapping(
    reader: &mut Reader,
    indent: usize,
    depth: usize,
) -> Result<Vec<(String, Yaml)>, ()> {
    if depth > MAX_NESTING_DEPTH {
        return Err(());
    }
    let mut entries = Vec::new();
    loop {
        reader.skip_ignorable();
        let Some(line) = reader.peek() else { break };
        let line_indent = indent_of(line);
        if line_indent < indent {
            break;
        }
        if line_indent > indent {
            return Err(());
        }
        let body = slice_from(line, indent);
        if is_sequence_entry(body) {
            break;
        }
        let colon = key_colon(body).ok_or(())?;
        let key = parse_key(slice_from(body, 0).get(..colon).unwrap_or(""));
        let rest = slice_from(body, colon + 1);
        reader.advance();
        let value = parse_value(reader, rest, indent, depth)?;
        entries.push((key, value));
    }
    Ok(entries)
}

/// Parse a block sequence whose `-` markers sit at column `indent`.
fn parse_sequence(reader: &mut Reader, indent: usize, depth: usize) -> Result<Vec<Yaml>, ()> {
    if depth > MAX_NESTING_DEPTH {
        return Err(());
    }
    let mut items = Vec::new();
    loop {
        reader.skip_ignorable();
        let Some(line) = reader.peek() else { break };
        if indent_of(line) != indent {
            break;
        }
        let body = slice_from(line, indent);
        if !is_sequence_entry(body) {
            break;
        }
        let after_dash = slice_from(body, 1);
        reader.advance();
        let trimmed = after_dash.trim_start();
        if !trimmed.is_empty() && key_colon(trimmed).is_some() {
            // A block mapping written as a sequence item: its keys align under the first one.
            let key_indent = indent + 1 + whitespace(after_dash);
            items.push(Yaml::Mapping(parse_mapping_from(
                reader, key_indent, trimmed, depth,
            )?));
        } else {
            items.push(parse_value(reader, after_dash, indent, depth)?);
        }
    }
    Ok(items)
}

/// Parse a block mapping whose first entry comes from `first` (the text after a `- ` marker) and
/// whose remaining entries are continuation lines indented to `indent`.
fn parse_mapping_from(
    reader: &mut Reader,
    indent: usize,
    first: &str,
    depth: usize,
) -> Result<Vec<(String, Yaml)>, ()> {
    let colon = key_colon(first).ok_or(())?;
    let key = parse_key(first.get(..colon).unwrap_or(""));
    let value = parse_value(reader, slice_from(first, colon + 1), indent, depth)?;
    let mut entries = vec![(key, value)];
    entries.append(&mut parse_mapping(reader, indent, depth)?);
    Ok(entries)
}

/// Parse the value that follows a `key:` or `-` marker. `rest` is the remainder of the marker's line
/// (possibly empty); `marker_indent` is the indentation of the line the marker sat on.
fn parse_value(
    reader: &mut Reader,
    rest: &str,
    marker_indent: usize,
    depth: usize,
) -> Result<Yaml, ()> {
    let after = rest.trim_start();
    if after.is_empty() || after.starts_with('#') {
        return parse_nested_or_null(reader, marker_indent, depth);
    }
    match after.as_bytes().first() {
        Some(b'|' | b'>') => Ok(Yaml::Scalar(parse_block_scalar(
            reader,
            after,
            marker_indent,
        ))),
        Some(b'[') => parse_flow(after, depth).map(|(value, _)| value),
        Some(b'{') => parse_flow(after, depth).map(|(value, _)| value),
        Some(b'\'' | b'"') => {
            let (text, _) = parse_quoted(after)?;
            Ok(Yaml::Scalar(Scalar::Quoted(text)))
        }
        // A plain scalar cannot contain a `: ` mapping indicator; such a value is malformed.
        _ if key_colon(after).is_some() => Err(()),
        _ => Ok(Yaml::Scalar(Scalar::Plain(parse_plain(
            reader,
            after,
            marker_indent,
        )))),
    }
}

/// After a marker with no inline value, the value is either a more-indented nested block or null.
fn parse_nested_or_null(
    reader: &mut Reader,
    marker_indent: usize,
    depth: usize,
) -> Result<Yaml, ()> {
    reader.skip_ignorable();
    let Some(line) = reader.peek() else {
        return Ok(Yaml::Scalar(Scalar::Plain(String::new())));
    };
    let line_indent = indent_of(line);
    let body = slice_from(line, line_indent);
    // A block sequence may sit at the marker's own column; any other nested block must be deeper.
    if is_sequence_entry(body) && line_indent >= marker_indent {
        return Ok(Yaml::Sequence(parse_sequence(
            reader,
            line_indent,
            depth + 1,
        )?));
    }
    if line_indent > marker_indent {
        if is_sequence_entry(body) {
            return Ok(Yaml::Sequence(parse_sequence(
                reader,
                line_indent,
                depth + 1,
            )?));
        }
        if key_colon(body).is_some() {
            return Ok(Yaml::Mapping(parse_mapping(
                reader,
                line_indent,
                depth + 1,
            )?));
        }
    }
    Ok(Yaml::Scalar(Scalar::Plain(String::new())))
}

fn parse_key(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(b'\'' | b'"') = trimmed.as_bytes().first()
        && let Ok((text, _)) = parse_quoted(trimmed)
    {
        return text;
    }
    trimmed.to_owned()
}

/// Collect a plain scalar: this line's text (minus any trailing comment) folded with following more
/// indented continuation lines, newlines becoming spaces.
fn parse_plain(reader: &mut Reader, first: &str, marker_indent: usize) -> String {
    let mut parts = vec![strip_trailing_comment(first).trim_end().to_owned()];
    while let Some(line) = reader.peek() {
        if is_blank(line) || is_comment(line) {
            break;
        }
        if indent_of(line) <= marker_indent {
            break;
        }
        let body = slice_from(line, indent_of(line));
        if is_sequence_entry(body) || key_colon(body).is_some() {
            break;
        }
        parts.push(body.trim_end().to_owned());
        reader.advance();
    }
    parts.join(" ")
}

/// Drop a trailing ` # comment` from an unquoted scalar.
fn strip_trailing_comment(text: &str) -> &str {
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'#' && (i == 0 || bytes.get(i - 1) == Some(&b' ')) {
            return text.get(..i).unwrap_or(text);
        }
        i += 1;
    }
    text
}

/// How a block scalar treats trailing newlines.
enum Chomp {
    Clip,
    Strip,
    Keep,
}

/// Parse a literal (`|`) or folded (`>`) block scalar, returning its dedented, chomped text.
fn parse_block_scalar(reader: &mut Reader, header: &str, marker_indent: usize) -> Scalar {
    let folded = header.as_bytes().first() == Some(&b'>');
    let mut chomp = Chomp::Clip;
    let mut explicit_indent = None;
    for ch in slice_from(header, 1).chars() {
        match ch {
            '-' => chomp = Chomp::Strip,
            '+' => chomp = Chomp::Keep,
            '1'..='9' => explicit_indent = Some(marker_indent + (ch as usize - '0' as usize)),
            ' ' | '\t' => {}
            _ => break,
        }
    }

    let mut raw = Vec::new();
    while let Some(line) = reader.peek() {
        if is_blank(line) || indent_of(line) > marker_indent {
            raw.push(line);
            reader.advance();
        } else {
            break;
        }
    }
    // Trailing blank lines belong to the next block unless chomping keeps them; trim, remembering how
    // many there were so `Keep` can restore them.
    let trailing_blanks = raw.iter().rev().take_while(|l| is_blank(l)).count();
    let content_indent = explicit_indent.unwrap_or_else(|| {
        raw.iter()
            .filter(|l| !is_blank(l))
            .map(|l| indent_of(l))
            .min()
            .unwrap_or(marker_indent + 1)
    });

    let kept = raw.len() - trailing_blanks;
    let mut lines: Vec<String> = raw
        .iter()
        .take(kept)
        .map(|line| {
            if is_blank(line) {
                String::new()
            } else {
                slice_from(line, content_indent.min(indent_of(line))).to_owned()
            }
        })
        .collect();

    let body = if folded {
        fold(&lines)
    } else {
        lines.join("\n")
    };
    let text = match chomp {
        Chomp::Strip => body,
        Chomp::Clip => {
            if body.is_empty() {
                body
            } else {
                format!("{body}\n")
            }
        }
        Chomp::Keep => {
            lines.extend(std::iter::repeat_n(String::new(), trailing_blanks));
            let kept_body = if folded {
                fold(&lines)
            } else {
                lines.join("\n")
            };
            format!("{kept_body}\n")
        }
    };
    Scalar::Block(text)
}

/// Fold block-scalar lines: runs of non-empty lines join with a single space; each blank line
/// becomes a newline break.
fn fold(lines: &[String]) -> String {
    let mut out = String::new();
    let mut prev_blank = true;
    for line in lines {
        if line.is_empty() {
            out.push('\n');
            prev_blank = true;
        } else {
            if !prev_blank {
                out.push(' ');
            }
            out.push_str(line);
            prev_blank = false;
        }
    }
    out
}

/// Parse a single- or double-quoted scalar starting at `s`, returning its unescaped text and the
/// number of bytes consumed (including the closing quote).
fn parse_quoted(s: &str) -> Result<(String, usize), ()> {
    let bytes = s.as_bytes();
    let quote = *bytes.first().ok_or(())?;
    let mut out = String::new();
    let mut i = 1;
    while let Some(&b) = bytes.get(i) {
        if quote == b'\'' {
            if b == b'\'' {
                if bytes.get(i + 1) == Some(&b'\'') {
                    out.push('\'');
                    i += 2;
                    continue;
                }
                return Ok((out, i + 1));
            }
            push_byte(s, &mut out, i, b);
        } else {
            match b {
                b'"' => return Ok((out, i + 1)),
                b'\\' => {
                    i += 1;
                    let Some(&esc) = bytes.get(i) else {
                        return Err(());
                    };
                    push_escape(&mut out, esc);
                }
                _ => push_byte(s, &mut out, i, b),
            }
        }
        i += 1;
    }
    Err(())
}

/// Append the character that starts at byte `i`, falling back to the raw byte for ASCII (the common
/// case) so multi-byte text is reassembled correctly.
fn push_byte(s: &str, out: &mut String, i: usize, b: u8) {
    if b.is_ascii() {
        out.push(b as char);
    } else if let Some(ch) = slice_from(s, i).chars().next() {
        out.push(ch);
    }
}

fn push_escape(out: &mut String, esc: u8) {
    out.push(match esc {
        b'n' => '\n',
        b't' => '\t',
        b'r' => '\r',
        b'0' => '\0',
        _ => esc as char,
    });
}

/// Parse a flow collection (`[...]` or `{...}`) starting at `s`, returning the value and bytes
/// consumed. Flow collections are confined to a single line here.
fn parse_flow(s: &str, depth: usize) -> Result<(Yaml, usize), ()> {
    if depth > MAX_NESTING_DEPTH {
        return Err(());
    }
    match s.as_bytes().first() {
        Some(b'[') => parse_flow_sequence(s, depth),
        Some(b'{') => parse_flow_mapping(s, depth),
        _ => Err(()),
    }
}

fn parse_flow_sequence(s: &str, depth: usize) -> Result<(Yaml, usize), ()> {
    let bytes = s.as_bytes();
    let mut items = Vec::new();
    let mut i = 1;
    loop {
        i += whitespace(slice_from(s, i));
        match bytes.get(i) {
            Some(b']') => return Ok((Yaml::Sequence(items), i + 1)),
            None => return Err(()),
            _ => {}
        }
        let (value, used) = parse_flow_node(slice_from(s, i), depth)?;
        items.push(value);
        i += used;
        i += whitespace(slice_from(s, i));
        match bytes.get(i) {
            Some(b',') => i += 1,
            Some(b']') => return Ok((Yaml::Sequence(items), i + 1)),
            _ => return Err(()),
        }
    }
}

fn parse_flow_mapping(s: &str, depth: usize) -> Result<(Yaml, usize), ()> {
    let bytes = s.as_bytes();
    let mut entries = Vec::new();
    let mut i = 1;
    loop {
        i += whitespace(slice_from(s, i));
        match bytes.get(i) {
            Some(b'}') => return Ok((Yaml::Mapping(entries), i + 1)),
            None => return Err(()),
            _ => {}
        }
        let (key, used) = parse_flow_key(slice_from(s, i))?;
        i += used;
        i += whitespace(slice_from(s, i));
        if bytes.get(i) != Some(&b':') {
            return Err(());
        }
        i += 1;
        i += whitespace(slice_from(s, i));
        let (value, used) = parse_flow_node(slice_from(s, i), depth)?;
        entries.push((key, value));
        i += used;
        i += whitespace(slice_from(s, i));
        match bytes.get(i) {
            Some(b',') => i += 1,
            Some(b'}') => return Ok((Yaml::Mapping(entries), i + 1)),
            _ => return Err(()),
        }
    }
}

/// Parse one element of a flow collection: a nested flow collection, a quoted scalar, or a plain
/// scalar that runs to the next structural character.
fn parse_flow_node(s: &str, depth: usize) -> Result<(Yaml, usize), ()> {
    match s.as_bytes().first() {
        Some(b'[' | b'{') => parse_flow(s, depth + 1),
        Some(b'\'' | b'"') => {
            let (text, used) = parse_quoted(s)?;
            Ok((Yaml::Scalar(Scalar::Quoted(text)), used))
        }
        _ => {
            let used = flow_plain_len(s);
            let text = slice_from(s, 0).get(..used).unwrap_or("").trim_end();
            Ok((Yaml::Scalar(Scalar::Plain(text.to_owned())), used))
        }
    }
}

fn parse_flow_key(s: &str) -> Result<(String, usize), ()> {
    if let Some(b'\'' | b'"') = s.as_bytes().first() {
        return parse_quoted(s);
    }
    let bytes = s.as_bytes();
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if matches!(b, b':' | b',' | b'}' | b'{' | b'[' | b']') {
            break;
        }
        i += 1;
    }
    Ok((slice_from(s, 0).get(..i).unwrap_or("").trim().to_owned(), i))
}

/// Length of a plain scalar inside a flow collection: it ends at the next `,`, `]`, or `}`.
fn flow_plain_len(s: &str) -> usize {
    let bytes = s.as_bytes();
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if matches!(b, b',' | b']' | b'}') {
            break;
        }
        i += 1;
    }
    i
}

fn whitespace(s: &str) -> usize {
    s.bytes().take_while(|&b| b == b' ' || b == b'\t').count()
}

/// Canonicalize a numeric scalar the way the metadata resolver does, or `None` if the text is not
/// one of the recognized number forms (those stay verbatim strings). Recognized: decimal, `0x` hex,
/// and `0o` octal integers, and floats with at least one leading digit.
pub(crate) fn canonicalize_number(text: &str) -> Option<String> {
    if let Some(value) = parse_radix_int(text) {
        return Some(value);
    }
    if is_decimal_int(text) {
        if let Ok(n) = text.parse::<i64>() {
            return Some(n.to_string());
        }
        // Outside the signed 64-bit range a whole number renders in scientific form.
        let (neg, digits) = match text.as_bytes().first() {
            Some(b'-') => (true, slice_from(text, 1)),
            Some(b'+') => (false, slice_from(text, 1)),
            _ => (false, text),
        };
        return Some(render_decimal(neg, digits, "", 0));
    }
    canonicalize_float(text)
}

fn is_decimal_int(text: &str) -> bool {
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

fn parse_radix_int(text: &str) -> Option<String> {
    // A radix integer carries no sign; a leading `+`/`-` makes the token an ordinary string.
    if matches!(text.as_bytes().first(), Some(b'-' | b'+')) {
        return None;
    }
    let (radix, digits) = if let Some(d) = text.strip_prefix("0x") {
        (16, d)
    } else if let Some(d) = text.strip_prefix("0o") {
        (8, d)
    } else {
        return None;
    };
    if digits.is_empty() {
        return None;
    }
    let magnitude = i128::from_str_radix(digits, radix).ok()?;
    Some(match i64::try_from(magnitude) {
        Ok(n) => n.to_string(),
        // Beyond the signed 64-bit range the value renders in scientific form, as decimals do.
        Err(_) => render_decimal(false, &magnitude.to_string(), "", 0),
    })
}

/// Canonicalize a float scalar (sign, integer digits, optional fraction, optional exponent — with at
/// least one leading digit and either a `.` or an exponent), or `None` if `text` is not such a form.
fn canonicalize_float(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut i = 0;
    let neg = match bytes.first() {
        Some(b'-') => {
            i = 1;
            true
        }
        Some(b'+') => {
            i = 1;
            false
        }
        _ => false,
    };
    let int_start = i;
    while bytes.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    if i == int_start {
        return None; // a leading digit is required
    }
    let int_digits = slice_from(text, int_start)
        .get(..i - int_start)
        .unwrap_or("");

    let mut frac_digits = "";
    let mut has_dot = false;
    if bytes.get(i) == Some(&b'.') {
        has_dot = true;
        i += 1;
        let frac_start = i;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        frac_digits = slice_from(text, frac_start)
            .get(..i - frac_start)
            .unwrap_or("");
    }

    let mut exp: i64 = 0;
    let mut has_exp = false;
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        has_exp = true;
        i += 1;
        let exp_neg = match bytes.get(i) {
            Some(b'-') => {
                i += 1;
                true
            }
            Some(b'+') => {
                i += 1;
                false
            }
            _ => false,
        };
        let exp_start = i;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == exp_start {
            return None;
        }
        exp = slice_from(text, exp_start)
            .get(..i - exp_start)
            .unwrap_or("")
            .parse::<i64>()
            .ok()?;
        if exp_neg {
            exp = -exp;
        }
    }

    if i != text.len() || !(has_dot || has_exp) {
        return None;
    }
    Some(render_decimal(neg, int_digits, frac_digits, exp))
}

/// Render the decimal value `±(int_digits.frac_digits) × 10^exp` the way the metadata number
/// renderer does: integral and modest-magnitude values as plain decimals, very large or very small
/// ones in `d.d…e±n` scientific form.
fn render_decimal(neg: bool, int_digits: &str, frac_digits: &str, exp: i64) -> String {
    let mut digits: Vec<u8> = int_digits.bytes().chain(frac_digits.bytes()).collect();
    // The decimal point starts after the integer digits, then shifts by the exponent. Saturating
    // arithmetic keeps an absurd exponent (e.g. `1e9999999999999999999`) from overflowing.
    let mut point = len_i64(int_digits.len()).saturating_add(exp);

    // Strip leading zeros, each shifting the point left; then strip trailing zeros.
    let leading = digits.iter().take_while(|&&b| b == b'0').count();
    digits.drain(..leading);
    point = point.saturating_sub(len_i64(leading));
    while digits.last() == Some(&b'0') {
        digits.pop();
    }
    if digits.is_empty() {
        return "0".to_owned();
    }

    let sign = if neg { "-" } else { "" };
    let digit_count = len_i64(digits.len());
    // With no fractional digits the value is a whole number: rendered plainly when it lands in the
    // signed 64-bit range, otherwise in scientific form. A fractional value is plain only at modest
    // magnitudes (leading digit between 10^-1 and 10^6); beyond that it too goes scientific.
    let body = if point >= digit_count {
        if fits_i64_integer(&digits, point, neg) {
            fixed(&digits, point, digit_count)
        } else {
            scientific(&digits, point)
        }
    } else if (0..=7).contains(&point) {
        fixed(&digits, point, digit_count)
    } else {
        scientific(&digits, point)
    };
    format!("{sign}{body}")
}

/// Convert a length to `i64`, saturating at the maximum; lengths never realistically reach it.
fn len_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// Whether the whole number with `point` total digits (significant `digits` then trailing zeros)
/// and the given sign lands in the signed 64-bit range. The bound has 19 digits, so shorter
/// magnitudes always fit and longer ones never do; equal-length ones compare digit by digit.
fn fits_i64_integer(digits: &[u8], point: i64, neg: bool) -> bool {
    if point < 19 {
        return true;
    }
    if point > 19 {
        return false;
    }
    let zeros = usize::try_from(point.saturating_sub(len_i64(digits.len()))).unwrap_or(0);
    let mut magnitude = digits_str(digits);
    magnitude.push_str(&"0".repeat(zeros));
    let limit = if neg {
        "9223372036854775808"
    } else {
        "9223372036854775807"
    };
    magnitude.as_str() <= limit
}

fn digits_str(digits: &[u8]) -> String {
    String::from_utf8_lossy(digits).into_owned()
}

fn fixed(digits: &[u8], point: i64, digit_count: i64) -> String {
    let text = digits_str(digits);
    if point <= 0 {
        let zeros = usize::try_from(point.unsigned_abs()).unwrap_or(usize::MAX);
        format!("0.{}{}", "0".repeat(zeros), text)
    } else if point >= digit_count {
        let zeros = usize::try_from(point.saturating_sub(digit_count)).unwrap_or(usize::MAX);
        format!("{}{}", text, "0".repeat(zeros))
    } else {
        let split = usize::try_from(point).unwrap_or(0);
        format!(
            "{}.{}",
            text.get(..split).unwrap_or(""),
            text.get(split..).unwrap_or("")
        )
    }
}

fn scientific(digits: &[u8], point: i64) -> String {
    let text = digits_str(digits);
    let lead = text.get(..1).unwrap_or("0");
    let rest = text.get(1..).unwrap_or("");
    let mantissa = if rest.is_empty() {
        format!("{lead}.0")
    } else {
        format!("{lead}.{rest}")
    };
    format!("{mantissa}e{}", point.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::{MAX_NESTING_DEPTH, Scalar, TopLevel, Yaml, canonicalize_number, parse};

    fn map(content: &str) -> Vec<(String, Yaml)> {
        match parse(content) {
            Ok(TopLevel::Mapping(entries)) => entries,
            other => panic!("expected a mapping, got {other:?}"),
        }
    }

    fn plain(content: &str) -> String {
        match map(content).into_iter().next() {
            Some((_, Yaml::Scalar(Scalar::Plain(text)))) => text,
            other => panic!("expected one plain entry, got {other:?}"),
        }
    }

    #[test]
    fn a_simple_mapping_keeps_insertion_order_and_plain_values() {
        assert_eq!(
            map("a: 1\nb: two\n"),
            vec![
                ("a".to_owned(), Yaml::Scalar(Scalar::Plain("1".to_owned()))),
                (
                    "b".to_owned(),
                    Yaml::Scalar(Scalar::Plain("two".to_owned()))
                ),
            ]
        );
    }

    #[test]
    fn an_empty_value_is_an_empty_plain_scalar() {
        assert_eq!(plain("k:\n"), "");
        assert_eq!(plain("k: # only a comment\n"), "");
    }

    #[test]
    fn a_trailing_comment_is_dropped_from_a_plain_scalar() {
        assert_eq!(plain("k: value # note\n"), "value");
    }

    #[test]
    fn empty_content_is_an_empty_mapping() {
        assert_eq!(map(""), Vec::new());
        assert_eq!(map("# just a comment\n"), Vec::new());
    }

    #[test]
    fn a_top_level_sequence_or_scalar_is_not_a_mapping() {
        assert_eq!(parse("- a\n- b\n"), Ok(TopLevel::NotMapping));
        assert_eq!(parse("foo\n"), Ok(TopLevel::NotMapping));
    }

    #[test]
    fn a_block_sequence_value_may_sit_at_the_key_column() {
        let entries = map("tags:\n- x\n- y\n");
        let [(key, Yaml::Sequence(items))] = entries.as_slice() else {
            panic!("expected one sequence entry, got {entries:?}");
        };
        assert_eq!(key, "tags");
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn a_nested_mapping_is_parsed_by_indentation() {
        let entries = map("m:\n  k: v\n");
        let [(_, Yaml::Mapping(inner))] = entries.as_slice() else {
            panic!("expected a nested mapping, got {entries:?}");
        };
        assert_eq!(inner.len(), 1);
        assert_eq!(inner.first().map(|(k, _)| k.as_str()), Some("k"));
    }

    #[test]
    fn a_flow_sequence_parses_its_elements() {
        let entries = map("k: [x, y]\n");
        let [(_, Yaml::Sequence(items))] = entries.as_slice() else {
            panic!("expected a flow sequence, got {entries:?}");
        };
        assert_eq!(
            items,
            &[
                Yaml::Scalar(Scalar::Plain("x".to_owned())),
                Yaml::Scalar(Scalar::Plain("y".to_owned())),
            ]
        );
    }

    #[test]
    fn an_unclosed_flow_sequence_is_malformed() {
        assert_eq!(parse("k: [unclosed\n"), Err(()));
    }

    #[test]
    fn quotes_force_a_string_and_unescape() {
        let entries = map("a: \"x\\ty\"\nb: 'it''s'\n");
        assert_eq!(
            entries,
            vec![
                (
                    "a".to_owned(),
                    Yaml::Scalar(Scalar::Quoted("x\ty".to_owned()))
                ),
                (
                    "b".to_owned(),
                    Yaml::Scalar(Scalar::Quoted("it's".to_owned()))
                ),
            ]
        );
    }

    #[test]
    fn a_literal_block_scalar_keeps_newlines_and_a_clip_newline() {
        let entries = map("k: |\n  a\n  b\n");
        let [(_, Yaml::Scalar(Scalar::Block(text)))] = entries.as_slice() else {
            panic!("expected a block scalar, got {entries:?}");
        };
        assert_eq!(text, "a\nb\n");
    }

    #[test]
    fn a_strip_block_scalar_drops_the_trailing_newline() {
        let entries = map("k: |-\n  a\n  b\n");
        let [(_, Yaml::Scalar(Scalar::Block(text)))] = entries.as_slice() else {
            panic!("expected a block scalar, got {entries:?}");
        };
        assert_eq!(text, "a\nb");
    }

    #[test]
    fn a_folded_block_scalar_joins_lines_with_spaces() {
        let entries = map("k: >\n  a\n  b\n");
        let [(_, Yaml::Scalar(Scalar::Block(text)))] = entries.as_slice() else {
            panic!("expected a block scalar, got {entries:?}");
        };
        assert_eq!(text, "a b\n");
    }

    #[test]
    fn a_multi_line_plain_scalar_folds_with_spaces() {
        assert_eq!(plain("k: one\n  two\n"), "one two");
    }

    #[test]
    fn integers_canonicalize_across_radixes() {
        assert_eq!(canonicalize_number("007").as_deref(), Some("7"));
        assert_eq!(canonicalize_number("010").as_deref(), Some("10"));
        assert_eq!(canonicalize_number("0o10").as_deref(), Some("8"));
        assert_eq!(canonicalize_number("0x10").as_deref(), Some("16"));
        assert_eq!(canonicalize_number("0xFF").as_deref(), Some("255"));
        assert_eq!(canonicalize_number("+7").as_deref(), Some("7"));
        assert_eq!(canonicalize_number("-0").as_deref(), Some("0"));
        assert_eq!(canonicalize_number("-7").as_deref(), Some("-7"));
    }

    #[test]
    fn whole_numbers_past_the_64_bit_range_render_in_scientific_form() {
        // The largest magnitudes the signed 64-bit range holds stay plain integers.
        assert_eq!(
            canonicalize_number("9223372036854775807").as_deref(),
            Some("9223372036854775807")
        );
        assert_eq!(
            canonicalize_number("-9223372036854775808").as_deref(),
            Some("-9223372036854775808")
        );
        // One step past either bound switches to scientific notation, full precision preserved.
        assert_eq!(
            canonicalize_number("9223372036854775808").as_deref(),
            Some("9.223372036854775808e18")
        );
        assert_eq!(
            canonicalize_number("-9223372036854775809").as_deref(),
            Some("-9.223372036854775809e18")
        );
        assert_eq!(
            canonicalize_number("100000000000000000000").as_deref(),
            Some("1.0e20")
        );
        // The same threshold applies to hexadecimal and octal whole numbers.
        assert_eq!(
            canonicalize_number("0x8000000000000000").as_deref(),
            Some("9.223372036854775808e18")
        );
        // A signed radix token is not a number at all; it stays a verbatim string.
        assert_eq!(canonicalize_number("-0xF"), None);
        assert_eq!(canonicalize_number("+0o17"), None);
    }

    #[test]
    fn floats_canonicalize_to_fixed_or_scientific() {
        assert_eq!(canonicalize_number("1e3").as_deref(), Some("1000"));
        assert_eq!(canonicalize_number("1.5e3").as_deref(), Some("1500"));
        assert_eq!(canonicalize_number("3.14").as_deref(), Some("3.14"));
        assert_eq!(canonicalize_number("1.0").as_deref(), Some("1"));
        assert_eq!(canonicalize_number("12.340").as_deref(), Some("12.34"));
        assert_eq!(canonicalize_number("100.00").as_deref(), Some("100"));
        assert_eq!(canonicalize_number("0.0").as_deref(), Some("0"));
        assert_eq!(
            canonicalize_number("1e18").as_deref(),
            Some("1000000000000000000")
        );
        assert_eq!(canonicalize_number("1e19").as_deref(), Some("1.0e19"));
        assert_eq!(canonicalize_number("6.022e23").as_deref(), Some("6.022e23"));
        assert_eq!(canonicalize_number("0.09").as_deref(), Some("9.0e-2"));
        assert_eq!(canonicalize_number("0.1").as_deref(), Some("0.1"));
        // A fractional value stays plain up to a leading digit at 10^6, then turns scientific.
        assert_eq!(
            canonicalize_number("1234567.5").as_deref(),
            Some("1234567.5")
        );
        assert_eq!(
            canonicalize_number("12345678.5").as_deref(),
            Some("1.23456785e7")
        );
        // Below 10^-1 it likewise turns scientific.
        assert_eq!(canonicalize_number("0.9").as_deref(), Some("0.9"));
        assert_eq!(canonicalize_number("0.05").as_deref(), Some("5.0e-2"));
        // An integral float that fits stays a plain integer, however it is written.
        assert_eq!(canonicalize_number("2.5e8").as_deref(), Some("250000000"));
        assert_eq!(canonicalize_number("1.5e19").as_deref(), Some("1.5e19"));
        // Scientific notation keeps every significant digit rather than rounding to a float.
        assert_eq!(
            canonicalize_number("1234567890123456.5").as_deref(),
            Some("1.2345678901234565e15")
        );
    }

    #[test]
    fn nesting_past_the_limit_is_rejected_rather_than_overflowing() {
        let depth = MAX_NESTING_DEPTH + 50;
        // Flow collections nest within bounded input, the recursion the depth guard protects.
        let flow = format!("k: {}{}", "[".repeat(depth), "]".repeat(depth));
        assert_eq!(parse(&flow), Err(()));
        // The same guard covers block-mapping nesting.
        let mut block = String::new();
        for i in 0..depth {
            block.push_str(&" ".repeat(i));
            let _ = writeln!(block, "k{i}:");
        }
        assert_eq!(parse(&block), Err(()));
    }

    #[test]
    fn non_numbers_stay_verbatim_strings() {
        assert_eq!(canonicalize_number(".5"), None);
        assert_eq!(canonicalize_number("1_000"), None);
        assert_eq!(canonicalize_number("0b101"), None);
        assert_eq!(canonicalize_number("07:30"), None);
        assert_eq!(canonicalize_number("v1"), None);
        assert_eq!(canonicalize_number(""), None);
    }
}
