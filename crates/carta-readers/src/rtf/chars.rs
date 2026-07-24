//! Character and measurement helpers: escapes, code-page mapping, and field-instruction parsing.

use carta_ast::{Attr, Text};

use crate::numeric::general_decimal;

/// Builds the [`Attr`] for an embedded picture from its goal dimensions. A `\picwgoal`/`\pichgoal`
/// value is a measurement in twips (1/1440 inch); each present dimension becomes a `width`/`height`
/// attribute expressed in inches.
pub(super) fn picture_attr(goal_width: Option<i32>, goal_height: Option<i32>) -> Attr {
    let mut attributes: Vec<(Text, Text)> = Vec::new();
    if let Some(twips) = goal_width {
        attributes.push((Text::from("width"), Text::from(twips_to_inches(twips))));
    }
    if let Some(twips) = goal_height {
        attributes.push((Text::from("height"), Text::from(twips_to_inches(twips))));
    }
    Attr {
        id: Text::default(),
        classes: Vec::new(),
        attributes,
    }
}

/// A twip measurement (1/1440 inch) rendered as an inch dimension, e.g. `1440` -> `1.0in`.
fn twips_to_inches(twips: i32) -> String {
    format!("{}in", general_decimal(f64::from(twips) / 1440.0))
}

/// The Unicode string a special-character control word stands for, or `None` if the word carries no
/// character.
pub(super) fn special_char(word: &str) -> Option<&'static str> {
    Some(match word {
        "emdash" => "\u{2014}",
        "endash" => "\u{2013}",
        "bullet" => "\u{2022}",
        "lquote" => "\u{2018}",
        "rquote" => "\u{2019}",
        "ldblquote" => "\u{201C}",
        "rdblquote" => "\u{201D}",
        "emspace" => "\u{2003}",
        "enspace" => "\u{2002}",
        "qmspace" => "\u{2005}",
        "zwj" => "\u{200D}",
        "zwnj" => "\u{200C}",
        "ltrmark" => "\u{200E}",
        "rtlmark" => "\u{200F}",
        _ => return None,
    })
}

/// The character a control symbol (`\` before one non-letter) stands for, or `None` if it carries
/// no text.
pub(super) fn symbol_char(symbol: char) -> Option<&'static str> {
    Some(match symbol {
        '\\' => "\\",
        '{' => "{",
        '}' => "}",
        '~' => "\u{00A0}",
        '-' => "\u{00AD}",
        '_' => "\u{2011}",
        _ => return None,
    })
}

/// Resolves a `\uN` parameter to a Unicode scalar value. The parameter is a signed 16-bit integer;
/// a negative value denotes a code point above 0x7FFF, recovered by adding 65536.
pub(super) fn unicode_code(raw: i32) -> u32 {
    if raw < 0 {
        u32::try_from(i64::from(raw) + 65536).unwrap_or(0)
    } else {
        u32::try_from(raw).unwrap_or(0)
    }
}

/// Folds a `\u` scalar into the pending UTF-16 surrogate state, returning the code point to emit, if
/// any. A high surrogate is held back; a following low surrogate combines with it into a supplementary
/// scalar; any other value clears a stale pending high and is emitted unchanged.
pub(super) fn combine_surrogate(pending_high: &mut Option<u32>, code: u32) -> Option<u32> {
    if (0xD800..=0xDBFF).contains(&code) {
        *pending_high = Some(code);
        None
    } else if (0xDC00..=0xDFFF).contains(&code) {
        pending_high
            .take()
            .map(|high| 0x1_0000 + ((high - 0xD800) << 10) + (code - 0xDC00))
    } else {
        *pending_high = None;
        Some(code)
    }
}

/// Maps a code-page byte to a character. Bytes outside `0x80..=0x9F` are Latin-1; that window uses
/// the Windows-1252 assignments, the code page an unqualified `\ansi` document carries.
pub(super) fn code_page_char(byte: u8) -> char {
    let scalar: u32 = match byte {
        0x80 => 0x20AC,
        0x82 => 0x201A,
        0x83 => 0x0192,
        0x84 => 0x201E,
        0x85 => 0x2026,
        0x86 => 0x2020,
        0x87 => 0x2021,
        0x88 => 0x02C6,
        0x89 => 0x2030,
        0x8A => 0x0160,
        0x8B => 0x2039,
        0x8C => 0x0152,
        0x8E => 0x017D,
        0x91 => 0x2018,
        0x92 => 0x2019,
        0x93 => 0x201C,
        0x94 => 0x201D,
        0x95 => 0x2022,
        0x96 => 0x2013,
        0x97 => 0x2014,
        0x98 => 0x02DC,
        0x99 => 0x2122,
        0x9A => 0x0161,
        0x9B => 0x203A,
        0x9C => 0x0153,
        0x9E => 0x017E,
        0x9F => 0x0178,
        other => u32::from(other),
    };
    char::from_u32(scalar).unwrap_or('\u{FFFD}')
}

/// Decodes a hex-digit string into bytes, ignoring a trailing unpaired digit.
pub(super) fn decode_hex(hex: &str) -> Vec<u8> {
    let digits: Vec<u32> = hex.chars().filter_map(|c| c.to_digit(16)).collect();
    digits
        .chunks_exact(2)
        .filter_map(|pair| match pair {
            [hi, lo] => u8::try_from((hi << 4) | lo).ok(),
            _ => None,
        })
        .collect()
}

/// Extracts a link target from a field instruction. A `HYPERLINK` instruction is followed by its
/// destination, which runs from the keyword to the first backslash, the marker every field switch
/// (`\l`, `\o`, ...) carries, with any quotes removed and outer whitespace trimmed. When no such
/// destination is present, an `\l` switch names an in-document bookmark, and its argument becomes a
/// fragment target (`#name`). An instruction without the `HYPERLINK` keyword is not a link and yields
/// `None`.
pub(super) fn parse_hyperlink(instruction: &str) -> Option<String> {
    const KEYWORD: &str = "HYPERLINK";
    let after = instruction.find(KEYWORD)? + KEYWORD.len();
    let tail = instruction.get(after..).unwrap_or_default();
    let destination = match tail.find('\\') {
        Some(cut) => tail.get(..cut).unwrap_or_default(),
        None => tail,
    };
    let target = strip_field_quotes(destination);
    if !target.is_empty() {
        return Some(target);
    }
    if let Some(anchor) = field_switch_argument(tail, "l")
        && !anchor.is_empty()
    {
        return Some(format!("#{anchor}"));
    }
    Some(target)
}

/// Strips the double quotes and outer whitespace that wrap a field argument.
fn strip_field_quotes(text: &str) -> String {
    text.chars()
        .filter(|&c| c != '"')
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Locates a field switch (`\<name>`) in an instruction and returns the argument that follows it
/// (the text up to the next switch), with quotes and outer whitespace removed. Returns `None` when no
/// switch of that name is present. Matching is by whole control-word name, so `\l` is not found
/// inside a longer word such as `\line`.
fn field_switch_argument(tail: &str, name: &str) -> Option<String> {
    let mut rest = tail;
    while let Some(backslash) = rest.find('\\') {
        let after = rest.get(backslash + 1..).unwrap_or_default();
        let word_len = after
            .chars()
            .take_while(char::is_ascii_alphabetic)
            .map(char::len_utf8)
            .sum();
        let word = after.get(..word_len).unwrap_or_default();
        let argument = after.get(word_len..).unwrap_or_default();
        if word == name {
            let value = match argument.find('\\') {
                Some(cut) => argument.get(..cut).unwrap_or_default(),
                None => argument,
            };
            return Some(strip_field_quotes(value));
        }
        rest = argument;
    }
    None
}
