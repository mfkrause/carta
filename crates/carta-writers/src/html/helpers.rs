//! Element, attribute, and table helpers shared by the html serializer.

use std::fmt::Write as _;

use carta_ast::{
    Alignment, Attr, Block, Caption, ColSpec, ColWidth, Inline, ListNumberStyle, Target, Text,
    to_plain_text,
};

use crate::common::{is_known_attribute, normalize_image_attr};

use super::{AttrOrder, BREAK, Flavor, escape_attr, escape_attr_into, protect};

pub(super) fn image(attr: &Attr, inlines: &[Inline], target: &Target, flavor: Flavor) -> String {
    let alt = to_plain_text(inlines);
    // EPUB XHTML always carries `alt` (possibly empty); other flavors omit an empty one
    let alt_attr = if inlines.is_empty() && !matches!(flavor, Flavor::Epub3 | Flavor::Epub2) {
        String::new()
    } else {
        format!("{BREAK}alt=\"{}\"", escape_attr(&alt))
    };
    let source = match flavor {
        Flavor::Slides => "data-src",
        Flavor::Html5 | Flavor::Html4 | Flavor::Epub3 | Flavor::Epub2 => "src",
    };
    let mut out = String::from("<img");
    out.push(BREAK);
    let _ = write!(out, "{source}=\"");
    escape_attr_into(&mut out, &target.url);
    out.push('"');
    out.push_str(&title_attr(&target.title));
    render_attr_into(
        &mut out,
        &normalize_image_attr(attr),
        AttrOrder::Standard,
        flavor,
    );
    out.push_str(&alt_attr);
    // literal space, not a break point: `/>` stays glued to the last attribute even past the fill column
    out.push(' ');
    out.push_str("/>");
    out
}

/// Whether a figure's body is a single captioned image whose alt text reads the same as its
/// caption. Such a caption is marked `aria-hidden="true"` so a screen reader does not announce the
/// duplicated text twice. The comparison is on plain text, so markup that leaves the spoken words
/// unchanged (emphasis, say) still counts as a match.
pub(super) fn is_implicit_figure(caption: &Caption, blocks: &[Block]) -> bool {
    let [Block::Plain(plain)] = blocks else {
        return false;
    };
    let [Inline::Image(_, alt, _)] = plain.as_slice() else {
        return false;
    };
    let [Block::Para(cap) | Block::Plain(cap)] = caption.long.as_slice() else {
        return false;
    };
    carta_ast::to_plain_text(cap) == carta_ast::to_plain_text(alt)
}

/// A list item is a task-list entry when its first block opens with a ballot-box character followed
/// by a space; the boolean reports whether the box is checked.
pub(super) fn checkbox_state(item: &[Block]) -> Option<bool> {
    let (Block::Plain(inlines) | Block::Para(inlines)) = item.first()? else {
        return None;
    };
    let [Inline::Str(marker), Inline::Space, ..] = inlines.as_slice() else {
        return None;
    };
    match marker.as_str() {
        "\u{2610}" => Some(false),
        "\u{2612}" => Some(true),
        _ => None,
    }
}

fn has_explicit_widths(specs: &[ColSpec]) -> bool {
    specs
        .iter()
        .any(|spec| matches!(spec.width, ColWidth::ColWidth(_)))
}

pub(super) fn colgroup(specs: &[ColSpec], flavor: Flavor) -> String {
    if !has_explicit_widths(specs) {
        return String::new();
    }
    let cols: Vec<String> = specs
        .iter()
        .map(|spec| match spec.width {
            ColWidth::ColWidth(width) if flavor.is_html5_family() => {
                format!("<col style=\"width: {}%\" />", width_percent(width))
            }
            ColWidth::ColWidth(width) => format!("<col width=\"{}%\" />", width_percent(width)),
            ColWidth::ColWidthDefault => "<col />".to_owned(),
        })
        .collect();
    format!("\n<colgroup>\n{}\n</colgroup>", cols.join("\n"))
}

/// The `style="width:N%;"` a table carries when its explicit column widths leave it narrower
/// than the page: the column fractions summed and rounded to a whole percent. Empty when every
/// column uses the default width, and also when the fractions already cover the full width.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(super) fn table_width_style(specs: &[ColSpec]) -> String {
    if !has_explicit_widths(specs) {
        return String::new();
    }
    let total: f64 = specs
        .iter()
        .map(|spec| match spec.width {
            ColWidth::ColWidth(width) => width,
            ColWidth::ColWidthDefault => 0.0,
        })
        .sum();
    if total >= 1.0 {
        return String::new();
    }
    format!(
        "{BREAK}style=\"width:{}%;\"",
        (total * 100.0).round() as u32
    )
}

/// Append a newline to `text` unless it is empty (used to separate a footnote's leading blocks
/// from the paragraph that carries the backlink).
pub(super) fn append_trailing_newline(text: &mut String) {
    if !text.is_empty() {
        text.push('\n');
    }
}

pub(super) fn title_attr(title: &Text) -> String {
    if title.is_empty() {
        String::new()
    } else {
        format!("{BREAK}title=\"{}\"", escape_attr(title))
    }
}

pub(super) fn header_tag(level: i32) -> &'static str {
    const TAGS: [&str; 6] = ["h1", "h2", "h3", "h4", "h5", "h6"];
    let index = usize::try_from(level.clamp(1, 6) - 1).unwrap_or(0);
    TAGS.get(index).copied().unwrap_or("h1")
}

pub(super) fn ordered_list_type(style: ListNumberStyle) -> Option<&'static str> {
    match style {
        ListNumberStyle::DefaultStyle => None,
        ListNumberStyle::Decimal | ListNumberStyle::Example => Some("1"),
        ListNumberStyle::LowerAlpha => Some("a"),
        ListNumberStyle::UpperAlpha => Some("A"),
        ListNumberStyle::LowerRoman => Some("i"),
        ListNumberStyle::UpperRoman => Some("I"),
    }
}

/// The CSS `list-style-type` name for an ordered list's numbering, or `None` for the default style
/// (which carries no explicit list-style declaration).
pub(super) fn list_style_type(style: ListNumberStyle) -> Option<&'static str> {
    match style {
        ListNumberStyle::DefaultStyle => None,
        ListNumberStyle::Decimal | ListNumberStyle::Example => Some("decimal"),
        ListNumberStyle::LowerAlpha => Some("lower-alpha"),
        ListNumberStyle::UpperAlpha => Some("upper-alpha"),
        ListNumberStyle::LowerRoman => Some("lower-roman"),
        ListNumberStyle::UpperRoman => Some("upper-roman"),
    }
}

/// The `align="…"` attribute value for a cell's effective alignment, or `None` for the default
/// (which carries no alignment attribute).
fn alignment_word(align: &Alignment) -> Option<&'static str> {
    match align {
        Alignment::AlignLeft => Some("left"),
        Alignment::AlignRight => Some("right"),
        Alignment::AlignCenter => Some("center"),
        Alignment::AlignDefault => None,
    }
}

pub(super) fn alignment_style(align: &Alignment) -> Option<&'static str> {
    match align {
        Alignment::AlignLeft => Some("text-align: left;"),
        Alignment::AlignRight => Some("text-align: right;"),
        Alignment::AlignCenter => Some("text-align: center;"),
        Alignment::AlignDefault => None,
    }
}

/// A column width fraction as a whole-percent integer: the fraction times 100, floored.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn width_percent(fraction: f64) -> u32 {
    (fraction * 100.0).floor() as u32
}

/// Emit a raw-passthrough payload verbatim when its format targets HTML, else drop it (other
/// target formats produce no output in an HTML document).
pub(super) fn raw_passthrough(format: &str, text: &str) -> String {
    if matches!(format, "html" | "html5" | "html4") {
        protect(text)
    } else {
        String::new()
    }
}

/// Renders an [`Attr`] to its HTML attribute string (with a leading space when non-empty). The
/// field order depends on [`AttrOrder`]; the spelling of non-standard attribute keys depends on the
/// [`Flavor`].
pub(super) fn render_attr_into(out: &mut String, attr: &Attr, order: AttrOrder, flavor: Flavor) {
    match order {
        AttrOrder::Standard => {
            render_id_into(out, &attr.id);
            render_class_into(out, &attr.classes);
            render_keyvals_into(out, &attr.attributes, flavor);
        }
        AttrOrder::Header => {
            render_class_into(out, &attr.classes);
            render_keyvals_into(out, &attr.attributes, flavor);
            render_id_into(out, &attr.id);
        }
    }
}

/// The HTML4-valid universal attributes for a heading element. HTML4 admits only the core, i18n,
/// and presentational attributes plus event handlers on `<hN>`; any other key/value pair is
/// dropped rather than carried through under a `data-` prefix.
pub(super) fn heading_attr_html4(attr: &Attr) -> Attr {
    let attributes = attr
        .attributes
        .iter()
        .filter(|(key, _)| is_html4_universal_attribute(key))
        .cloned()
        .collect();
    Attr {
        id: attr.id.clone(),
        classes: attr.classes.clone(),
        attributes,
    }
}

/// Whether a key is admissible on any HTML4 element: the core attributes (`style`, `title`, `class`,
/// `id` are handled separately), the i18n attributes, the presentational `align`, and the intrinsic
/// event handlers (`on…`).
fn is_html4_universal_attribute(key: &str) -> bool {
    matches!(key, "style" | "title" | "lang" | "dir" | "align") || key.starts_with("on")
}

/// The presentational dimension attributes HTML4 admits on the elements that carry them (an image,
/// a table cell or column): a pixel `width` or `height`. Percentage and length dimensions fold into a
/// `style` declaration upstream, so only bare pixel counts reach the attribute renderer, where the
/// strict XHTML 1.1 dialect would otherwise drop them as unknown.
fn is_html4_dimension_attribute(key: &str) -> bool {
    matches!(key, "width" | "height")
}

/// Render a table cell's attributes for the HTML4 dialect: id, class, an explicit `align="…"`
/// attribute for the effective alignment, then the cell's own key/value pairs verbatim.
pub(super) fn cell_attr_html4(attr: &Attr, align: &Alignment, flavor: Flavor) -> String {
    let mut out = String::new();
    render_id_into(&mut out, &attr.id);
    render_class_into(&mut out, &attr.classes);
    if let Some(word) = alignment_word(align) {
        let _ = write!(out, "{BREAK}align=\"{word}\"");
    }
    render_keyvals_into(&mut out, &attr.attributes, flavor);
    out
}

/// Render a table cell's attributes, folding the column's alignment into the `style` declaration.
/// The alignment prefixes any existing `style` value (at that value's position); with no `style`
/// attribute present, an alignment-only `style` is emitted as the first key/value pair, after id and
/// class. With no alignment the attributes render unchanged.
pub(super) fn cell_attr(attr: &Attr, align_style: Option<&str>) -> String {
    let mut out = String::new();
    render_id_into(&mut out, &attr.id);
    render_class_into(&mut out, &attr.classes);
    let Some(align_style) = align_style else {
        render_keyvals_into(&mut out, &attr.attributes, Flavor::Html5);
        return out;
    };
    let mut keyvals = String::new();
    let mut merged = false;
    for (key, value) in &attr.attributes {
        if key.is_empty() {
            continue;
        }
        keyvals.push(BREAK);
        if key == "style" {
            let combined = combine_style(align_style, value);
            keyvals.push_str("style=\"");
            escape_attr_into(&mut keyvals, &combined);
            keyvals.push('"');
            merged = true;
        } else {
            if !is_known_attribute(key) {
                keyvals.push_str("data-");
            }
            keyvals.push_str(key);
            keyvals.push_str("=\"");
            escape_attr_into(&mut keyvals, value);
            keyvals.push('"');
        }
    }
    if !merged {
        let _ = write!(out, "{BREAK}style=\"{align_style}\"");
    }
    out.push_str(&keyvals);
    out
}

/// Prefix a `style` value with an alignment declaration, ensuring the result ends with a semicolon.
fn combine_style(align_style: &str, style: &str) -> String {
    let trimmed = style.trim();
    let suffix = if trimmed.ends_with(';') { "" } else { ";" };
    format!("{align_style} {trimmed}{suffix}")
}

pub(super) fn render_id_into(out: &mut String, id: &Text) {
    if id.is_empty() {
        return;
    }
    out.push(BREAK);
    out.push_str("id=\"");
    escape_attr_into(out, id);
    out.push('"');
}

pub(super) fn render_class_into(out: &mut String, classes: &[Text]) {
    if classes.iter().all(Text::is_empty) {
        return;
    }
    out.push(BREAK);
    out.push_str("class=\"");
    let mut first = true;
    for class in classes {
        if class.is_empty() {
            continue;
        }
        if !first {
            out.push(' ');
        }
        escape_attr_into(out, class);
        first = false;
    }
    out.push('"');
}

/// Render an attribute set's key/value pairs. In the html5 dialect a non-standard key is carried
/// through under a `data-` prefix; in html4 it is emitted by its bare name. The EPUB 2 dialect
/// targets XHTML 1.1, which admits no such extension attributes, so any key that is not a universal
/// html4 attribute is dropped rather than carried through.
pub(super) fn render_keyvals_into(out: &mut String, attributes: &[(Text, Text)], flavor: Flavor) {
    for (key, value) in attributes {
        if key.is_empty() {
            continue;
        }
        let prefixed = matches!(flavor, Flavor::Html5 | Flavor::Slides | Flavor::Epub3)
            && !is_known_attribute(key);
        let dropped = flavor == Flavor::Epub2
            && !is_html4_universal_attribute(key)
            && !is_html4_dimension_attribute(key);
        if dropped {
            continue;
        }
        out.push(BREAK);
        if prefixed {
            out.push_str("data-");
        }
        out.push_str(key);
        out.push_str("=\"");
        escape_attr_into(out, value);
        out.push('"');
    }
}
