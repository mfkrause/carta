//! Attribute lookup and image-dimension normalization. The attribute accessor serves nearly every
//! writer while the dimension parsers serve only the HTML-family writers, so which items are live
//! depends on the enabled features; unused-item warnings are allowed here rather than gated per item.
#![allow(dead_code)]

use carta_ast::Attr;

/// Look up a key/value attribute by key, returning its value.
pub(crate) fn attribute_value<'a>(attr: &'a Attr, key: &str) -> Option<&'a str> {
    attr.attributes
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

/// A parsed image dimension: a pixel count rendered as a bare HTML attribute, or a length rendered
/// inside a CSS `style` declaration.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Dimension {
    /// A pixel or unitless value, truncated to a whole number and emitted as a bare `width`/`height`
    /// attribute.
    Pixels(u64),
    /// A percentage, emitted in a `style` declaration with one-decimal formatting (`50.0%`).
    Percent(f64),
    /// A physical or font-relative length, emitted in a `style` declaration; the numeric part is
    /// rounded to five fractional digits with trailing zeros dropped, the unit kept verbatim.
    Length(f64, &'static str),
}

/// The length units accepted in an image dimension. Each entry pairs the spelling that may appear in
/// the source value with the unit emitted in the `style` declaration (`inch` normalizes to `in`).
const DIMENSION_UNITS: &[(&str, &str)] = &[
    ("cm", "cm"),
    ("mm", "mm"),
    ("in", "in"),
    ("inch", "in"),
    ("pt", "pt"),
    ("pc", "pc"),
    ("em", "em"),
];

/// Parse an image `width`/`height` value into a [`Dimension`], or `None` when it is not a recognized
/// dimension (an unknown unit, a malformed or signed number, surrounding whitespace), in which case
/// the attribute is dropped. The numeric part is a run of digits with an optional single fractional
/// part, no sign and no surrounding space; a bare number or a `px` suffix is a pixel count, `%` a
/// percentage, and a unit from [`DIMENSION_UNITS`] a physical length.
pub(crate) fn parse_dimension(value: &str) -> Option<Dimension> {
    if let Some(number) = value.strip_suffix('%') {
        let magnitude = parse_dimension_number(number)?;
        return Some(Dimension::Percent(magnitude));
    }
    if let Some(number) = value.strip_suffix("px") {
        let magnitude = parse_dimension_number(number)?;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        return Some(Dimension::Pixels(magnitude.trunc() as u64));
    }
    for (spelling, unit) in DIMENSION_UNITS {
        if let Some(number) = value.strip_suffix(spelling) {
            let magnitude = parse_dimension_number(number)?;
            return Some(Dimension::Length(magnitude, unit));
        }
    }
    let magnitude = parse_dimension_number(value)?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(Dimension::Pixels(magnitude.trunc() as u64))
}

/// Parse the numeric part of a dimension: a run of ASCII digits with an optional single fractional
/// run (`123` or `123.45`), no sign, no leading or trailing dot, no surrounding whitespace. Returns
/// the magnitude, or `None` for any other shape.
fn parse_dimension_number(text: &str) -> Option<f64> {
    let (whole, fraction) = match text.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (text, None),
    };
    let is_digits = |run: &str| !run.is_empty() && run.bytes().all(|byte| byte.is_ascii_digit());
    if !is_digits(whole) {
        return None;
    }
    if let Some(fraction) = fraction
        && !is_digits(fraction)
    {
        return None;
    }
    text.parse::<f64>().ok().filter(|value| value.is_finite())
}

/// Render the CSS value of a percentage dimension: the magnitude with at least one fractional digit
/// (`50` renders as `50.0`), kept to its shortest round-tripping form otherwise.
pub(crate) fn format_percent_dimension(magnitude: f64) -> String {
    format!("{magnitude:?}%")
}

/// Render the CSS value of a length dimension: the magnitude rounded to five fractional digits with
/// trailing zeros (and a bare trailing dot) dropped, followed by the unit.
pub(crate) fn format_length_dimension(magnitude: f64, unit: &str) -> String {
    let rounded = (magnitude * 100_000.0).round() / 100_000.0;
    let mut number = format!("{rounded:.5}");
    if number.contains('.') {
        let trimmed = number.trim_end_matches('0').trim_end_matches('.');
        number.truncate(trimmed.len());
    }
    format!("{number}{unit}")
}

/// Normalize an image's attributes so `width` and `height` render as an HTML `<img>` does: a pixel or
/// unitless value becomes a bare numeric attribute (the `px` stripped), a percentage or physical
/// length folds into a CSS `style` declaration. An unrecognized or malformed value is dropped.
///
/// The resulting key/value order is: the combined `style` (any source `style` value followed by the
/// `width` then `height` style declarations) first, then the remaining attributes in source order
/// with `width`/`height`/`style` removed, then the pixel `width` and `height` attributes last. The
/// `id` and `class` carry over unchanged. A writer renders the returned [`Attr`] with its own
/// attribute renderer, so the dimension rule lives here alone.
pub(crate) fn normalize_image_attr(attr: &Attr) -> Attr {
    let mut base_style: Option<String> = None;
    let mut style_declarations: Vec<String> = Vec::new();
    let mut pixel_attrs: Vec<(String, String)> = Vec::new();
    let mut rest: Vec<(String, String)> = Vec::new();

    let emit_dimension = |key: &str,
                          raw: &str,
                          declarations: &mut Vec<String>,
                          pixels: &mut Vec<(String, String)>| {
        match parse_dimension(raw) {
            Some(Dimension::Pixels(count)) => pixels.push((key.to_owned(), count.to_string())),
            Some(Dimension::Percent(magnitude)) => {
                declarations.push(format!("{key}:{}", format_percent_dimension(magnitude)));
            }
            Some(Dimension::Length(magnitude, unit)) => {
                declarations.push(format!(
                    "{key}:{}",
                    format_length_dimension(magnitude, unit)
                ));
            }
            None => {}
        }
    };

    // Width and height are emitted in a fixed order regardless of their source position; gather them
    // by lookup so a height-before-width source still renders width first.
    if let Some(raw) = attribute_value(attr, "width") {
        emit_dimension("width", raw, &mut style_declarations, &mut pixel_attrs);
    }
    if let Some(raw) = attribute_value(attr, "height") {
        emit_dimension("height", raw, &mut style_declarations, &mut pixel_attrs);
    }

    for (key, value) in &attr.attributes {
        match key.as_str() {
            "width" | "height" => {}
            "style" => base_style = Some(value.to_string()),
            _ => rest.push((key.to_string(), value.to_string())),
        }
    }

    let style = combine_dimension_style(base_style, &style_declarations);

    let mut attributes = Vec::new();
    if let Some(style) = style {
        attributes.push(("style".to_owned(), style));
    }
    attributes.extend(rest);
    attributes.extend(pixel_attrs);

    Attr {
        id: attr.id.clone(),
        classes: attr.classes.clone(),
        attributes: attributes
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect(),
    }
}

/// Combine a source `style` value with the dimension style declarations: a present source value is
/// kept verbatim and the declarations appended after a `;`; with no source style the declarations
/// join with `;`. Yields `None` when there is nothing to emit.
fn combine_dimension_style(base: Option<String>, declarations: &[String]) -> Option<String> {
    let joined = declarations.join(";");
    match base {
        Some(base) if joined.is_empty() => Some(base),
        Some(base) => Some(format!("{base};{joined}")),
        None if joined.is_empty() => None,
        None => Some(joined),
    }
}

/// Split a CSS length into its leading numeric run (digits, `.`, sign) and the trailing unit. A
/// value with no unit yields an empty unit; a value with no numeric prefix yields an empty number.
pub(crate) fn split_length_unit(raw: &str) -> (&str, &str) {
    let boundary = raw
        .find(|ch: char| !(ch.is_ascii_digit() || ch == '.' || ch == '-' || ch == '+'))
        .unwrap_or(raw.len());
    (
        raw.get(..boundary).unwrap_or(raw),
        raw.get(boundary..).unwrap_or(""),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "html")]
    #[test]
    fn parse_dimension_classifies_pixels_percent_and_length() {
        assert_eq!(parse_dimension("200"), Some(Dimension::Pixels(200)));
        assert_eq!(parse_dimension("200px"), Some(Dimension::Pixels(200)));
        // A fractional pixel value truncates toward zero.
        assert_eq!(parse_dimension("200.99px"), Some(Dimension::Pixels(200)));
        assert_eq!(parse_dimension("0.6"), Some(Dimension::Pixels(0)));
        assert_eq!(parse_dimension("0"), Some(Dimension::Pixels(0)));
        assert_eq!(parse_dimension("50%"), Some(Dimension::Percent(50.0)));
        assert_eq!(parse_dimension("12.5%"), Some(Dimension::Percent(12.5)));
        assert_eq!(parse_dimension("5cm"), Some(Dimension::Length(5.0, "cm")));
        assert_eq!(parse_dimension("72pt"), Some(Dimension::Length(72.0, "pt")));
        assert_eq!(parse_dimension("3em"), Some(Dimension::Length(3.0, "em")));
        // `inch` normalizes to the `in` unit.
        assert_eq!(
            parse_dimension("10inch"),
            Some(Dimension::Length(10.0, "in"))
        );
        assert_eq!(parse_dimension("2in"), Some(Dimension::Length(2.0, "in")));
    }

    #[cfg(feature = "html")]
    #[test]
    fn parse_dimension_rejects_unrecognized_shapes() {
        // Unknown units, signs, surrounding space, malformed numbers, and uppercase units all yield
        // no dimension, so the attribute is dropped.
        assert_eq!(parse_dimension("4ex"), None);
        assert_eq!(parse_dimension("50vw"), None);
        assert_eq!(parse_dimension("3rem"), None);
        assert_eq!(parse_dimension("-200"), None);
        assert_eq!(parse_dimension("-50%"), None);
        assert_eq!(parse_dimension("+5cm"), None);
        assert_eq!(parse_dimension(" 5cm "), None);
        assert_eq!(parse_dimension("5 cm"), None);
        assert_eq!(parse_dimension(".5cm"), None);
        assert_eq!(parse_dimension("5.cm"), None);
        assert_eq!(parse_dimension("5CM"), None);
        assert_eq!(parse_dimension("5PX"), None);
        assert_eq!(parse_dimension(""), None);
    }

    #[cfg(feature = "html")]
    #[test]
    fn format_percent_keeps_at_least_one_decimal() {
        assert_eq!(format_percent_dimension(50.0), "50.0%");
        assert_eq!(format_percent_dimension(100.0), "100.0%");
        assert_eq!(format_percent_dimension(0.0), "0.0%");
        assert_eq!(format_percent_dimension(12.5), "12.5%");
        assert_eq!(format_percent_dimension(33.333_333), "33.333333%");
    }

    #[cfg(feature = "html")]
    #[test]
    fn format_length_rounds_and_strips_trailing_zeros() {
        assert_eq!(format_length_dimension(5.0, "cm"), "5cm");
        assert_eq!(format_length_dimension(100.0, "pt"), "100pt");
        assert_eq!(format_length_dimension(2.54, "in"), "2.54in");
        assert_eq!(format_length_dimension(12.3, "cm"), "12.3cm");
        // Rounded to five fractional digits.
        assert_eq!(format_length_dimension(12.345_678_9, "cm"), "12.34568cm");
        assert_eq!(format_length_dimension(1.123_456, "cm"), "1.12346cm");
    }

    #[cfg(feature = "html")]
    #[test]
    fn normalize_image_attr_splits_pixel_and_style_dimensions() {
        let pixels = Attr {
            attributes: vec![("width".into(), "200px".into())],
            ..Attr::default()
        };
        assert_eq!(
            normalize_image_attr(&pixels).attributes,
            vec![("width".into(), "200".into())]
        );
        let percent = Attr {
            attributes: vec![("width".into(), "50%".into())],
            ..Attr::default()
        };
        assert_eq!(
            normalize_image_attr(&percent).attributes,
            vec![("style".into(), "width:50.0%".into())]
        );
    }

    #[cfg(feature = "html")]
    #[test]
    fn normalize_image_attr_orders_style_then_rest_then_pixels() {
        // Source order: a regular pair, a percentage width, a pixel height, another regular pair.
        // The combined style leads, the regular pairs keep their order, the pixel attribute trails.
        let attr = Attr {
            attributes: vec![
                ("data-a".into(), "1".into()),
                ("width".into(), "50%".into()),
                ("height".into(), "200px".into()),
                ("loading".into(), "lazy".into()),
            ],
            ..Attr::default()
        };
        assert_eq!(
            normalize_image_attr(&attr).attributes,
            vec![
                ("style".into(), "width:50.0%".into()),
                ("data-a".into(), "1".into()),
                ("loading".into(), "lazy".into()),
                ("height".into(), "200".into()),
            ]
        );
    }

    #[cfg(feature = "html")]
    #[test]
    fn normalize_image_attr_emits_width_before_height() {
        // Height precedes width in the source, yet width renders first.
        let attr = Attr {
            attributes: vec![
                ("height".into(), "100".into()),
                ("width".into(), "200".into()),
            ],
            ..Attr::default()
        };
        assert_eq!(
            normalize_image_attr(&attr).attributes,
            vec![
                ("width".into(), "200".into()),
                ("height".into(), "100".into()),
            ]
        );
        let both_style = Attr {
            attributes: vec![
                ("height".into(), "5cm".into()),
                ("width".into(), "50%".into()),
            ],
            ..Attr::default()
        };
        assert_eq!(
            normalize_image_attr(&both_style).attributes,
            vec![("style".into(), "width:50.0%;height:5cm".into())]
        );
    }

    #[cfg(feature = "html")]
    #[test]
    fn normalize_image_attr_appends_dimensions_to_existing_style() {
        let attr = Attr {
            attributes: vec![
                ("style".into(), "color:red".into()),
                ("width".into(), "50%".into()),
            ],
            ..Attr::default()
        };
        assert_eq!(
            normalize_image_attr(&attr).attributes,
            vec![("style".into(), "color:red;width:50.0%".into())]
        );
        // A source style with no dimensions still moves ahead of the remaining pairs.
        let style_only = Attr {
            attributes: vec![
                ("data-a".into(), "1".into()),
                ("style".into(), "color:red".into()),
                ("data-b".into(), "2".into()),
            ],
            ..Attr::default()
        };
        assert_eq!(
            normalize_image_attr(&style_only).attributes,
            vec![
                ("style".into(), "color:red".into()),
                ("data-a".into(), "1".into()),
                ("data-b".into(), "2".into()),
            ]
        );
    }

    #[cfg(feature = "html")]
    #[test]
    fn normalize_image_attr_drops_unrecognized_dimensions_and_keeps_id_class() {
        let attr = Attr {
            id: "x".into(),
            classes: vec!["c".into()],
            attributes: vec![
                ("width".into(), "4ex".into()),
                ("height".into(), "100".into()),
            ],
        };
        let normalized = normalize_image_attr(&attr);
        assert_eq!(normalized.id, "x");
        assert_eq!(normalized.classes, vec!["c".to_owned()]);
        // The unparsable width is dropped; the pixel height survives.
        assert_eq!(normalized.attributes, vec![("height".into(), "100".into())]);
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
}
