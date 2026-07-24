//! Shared parsing helpers for the docx reader: run toggles, numbering, fields, images, and paths.

use std::collections::BTreeMap;

use carta_ast::{Alignment, Attr, Inline, ListNumberDelim, ListNumberStyle, Text};

use crate::xml::{Element, local_name};

use super::{LevelDef, RunToggles};

/// Reads the tri-state run toggles from an `rPr` element.
pub(super) fn read_toggles(properties: &Element) -> RunToggles {
    let mut toggles = RunToggles::default();
    for child in properties.elements() {
        let on = !matches!(child.attr("val"), Some("false" | "0" | "off" | "none"));
        match local_name(&child.name) {
            "b" => toggles.bold = Some(on),
            "i" => toggles.italic = Some(on),
            "u" => toggles.underline = Some(on),
            "strike" | "dstrike" => toggles.strike = Some(on),
            "smallCaps" => toggles.smallcaps = Some(on),
            "highlight" => toggles.mark = Some(on),
            "vertAlign" => match child.attr("val") {
                Some("superscript") => toggles.superscript = Some(true),
                Some("subscript") => toggles.subscript = Some(true),
                _ => {
                    toggles.superscript = Some(false);
                    toggles.subscript = Some(false);
                }
            },
            _ => {}
        }
    }
    toggles
}

/// A paragraph's net left indent in twips: the left (or start) margin less any hanging indent. A
/// first-line indent does not shift the block edge and is excluded. Only a paragraph's own `ind`
/// counts; an indent inherited from its style does not.
pub(super) fn net_left_indent(properties: Option<&Element>) -> i32 {
    let Some(ind) = properties.and_then(|pr| pr.child("ind")) else {
        return 0;
    };
    let left = ind
        .attr("left")
        .or_else(|| ind.attr("start"))
        .and_then(parse_int)
        .unwrap_or(0);
    let hanging = ind.attr("hanging").and_then(parse_int).unwrap_or(0);
    left.saturating_sub(hanging)
}

/// Reads a list level's marker configuration from a `w:lvl` element.
pub(super) fn read_level(lvl: &Element) -> LevelDef {
    let num_fmt = lvl
        .child("numFmt")
        .and_then(|element| element.attr("val"))
        .unwrap_or("decimal");
    let lvl_text = lvl
        .child("lvlText")
        .and_then(|element| element.attr("val"))
        .unwrap_or("");
    let start = lvl
        .child("start")
        .and_then(|element| element.attr("val"))
        .and_then(parse_int)
        .unwrap_or(1);
    LevelDef {
        style: number_style(num_fmt),
        delim: number_delim(lvl_text),
        start,
    }
}

/// Applies any per-level start overrides a concrete `w:num` declares.
pub(super) fn apply_level_overrides(num: &Element, levels: &mut BTreeMap<i32, LevelDef>) {
    for override_element in num.elements() {
        if local_name(&override_element.name) != "lvlOverride" {
            continue;
        }
        let Some(ilvl) = override_element.attr("ilvl").and_then(parse_int) else {
            continue;
        };
        if let Some(start) = override_element
            .child("startOverride")
            .and_then(|element| element.attr("val"))
            .and_then(parse_int)
            && let Some(level) = levels.get_mut(&ilvl)
        {
            level.start = start;
        }
    }
}

/// Maps an OOXML number format to a list numeral style; a bullet or unnumbered level has none.
fn number_style(num_fmt: &str) -> Option<ListNumberStyle> {
    match num_fmt {
        "bullet" | "none" => None,
        "decimal" | "decimalZero" => Some(ListNumberStyle::Decimal),
        "upperRoman" => Some(ListNumberStyle::UpperRoman),
        "lowerRoman" => Some(ListNumberStyle::LowerRoman),
        "upperLetter" => Some(ListNumberStyle::UpperAlpha),
        "lowerLetter" => Some(ListNumberStyle::LowerAlpha),
        _ => Some(ListNumberStyle::DefaultStyle),
    }
}

/// Reads the marker delimiter from a level's format text (`%1.` → period, `%1)` → one-paren,
/// `(%1)` → two-parens).
fn number_delim(lvl_text: &str) -> ListNumberDelim {
    let trimmed = lvl_text.trim();
    if trimmed.starts_with('(') && trimmed.ends_with(')') {
        ListNumberDelim::TwoParens
    } else if trimmed.ends_with(')') {
        ListNumberDelim::OneParen
    } else {
        ListNumberDelim::Period
    }
}

/// Indexes note bodies by id, skipping the separator pseudo-notes.
pub(super) fn index_notes(root: &Element, tag: &str, out: &mut BTreeMap<String, Element>) {
    for note in root.elements() {
        if local_name(&note.name) != tag {
            continue;
        }
        if matches!(
            note.attr("type"),
            Some("separator" | "continuationSeparator" | "continuationNotice")
        ) {
            continue;
        }
        if let Some(id) = note.attr("id") {
            out.insert(id.to_owned(), note.clone());
        }
    }
}

/// A custom style's name rendered as a heading class: interior spaces become hyphens, case is kept.
pub(super) fn style_class(name: &str) -> Text {
    name.replace(' ', "-").into()
}

/// The `custom-style` attribute wrapper for a named Word style.
pub(super) fn custom_style_attr(name: &str) -> Attr {
    Attr {
        id: Text::default(),
        classes: Vec::new(),
        attributes: vec![("custom-style".into(), name.into())],
    }
}

/// The attribute wrapper for highlighted text: a span carrying the `mark` class.
pub(super) fn mark_attr() -> Attr {
    Attr {
        id: Text::default(),
        classes: vec!["mark".into()],
        attributes: Vec::new(),
    }
}

/// The link destination a complex field's instruction points to, or `None` when the field is not a
/// link. A `HYPERLINK` links to its address (with `\l` adding a fragment); a `REF` or `PAGEREF`
/// links to its bookmark only when the `\h` switch requests a hyperlink.
pub(super) fn field_link_target(instr: &str) -> Option<String> {
    let tokens = tokenize_field(instr);
    let (name, rest) = tokens.split_first()?;
    match name.to_ascii_uppercase().as_str() {
        "HYPERLINK" => {
            let mut url: Option<&str> = None;
            let mut anchor: Option<&str> = None;
            let mut index = 0;
            while let Some(token) = rest.get(index) {
                match token.strip_prefix('\\') {
                    // `\l` gives an in-document anchor; `\o` and `\t` carry an argument to ignore.
                    Some("l") => {
                        anchor = rest.get(index + 1).map(String::as_str);
                        index += 2;
                    }
                    Some("o" | "t") => index += 2,
                    Some(_) => index += 1,
                    None => {
                        if url.is_none() {
                            url = Some(token);
                        }
                        index += 1;
                    }
                }
            }
            let mut target = url.unwrap_or_default().to_owned();
            if let Some(anchor) = anchor {
                target.push('#');
                target.push_str(anchor);
            }
            (!target.is_empty()).then_some(target)
        }
        "REF" | "PAGEREF" => {
            let hyperlink = rest.iter().any(|token| token.eq_ignore_ascii_case("\\h"));
            if !hyperlink {
                return None;
            }
            let bookmark = rest.iter().find(|token| !token.starts_with('\\'))?;
            Some(format!("#{bookmark}"))
        }
        _ => None,
    }
}

/// Splits a field instruction into its type keyword, switches, and arguments. Whitespace separates
/// tokens except inside double quotes, where `\"` is a literal quote and `\\` a literal backslash.
fn tokenize_field(instr: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = instr.chars().peekable();
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
            continue;
        }
        let mut token = String::new();
        if ch == '"' {
            chars.next();
            while let Some(ch) = chars.next() {
                match ch {
                    '"' => break,
                    '\\' => match chars.peek() {
                        Some('"' | '\\') => {
                            if let Some(escaped) = chars.next() {
                                token.push(escaped);
                            }
                        }
                        _ => token.push('\\'),
                    },
                    other => token.push(other),
                }
            }
        } else {
            while let Some(&ch) = chars.peek() {
                if ch.is_whitespace() || ch == '"' {
                    break;
                }
                token.push(ch);
                chars.next();
            }
        }
        tokens.push(token);
    }
    tokens
}

/// Reads a drawing's `wp:extent` into `width`/`height` attributes measured in inches.
pub(super) fn image_attr(drawing: &Element) -> Attr {
    let mut attributes = Vec::new();
    if let Some(extent) = drawing.descendant("extent") {
        if let Some(cx) = extent
            .attr("cx")
            .and_then(|value| value.parse::<i64>().ok())
            && cx > 0
        {
            attributes.push(("width".into(), emu_to_inches(cx).into()));
        }
        if let Some(cy) = extent
            .attr("cy")
            .and_then(|value| value.parse::<i64>().ok())
            && cy > 0
        {
            attributes.push(("height".into(), emu_to_inches(cy).into()));
        }
    }
    Attr {
        id: Text::default(),
        classes: Vec::new(),
        attributes,
    }
}

/// Formats an English Metric Unit length as inches (914400 EMU per inch). A whole-number result
/// carries an explicit `.0` fractional part, so the shortest round-tripping decimal always shows a
/// decimal point.
#[allow(clippy::cast_precision_loss)]
fn emu_to_inches(emu: i64) -> String {
    let mut digits = format!("{}", emu as f64 / 914_400.0);
    if !digits.contains('.') {
        digits.push_str(".0");
    }
    format!("{digits}in")
}

/// The proportion `value / total` as a fraction, for column-width ratios.
#[allow(clippy::cast_precision_loss)]
pub(super) fn ratio(value: i64, total: i64) -> f64 {
    value as f64 / total as f64
}

/// A default page's printable width in twips (a US-Letter page less one-inch side margins), the
/// baseline a table's column-width fractions are measured against, independent of the document's own
/// declared page geometry.
pub(super) const DEFAULT_TEXT_WIDTH_TWIPS: i64 = 9360;

/// The width allowance deducted for each boundary between grid columns when sizing table columns.
pub(super) const INTER_COLUMN_TWIPS: i64 = 10;

/// Splits plain text into `Str`/`Space` inlines, collapsing whitespace and trimming the edges.
pub(super) fn tokenize_inlines(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut first = true;
    for word in text.split_whitespace() {
        if !first {
            out.push(Inline::Space);
        }
        out.push(Inline::Str(word.into()));
        first = false;
    }
    out
}

/// Lowercases and trims a style's display name for classification.
pub(super) fn canonical_style(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Whether a canonical paragraph-style name marks a caption, whose content folds into an adjacent
/// image (as a figure) or table.
pub(super) fn is_caption_style(canonical: &str) -> bool {
    matches!(canonical, "caption" | "image caption" | "table caption")
}

/// Whether an inline sequence is exactly one image and nothing else, so its paragraph can become a
/// figure.
pub(super) fn single_image(inlines: &[Inline]) -> bool {
    matches!(inlines, [Inline::Image(..)])
}

/// The heading level a canonical style name denotes, if any (`heading 3` → level 3).
pub(super) fn heading_level(canonical: &str) -> Option<i32> {
    let rest = canonical.strip_prefix("heading ")?;
    let level = rest.trim().parse::<i32>().ok()?;
    (1..=9).contains(&level).then_some(level)
}

/// Whether a canonical style name is one the reader gives dedicated block semantics, so the `styles`
/// extension leaves it alone rather than wrapping it in a `custom-style` container.
pub(super) fn is_builtin_style(canonical: &str) -> bool {
    heading_level(canonical).is_some()
        || matches!(
            canonical,
            "" | "normal"
                | "body text"
                | "first paragraph"
                | "compact"
                | "title"
                | "subtitle"
                | "author"
                | "date"
                | "abstract"
                | "quote"
                | "block text"
                | "intense quote"
                | "block quote"
                | "source code"
                | "verbatim char"
                | "hyperlink"
                | "footnote text"
                | "footnote reference"
        )
}

pub(super) fn alignment(value: &str) -> Alignment {
    match value {
        "left" | "both" => Alignment::AlignLeft,
        "right" => Alignment::AlignRight,
        "center" => Alignment::AlignCenter,
        _ => Alignment::AlignDefault,
    }
}

fn truthy(value: Option<&str>) -> bool {
    matches!(value, Some("1" | "true" | "on"))
}

/// Whether a `w:tblLook` requests first-row conditional formatting. Two encodings coexist: the packed
/// hex bitmask carried by `@w:val`, whose `0x0020` bit selects the first row, and the older boolean
/// `@w:firstRow` attribute. Either one asserting the flag promotes the first row to a header.
pub(super) fn table_look_first_row(look: &Element) -> bool {
    const FIRST_ROW_BIT: u16 = 0x0020;
    let from_mask = look
        .attr("val")
        .and_then(|val| u16::from_str_radix(val.trim(), 16).ok())
        .is_some_and(|bits| bits & FIRST_ROW_BIT != 0);
    from_mask || truthy(look.attr("firstRow"))
}

/// Whether a present `w:tblHeader` row marker is in force. The marker declares its row a header; an
/// explicit `w:val` of `0` switches that off, while any other value, or none, leaves it on.
pub(super) fn tbl_header_on(marker: &Element) -> bool {
    marker.attr("val") != Some("0")
}

pub(super) fn parse_int(value: &str) -> Option<i32> {
    value.trim().parse::<i32>().ok()
}

/// A relationship target reduced to the document-relative media path, dropping any `word/` prefix or
/// `../` segments so it names the media-bag key.
pub(super) fn normalize_target(target: &str) -> String {
    let trimmed = target.trim_start_matches("./");
    let trimmed = trimmed.trim_start_matches("../");
    trimmed.strip_prefix("word/").unwrap_or(trimmed).to_owned()
}

/// Resolves a relationship target against a base directory into an archive part name.
pub(super) fn normalize_part(target: &str, base: &str) -> String {
    let cleaned = target.trim_start_matches("./");
    if cleaned.starts_with("../") {
        return cleaned.trim_start_matches("../").to_owned();
    }
    if base.is_empty() || cleaned.starts_with("word/") || cleaned.contains(":/") {
        cleaned.to_owned()
    } else {
        format!("{base}{cleaned}")
    }
}

/// A conservative MIME type from a media path's extension. Recognized image types come from the
/// shared table; the legacy metafile formats and the fallbacks (an unrecognized extension keeps its
/// own `image/*` subtype, an extensionless path is treated as opaque binary) are docx-specific.
pub(super) fn mime_for(path: &str) -> String {
    if let Some(mime) = carta_core::media::image_mime_for_extension(path) {
        return mime.to_owned();
    }
    match path
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
    {
        Some(ext) => match ext.as_str() {
            "emf" => "image/x-emf".to_owned(),
            "wmf" => "image/x-wmf".to_owned(),
            other => format!("image/{other}"),
        },
        None => "application/octet-stream".to_owned(),
    }
}
