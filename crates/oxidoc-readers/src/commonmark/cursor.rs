//! A tab-aware cursor over a single input line, plus the small value types it parses out.
//!
//! The block phase scans each line through a `Cursor`: it tracks a byte offset and the
//! corresponding visual column (so tab stops expand correctly) and exposes the line-level probes
//! the open-block algorithm needs — indentation width, ATX/setext headings, thematic breaks, fenced
//! code openers, and list markers. It holds no tree state; recognizing a construct and acting on it
//! are separate concerns owned by the block phase.

use oxidoc_ast::ListNumberDelim;

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

/// A parsed list marker: its kind and delimiter, the start number for ordered lists, the marker's
/// own width in columns, and whether only whitespace follows it (an empty item opener).
#[derive(Debug)]
pub(super) struct ListMarkerParse {
    pub(super) bullet: bool,
    pub(super) marker: u8,
    pub(super) delim: ListNumberDelim,
    pub(super) start: i32,
    pub(super) marker_width: usize,
    pub(super) blank_after: bool,
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

    pub(super) fn list_marker_at(&mut self) -> Option<ListMarkerParse> {
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
                    delim: ListNumberDelim::DefaultDelim,
                    start: 1,
                    marker_width: 1,
                    blank_after,
                })
            }
            b'0'..=b'9' => self.ordered_marker_at(),
            _ => None,
        }
    }

    fn ordered_marker_at(&self) -> Option<ListMarkerParse> {
        let mut digits = 0;
        let mut value: i64 = 0;
        while let Some(byte) = self.bytes.get(self.offset + digits) {
            if byte.is_ascii_digit() {
                digits += 1;
                // CommonMark caps an ordered-list start at 9 digits; a longer run is not a marker.
                // Enforce it before accumulating so `value` cannot overflow.
                if digits > 9 {
                    return None;
                }
                value = value * 10 + i64::from(byte - b'0');
            } else {
                break;
            }
        }
        if digits == 0 {
            return None;
        }
        let delim_byte = self.bytes.get(self.offset + digits).copied();
        let delim = match delim_byte {
            Some(b'.') => ListNumberDelim::Period,
            Some(b')') => ListNumberDelim::OneParen,
            _ => return None,
        };
        let after = self.bytes.get(self.offset + digits + 1);
        let blank_after = rest_is_blank(self.bytes, self.offset + digits + 1);
        if !matches!(after, None | Some(b' ' | b'\t')) {
            return None;
        }
        let start = i32::try_from(value).unwrap_or(1);
        Some(ListMarkerParse {
            bullet: false,
            marker: delim_byte.unwrap_or(b'.'),
            delim,
            start,
            marker_width: digits + 1,
            blank_after,
        })
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
