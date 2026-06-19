//! A tab-aware cursor over a single input line, plus the small value types it parses out.
//!
//! The block phase scans each line through a `Cursor`: it tracks a byte offset and the
//! corresponding visual column (so tab stops expand correctly) and exposes the line-level probes
//! the open-block algorithm needs — indentation width, ATX/setext headings, thematic breaks, fenced
//! code openers, and list markers. It holds no tree state; recognizing a construct and acting on it
//! are separate concerns owned by the block phase.

use carta_ast::{ListNumberDelim, ListNumberStyle};

use super::{TAB_STOP, scan};

/// A parsed fenced-code opener: its marker byte, the run length, the opener's indentation, and the
/// trimmed (and unescaped) info string.
#[derive(Debug, Clone)]
pub(super) struct FenceInfo {
    pub(super) marker: u8,
    pub(super) length: usize,
    pub(super) indent: usize,
    pub(super) info: String,
}

/// A parsed list marker: its kind, the number style and delimiter, the start number for ordered
/// lists, the marker's own width in columns, and whether only whitespace follows it (an empty item
/// opener).
#[derive(Debug)]
pub(super) struct ListMarkerParse {
    pub(super) bullet: bool,
    pub(super) marker: u8,
    pub(super) style: ListNumberStyle,
    pub(super) delim: ListNumberDelim,
    pub(super) start: i32,
    /// Whether the enumerator is a single letter (`a`, `i`, …) rather than a multi-letter roman
    /// numeral or a number. A lone letter is ambiguous between alphabetic and roman readings, which
    /// governs whether it can continue a neighbouring list.
    pub(super) single_letter: bool,
    pub(super) marker_width: usize,
    pub(super) blank_after: bool,
    /// For an example-list marker (`(@label)`, `@label.`, `@label)`), the label that lets a later
    /// `@label` reference resolve to this item's number; `None` for the anonymous `@` and every
    /// non-example marker.
    pub(super) example_label: Option<String>,
}

/// A tab-aware cursor over a single input line.
pub(super) struct Cursor<'a> {
    bytes: &'a [u8],
    line: &'a str,
    offset: usize,
    column: usize,
    indent_mark: usize,
}

impl<'a> Cursor<'a> {
    pub(super) fn new(line: &'a str) -> Self {
        Cursor {
            bytes: line.as_bytes(),
            line,
            offset: 0,
            column: 0,
            indent_mark: 0,
        }
    }

    pub(super) fn peek(&self) -> Option<u8> {
        self.bytes.get(self.offset).copied()
    }

    pub(super) fn checkpoint(&self) -> (usize, usize) {
        (self.offset, self.column)
    }

    pub(super) fn reset_to(&mut self, (offset, column): (usize, usize)) {
        self.offset = offset;
        self.column = column;
    }

    pub(super) fn advance_one(&mut self) {
        if let Some(byte) = self.peek() {
            self.offset += 1;
            self.column += if byte == b'\t' {
                TAB_STOP - (self.column % TAB_STOP)
            } else {
                1
            };
        }
    }

    pub(super) fn consume_optional_space(&mut self) {
        match self.peek() {
            Some(b' ') => {
                self.offset += 1;
                self.column += 1;
            }
            Some(b'\t') => {
                self.offset += 1;
                self.column += TAB_STOP - (self.column % TAB_STOP);
            }
            _ => {}
        }
    }

    /// Visual columns of whitespace from the current position to the first non-space.
    pub(super) fn indent(&self) -> usize {
        let mut column = self.column;
        let mut offset = self.offset;
        while let Some(byte) = self.bytes.get(offset) {
            match byte {
                b' ' => column += 1,
                b'\t' => column += TAB_STOP - (column % TAB_STOP),
                _ => break,
            }
            offset += 1;
        }
        column - self.column
    }

    pub(super) fn note_indent(&mut self) {
        self.indent_mark = self.indent();
    }

    pub(super) fn is_blank(&self) -> bool {
        self.bytes
            .get(self.offset..)
            .unwrap_or(&[])
            .iter()
            .all(|byte| matches!(byte, b' ' | b'\t'))
    }

    pub(super) fn skip_up_to_three_spaces(&mut self) {
        let mut consumed = 0;
        while consumed < 3 {
            match self.peek() {
                Some(b' ') => {
                    self.offset += 1;
                    self.column += 1;
                    consumed += 1;
                }
                _ => break,
            }
        }
    }

    pub(super) fn skip_indent(&mut self) {
        while let Some(byte) = self.peek() {
            match byte {
                b' ' => {
                    self.offset += 1;
                    self.column += 1;
                }
                b'\t' => {
                    self.offset += 1;
                    self.column += TAB_STOP - (self.column % TAB_STOP);
                }
                _ => break,
            }
        }
    }

    /// Advance past `count` characters regardless of kind, used to consume a list marker.
    pub(super) fn advance_chars(&mut self, count: usize) {
        for _ in 0..count {
            self.advance_one();
        }
    }

    /// Advance by up to `count` visual columns of leading whitespace, stopping at non-whitespace.
    pub(super) fn advance_columns(&mut self, count: usize) {
        let target = self.column + count;
        while self.column < target {
            match self.peek() {
                Some(b' ') => {
                    self.offset += 1;
                    self.column += 1;
                }
                Some(b'\t') => {
                    // A tab spanning the target is consumed whole; overshooting the column is
                    // acceptable for indentation.
                    self.offset += 1;
                    self.column += TAB_STOP - (self.column % TAB_STOP);
                }
                _ => break,
            }
        }
    }

    pub(super) fn advance_up_to_columns(&mut self, count: usize) {
        let target = self.column + count;
        while self.column < target {
            match self.peek() {
                Some(b' ') => {
                    self.offset += 1;
                    self.column += 1;
                }
                Some(b'\t') => {
                    let width = TAB_STOP - (self.column % TAB_STOP);
                    if self.column + width > target {
                        break;
                    }
                    self.offset += 1;
                    self.column += width;
                }
                _ => break,
            }
        }
    }

    /// The remaining line content from the cursor, borrowed.
    pub(super) fn remaining(&self) -> &str {
        self.line.get(self.offset..).unwrap_or("")
    }

    /// The remaining line content from the cursor, as-is.
    pub(super) fn rest(&self) -> String {
        self.remaining().to_owned()
    }

    pub(super) fn rest_with_newline(&self) -> String {
        let mut out = self.rest();
        out.push('\n');
        out
    }

    pub(super) fn atx_heading(&mut self) -> Option<i32> {
        let start = self.offset;
        let start_col = self.column;
        let mut hashes = 0;
        while self.peek() == Some(b'#') {
            self.advance_one();
            hashes += 1;
        }
        if hashes == 0 || hashes > 6 {
            self.offset = start;
            self.column = start_col;
            return None;
        }
        match self.peek() {
            None => Some(hashes),
            Some(b' ' | b'\t') => {
                self.consume_optional_space();
                Some(hashes)
            }
            _ => {
                self.offset = start;
                self.column = start_col;
                None
            }
        }
    }

    /// If the remaining line is a setext heading underline, return its level (1 for `=`, 2 for
    /// `-`). The caller has already ensured the leading indent is under four columns.
    pub(super) fn setext_underline(&self) -> Option<i32> {
        let rest = self.bytes.get(self.offset..).unwrap_or(&[]);
        let mut index = 0;
        while rest.get(index) == Some(&b' ') {
            index += 1;
        }
        let marker = *rest.get(index)?;
        if marker != b'=' && marker != b'-' {
            return None;
        }
        let mut count = 0;
        while rest.get(index) == Some(&marker) {
            index += 1;
            count += 1;
        }
        if count == 0 {
            return None;
        }
        let trailing_ok = rest
            .get(index..)
            .is_some_and(|tail| tail.iter().all(|byte| matches!(byte, b' ' | b'\t')));
        if !trailing_ok {
            return None;
        }
        Some(if marker == b'=' { 1 } else { 2 })
    }

    pub(super) fn thematic_break(&self) -> bool {
        let rest = self.bytes.get(self.offset..).unwrap_or(&[]);
        let mut marker = None;
        let mut count = 0;
        for &byte in rest {
            match byte {
                b' ' | b'\t' => {}
                b'-' | b'_' | b'*' => {
                    if let Some(existing) = marker {
                        if existing != byte {
                            return false;
                        }
                    } else {
                        marker = Some(byte);
                    }
                    count += 1;
                }
                _ => return false,
            }
        }
        marker.is_some() && count >= 3
    }

    pub(super) fn fenced_code_start(&mut self) -> Option<FenceInfo> {
        let indent = self.indent_mark;
        let marker = self.peek()?;
        if marker != b'`' && marker != b'~' {
            return None;
        }
        let start = self.offset;
        let start_col = self.column;
        let mut length = 0;
        while self.peek() == Some(marker) {
            self.advance_one();
            length += 1;
        }
        if length < 3 {
            self.offset = start;
            self.column = start_col;
            return None;
        }
        let info = self.rest();
        // A backtick fence's info string may not contain a backtick.
        if marker == b'`' && info.contains('`') {
            self.offset = start;
            self.column = start_col;
            return None;
        }
        Some(FenceInfo {
            marker,
            length,
            indent,
            info: scan::unescape_string(info.trim()),
        })
    }

    pub(super) fn is_closing_fence(&self, marker: u8, min_length: usize) -> bool {
        let rest = self.bytes.get(self.offset..).unwrap_or(&[]);
        let mut count = 0;
        let mut index = 0;
        // Skip leading indentation already handled by caller via indent() check.
        while rest.get(index).copied() == Some(b' ') {
            index += 1;
        }
        while rest.get(index).copied() == Some(marker) {
            count += 1;
            index += 1;
        }
        if count < min_length {
            return false;
        }
        rest.get(index..)
            .is_some_and(|tail| tail.iter().all(|byte| matches!(byte, b' ' | b'\t')))
    }

    /// If the line begins with a footnote-definition marker `[^label]:`, consume the marker and
    /// return its raw label (the text between `[^` and `]`). The label is non-empty and holds no
    /// further brackets; the closing `]` must be followed immediately by a colon. No content space
    /// after the colon is consumed, so the remaining line keeps its indentation for block parsing.
    pub(super) fn footnote_def_marker(&mut self) -> Option<String> {
        let rest = self.remaining();
        let body = rest.strip_prefix("[^")?;
        let end = body.find(']')?;
        let label = body.get(..end)?;
        if label.is_empty() || label.contains('[') {
            return None;
        }
        if body.as_bytes().get(end + 1) != Some(&b':') {
            return None;
        }
        let marker = rest.get(..end + 4)?; // "[^" + label + "]:"
        let marker_len = marker.len();
        let marker_columns = marker.chars().count();
        let label = label.to_owned();
        self.offset += marker_len;
        self.column += marker_columns;
        Some(label)
    }

    /// If the cursor sits at a definition-list marker — a single `:` or `~` followed by a space, a
    /// tab, or the line's end — return whether only whitespace follows it (an empty definition). The
    /// marker char is not consumed.
    pub(super) fn definition_marker_at(&self) -> Option<bool> {
        let byte = self.peek()?;
        if byte != b':' && byte != b'~' {
            return None;
        }
        if !matches!(self.bytes.get(self.offset + 1), None | Some(b' ' | b'\t')) {
            return None;
        }
        Some(rest_is_blank(self.bytes, self.offset + 1))
    }

    /// If the cursor sits at a list marker, return its parse. With `fancy` set, ordered enumerators
    /// also recognize alphabetic and roman styles and the `(x)` parenthesized delimiter; otherwise
    /// only decimal `n.`/`n)` enumerators count.
    pub(super) fn list_marker_at(&self, fancy: bool, example: bool) -> Option<ListMarkerParse> {
        let byte = self.peek()?;
        match byte {
            b'-' | b'+' | b'*' => {
                // Distinguish from a thematic break.
                if self.thematic_break() {
                    return None;
                }
                let blank_after = rest_is_blank(self.bytes, self.offset + 1);
                let followed_ok =
                    matches!(self.bytes.get(self.offset + 1), None | Some(b' ' | b'\t'));
                if !followed_ok {
                    return None;
                }
                Some(ListMarkerParse {
                    bullet: true,
                    marker: byte,
                    style: ListNumberStyle::DefaultStyle,
                    delim: ListNumberDelim::DefaultDelim,
                    start: 1,
                    single_letter: false,
                    marker_width: 1,
                    blank_after,
                    example_label: None,
                })
            }
            b'0'..=b'9' => self.enumerator_at(self.offset),
            b'@' if example => self.example_marker_bare(),
            b'(' if example && self.bytes.get(self.offset + 1) == Some(&b'@') => {
                self.example_marker_paren()
            }
            b'a'..=b'z' | b'A'..=b'Z' if fancy => self.enumerator_at(self.offset),
            b'(' if fancy => self.paren_enumerator_at(),
            _ => None,
        }
    }

    /// Parse an enumerator with a trailing `.` or `)` delimiter at `body`. Used for both decimal and
    /// (when fancy lists are on) alphabetic/roman enumerators.
    fn enumerator_at(&self, body: usize) -> Option<ListMarkerParse> {
        let (style, start, len) = parse_enum_body(self.bytes, body)?;
        let delim_byte = self.bytes.get(body + len).copied();
        let delim = match delim_byte {
            Some(b'.') => ListNumberDelim::Period,
            Some(b')') => ListNumberDelim::OneParen,
            _ => return None,
        };
        let after = body + len + 1;
        let blank_after = rest_is_blank(self.bytes, after);
        if !matches!(self.bytes.get(after), None | Some(b' ' | b'\t')) {
            return None;
        }
        // An uppercase letter followed by a period needs two spaces of separation before content, so
        // a sentence opener like "B. Franklin" is not mistaken for a list (the gap is unnecessary
        // when the item is empty, since nothing follows to be confused).
        if delim == ListNumberDelim::Period
            && matches!(
                style,
                ListNumberStyle::UpperAlpha | ListNumberStyle::UpperRoman
            )
            && !blank_after
            && !two_spaces_at(self.bytes, after)
        {
            return None;
        }
        let single_letter = single_letter(&style, len);
        Some(ListMarkerParse {
            bullet: false,
            marker: delim_byte.unwrap_or(b'.'),
            style,
            delim,
            start,
            single_letter,
            marker_width: len + 1,
            blank_after,
            example_label: None,
        })
    }

    /// Parse a parenthesized enumerator `(x)` at the cursor (fancy lists only).
    fn paren_enumerator_at(&self) -> Option<ListMarkerParse> {
        let body = self.offset + 1;
        let (style, start, len) = parse_enum_body(self.bytes, body)?;
        if self.bytes.get(body + len) != Some(&b')') {
            return None;
        }
        let after = body + len + 1;
        let blank_after = rest_is_blank(self.bytes, after);
        if !matches!(self.bytes.get(after), None | Some(b' ' | b'\t')) {
            return None;
        }
        let single_letter = single_letter(&style, len);
        Some(ListMarkerParse {
            bullet: false,
            marker: b'(',
            style,
            delim: ListNumberDelim::TwoParens,
            start,
            single_letter,
            marker_width: len + 2,
            blank_after,
            example_label: None,
        })
    }

    /// Parse a bare example-list marker `@label.` or `@label)` at the cursor (example lists only).
    /// The label is optional: a lone `@` opens an anonymous, unreferenceable item.
    fn example_marker_bare(&self) -> Option<ListMarkerParse> {
        let body = self.offset + 1;
        let (label, len) = parse_example_label(self.bytes, body);
        let delim = match self.bytes.get(body + len) {
            Some(b'.') => ListNumberDelim::Period,
            Some(b')') => ListNumberDelim::OneParen,
            _ => return None,
        };
        self.example_marker(delim, label, body + len + 1)
    }

    /// Parse a parenthesized example-list marker `(@label)` at the cursor (example lists only).
    fn example_marker_paren(&self) -> Option<ListMarkerParse> {
        let body = self.offset + 2;
        let (label, len) = parse_example_label(self.bytes, body);
        if self.bytes.get(body + len) != Some(&b')') {
            return None;
        }
        self.example_marker(ListNumberDelim::TwoParens, label, body + len + 1)
    }

    /// Assemble an example-list marker that ends at byte `after`, once its delimiter and label are
    /// known. The number style is fixed; the start is a placeholder the block phase replaces with the
    /// item's position in the document-wide example sequence.
    fn example_marker(
        &self,
        delim: ListNumberDelim,
        label: String,
        after: usize,
    ) -> Option<ListMarkerParse> {
        if !matches!(self.bytes.get(after), None | Some(b' ' | b'\t')) {
            return None;
        }
        Some(ListMarkerParse {
            bullet: false,
            marker: b'@',
            style: ListNumberStyle::Example,
            delim,
            start: 1,
            single_letter: false,
            marker_width: after - self.offset,
            blank_after: rest_is_blank(self.bytes, after),
            example_label: (!label.is_empty()).then_some(label),
        })
    }
}

/// Consume an example-list label — a run of `[A-Za-z0-9_-]` — at `start`, returning it and its byte
/// length. An empty run is valid: it marks the anonymous `@`.
fn parse_example_label(bytes: &[u8], start: usize) -> (String, usize) {
    let mut len = 0;
    while let Some(byte) = bytes.get(start + len) {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
            len += 1;
        } else {
            break;
        }
    }
    let label = bytes
        .get(start..start + len)
        .map(|run| String::from_utf8_lossy(run).into_owned())
        .unwrap_or_default();
    (label, len)
}

/// Whether an enumerator of `len` bytes in `style` is a single alphabetic/roman letter.
fn single_letter(style: &ListNumberStyle, len: usize) -> bool {
    len == 1 && !matches!(style, ListNumberStyle::Decimal)
}

/// Parse an ordered-list enumerator value at `start`: a run of digits (decimal), or a run of
/// same-case ASCII letters read as alphabetic (single letter) or roman. Returns the number style,
/// the natural start value, and the enumerator's byte length (excluding any delimiter).
fn parse_enum_body(bytes: &[u8], start: usize) -> Option<(ListNumberStyle, i32, usize)> {
    let first = bytes.get(start).copied()?;
    if first.is_ascii_digit() {
        let mut len = 0;
        let mut value: i64 = 0;
        while let Some(byte) = bytes.get(start + len) {
            if byte.is_ascii_digit() {
                len += 1;
                // An ordered-list start caps at 9 digits; a longer run is not a marker. Enforce the
                // cap before accumulating so `value` cannot overflow.
                if len > 9 {
                    return None;
                }
                value = value * 10 + i64::from(byte - b'0');
            } else {
                break;
            }
        }
        return Some((
            ListNumberStyle::Decimal,
            i32::try_from(value).unwrap_or(1),
            len,
        ));
    }
    if !first.is_ascii_alphabetic() {
        return None;
    }
    let upper = first.is_ascii_uppercase();
    let mut len = 0;
    while let Some(byte) = bytes.get(start + len) {
        if byte.is_ascii_alphabetic() && byte.is_ascii_uppercase() == upper {
            len += 1;
        } else {
            break;
        }
    }
    if len == 1 {
        // A lone `i`/`I` reads as roman one; every other single letter is alphabetic.
        if first == b'i' || first == b'I' {
            let style = if upper {
                ListNumberStyle::UpperRoman
            } else {
                ListNumberStyle::LowerRoman
            };
            return Some((style, 1, 1));
        }
        let style = if upper {
            ListNumberStyle::UpperAlpha
        } else {
            ListNumberStyle::LowerAlpha
        };
        return Some((style, alpha_value(first), 1));
    }
    // A multi-letter enumerator is only valid as a roman numeral.
    let value = roman_value(bytes.get(start..start + len)?)?;
    let style = if upper {
        ListNumberStyle::UpperRoman
    } else {
        ListNumberStyle::LowerRoman
    };
    Some((style, value, len))
}

/// Alphabetic enumerator value: `a`/`A` → 1 … `z`/`Z` → 26.
fn alpha_value(byte: u8) -> i32 {
    i32::from(byte.to_ascii_lowercase() - b'a') + 1
}

/// Value of a single roman digit, case-insensitive.
fn roman_digit(byte: u8) -> Option<i32> {
    Some(match byte.to_ascii_lowercase() {
        b'i' => 1,
        b'v' => 5,
        b'x' => 10,
        b'l' => 50,
        b'c' => 100,
        b'd' => 500,
        b'm' => 1000,
        _ => return None,
    })
}

/// Value of a roman numeral, or `None` if any character is not a roman digit. Uses the subtractive
/// rule (a smaller digit before a larger one subtracts), so `iv` → 4 and `xl` → 40.
fn roman_value(run: &[u8]) -> Option<i32> {
    let mut total = 0i32;
    let mut prev = 0i32;
    let mut idx = run.len();
    while idx > 0 {
        idx -= 1;
        let value = roman_digit(*run.get(idx)?)?;
        if value < prev {
            total -= value;
        } else {
            total += value;
            prev = value;
        }
    }
    (total > 0).then_some(total)
}

/// Whether at least two columns of whitespace begin at `idx` — two spaces, or a single tab.
fn two_spaces_at(bytes: &[u8], idx: usize) -> bool {
    match bytes.get(idx) {
        Some(b'\t') => true,
        Some(b' ') => matches!(bytes.get(idx + 1), Some(b' ' | b'\t')),
        _ => false,
    }
}

/// Whether every byte from `start` to end of line is whitespace (or the line ends there). A list
/// marker followed only by whitespace opens an empty item, regardless of how many spaces follow.
fn rest_is_blank(bytes: &[u8], start: usize) -> bool {
    bytes
        .get(start..)
        .into_iter()
        .flatten()
        .take_while(|byte| **byte != b'\n')
        .all(|byte| matches!(byte, b' ' | b'\t'))
}
