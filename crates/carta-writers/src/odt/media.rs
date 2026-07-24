//! Image dimension probing and frame sizing for the ODT writer.

use carta_ast::Attr;

use crate::image_size::{
    gif_dimensions, jpeg_dimensions, png_dimensions, read_be_u16, webp_dimensions,
};

use super::helpers::attr_value;

/// The intrinsic pixel dimensions of an encoded image paired with the pixel density (horizontal,
/// vertical dots per inch) that maps those pixels to a physical size. Unrecognized data resolves to
/// a square default so a frame still gets a sensible size rather than a degenerate zero one.
pub(super) fn image_metrics(bytes: &[u8]) -> ((u32, u32), (f64, f64)) {
    if let Some(dimensions) = png_dimensions(bytes) {
        return (dimensions, (72.0, 72.0));
    }
    if let Some(dimensions) = gif_dimensions(bytes) {
        return (dimensions, (72.0, 72.0));
    }
    if let Some(dimensions) = jpeg_dimensions(bytes) {
        return (dimensions, jpeg_density(bytes).unwrap_or((72.0, 72.0)));
    }
    if let Some(dimensions) = webp_dimensions(bytes) {
        return (dimensions, (96.0, 96.0));
    }
    if let Some(dimensions) = svg_dimensions(bytes) {
        return (dimensions, (96.0, 96.0));
    }
    ((100, 100), (72.0, 72.0))
}

/// The pixel dimensions declared by an SVG document's `<svg>` element: its `width` and `height`
/// attributes when both are present, otherwise the extents given by its `viewBox`.
fn svg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let text = core::str::from_utf8(bytes).ok()?;
    let opening = text.get(text.find("<svg")?..)?;
    let tag = opening.get(..opening.find('>')?)?;
    let width = svg_attribute(tag, "width").and_then(|value| svg_length_pixels(&value));
    let height = svg_attribute(tag, "height").and_then(|value| svg_length_pixels(&value));
    if let (Some(w), Some(h)) = (width, height) {
        return Some((w, h));
    }
    let view_box = svg_attribute(tag, "viewBox")?;
    let mut extents = view_box
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|part| !part.is_empty());
    let w = extents.nth(2)?;
    let h = extents.next()?;
    Some((svg_number_pixels(w)?, svg_number_pixels(h)?))
}

/// Extracts the value of a whole-token attribute from an element's opening tag, ignoring names that
/// merely occur as a suffix of another attribute (so `width` is not read out of `stroke-width`).
fn svg_attribute(tag: &str, name: &str) -> Option<String> {
    let bytes = tag.as_bytes();
    let mut from = 0usize;
    while let Some(offset) = tag.get(from..)?.find(name) {
        let index = from + offset;
        let before = index.checked_sub(1).and_then(|i| bytes.get(i)).copied();
        let after = bytes.get(index + name.len()).copied();
        let starts = before.is_none_or(|b| b.is_ascii_whitespace() || b == b'<');
        let ends = after.is_some_and(|b| b == b'=' || b.is_ascii_whitespace());
        if starts && ends {
            let tail = tag.get(index + name.len()..)?;
            let after_equals = tail.get(tail.find('=')? + 1..)?.trim_start();
            let quote = after_equals.chars().next()?;
            if quote == '"' || quote == '\'' {
                let value = after_equals.get(1..)?;
                return value.get(..value.find(quote)?).map(str::to_string);
            }
            return None;
        }
        from = index + name.len();
    }
    None
}

/// Rounds a pixel measure to the nearest whole pixel. The float-to-integer cast saturates, so a
/// negative, non-finite, or oversized measure maps to `0` or `u32::MAX` rather than wrapping.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn round_to_pixels(measure: f64) -> u32 {
    measure.round() as u32
}

/// Converts a CSS length (a number with an optional unit) to pixels at 96 dots per inch; a bare
/// number is already in pixels.
fn svg_length_pixels(value: &str) -> Option<u32> {
    let text = value.trim();
    let units: [(&str, f64); 7] = [
        ("px", 1.0),
        ("pt", 96.0 / 72.0),
        ("pc", 16.0),
        ("in", 96.0),
        ("cm", 96.0 / 2.54),
        ("mm", 96.0 / 25.4),
        ("em", 16.0),
    ];
    for (unit, factor) in units {
        if let Some(number) = text.strip_suffix(unit)
            && let Ok(measure) = number.trim().parse::<f64>()
        {
            return Some(round_to_pixels(measure * factor));
        }
    }
    text.parse::<f64>().ok().map(round_to_pixels)
}

/// A `viewBox` extent is expressed in unit-less user-space coordinates, which map one to one to
/// pixels.
fn svg_number_pixels(value: &str) -> Option<u32> {
    value.trim().parse::<f64>().ok().map(round_to_pixels)
}

fn jpeg_density(bytes: &[u8]) -> Option<(f64, f64)> {
    let start: [u8; 2] = bytes.get(0..2)?.try_into().ok()?;
    if start != [0xFF, 0xD8] {
        return None;
    }
    let mut pos = 2usize;
    for _ in 0..8192 {
        if *bytes.get(pos)? != 0xFF {
            return None;
        }
        let mut marker_pos = pos + 1;
        while *bytes.get(marker_pos)? == 0xFF {
            marker_pos += 1;
        }
        let marker = *bytes.get(marker_pos)?;
        let segment = marker_pos + 1;
        let length = read_be_u16(bytes, segment)? as usize;
        match marker {
            // The JFIF application header records a density in dots per inch or per centimetre.
            0xE0 if bytes.get(segment + 2..segment + 7)? == b"JFIF\0" => {
                let units = *bytes.get(segment + 9)?;
                let x = f64::from(read_be_u16(bytes, segment + 10)?);
                let y = f64::from(read_be_u16(bytes, segment + 12)?);
                if x > 0.0 && y > 0.0 {
                    return match units {
                        1 => Some((x, y)),
                        2 => Some((x * 2.54, y * 2.54)),
                        _ => None,
                    };
                }
                pos = segment + length;
            }
            0xD8..=0xDA => return None,
            _ => pos = segment + length,
        }
    }
    None
}

/// A parsed image dimension: a percentage of the available width, or an absolute length already
/// resolved to inches.
enum Dimension {
    Percent(f64),
    Inches(f64),
}

/// The size attributes for an image frame. An explicit `width`/`height` attribute is resolved to
/// inches; a single explicit dimension carries the other along the image's aspect ratio. Absent
/// both, the frame takes the intrinsic pixel size mapped to points through the image's density.
pub(super) fn image_size(attr: &Attr, width: u32, height: u32, density: (f64, f64)) -> String {
    let (dpi_x, dpi_y) = density;
    let requested_width = attr_value(attr, "width").and_then(parse_dimension);
    let requested_height = attr_value(attr, "height").and_then(parse_dimension);
    let natural_width = format!("{}pt", show_number(f64::from(width) * 72.0 / dpi_x));
    let natural_height = format!("{}pt", show_number(f64::from(height) * 72.0 / dpi_y));

    if let Some(Dimension::Percent(percent)) = &requested_width {
        return format!(
            " style:rel-width=\"{percent:.1}%\" style:rel-height=\"scale\" \
             svg:width=\"{natural_width}\" svg:height=\"{natural_height}\""
        );
    }

    let width_inches = match requested_width {
        Some(Dimension::Inches(value)) => Some(value),
        _ => None,
    };
    let height_inches = match requested_height {
        Some(Dimension::Inches(value)) => Some(value),
        _ => None,
    };

    let (final_width, final_height) = match (width_inches, height_inches) {
        (Some(w), Some(h)) => (inches(&show_inches(w)), inches(&show_inches(h))),
        (Some(w), None) => {
            let h = if width > 0 {
                inches(&show_number(w * (f64::from(height) / f64::from(width))))
            } else {
                natural_height
            };
            (inches(&show_inches(w)), h)
        }
        (None, Some(h)) => {
            let w = if height > 0 {
                inches(&show_number(h * (f64::from(width) / f64::from(height))))
            } else {
                natural_width
            };
            (w, inches(&show_inches(h)))
        }
        (None, None) => (natural_width, natural_height),
    };
    format!(" svg:width=\"{final_width}\" svg:height=\"{final_height}\"")
}

fn inches(value: &str) -> String {
    format!("{value}in")
}

/// Parses an image dimension attribute into a percentage or an absolute length in inches. A bare
/// number and `px` are pixels at 96 per inch; other units convert to inches by their fixed ratio.
fn parse_dimension(raw: &str) -> Option<Dimension> {
    let text = raw.trim();
    if let Some(number) = text.strip_suffix('%') {
        return number.trim().parse::<f64>().ok().map(Dimension::Percent);
    }
    let units: [(&str, f64); 7] = [
        ("in", 1.0),
        ("cm", 0.393_700_787_4),
        ("mm", 0.039_370_078_74),
        ("pt", 1.0 / 72.0),
        ("pc", 1.0 / 6.0),
        ("em", 0.171_875),
        ("px", 1.0 / 96.0),
    ];
    for (unit, factor) in units {
        if let Some(number) = text.strip_suffix(unit)
            && let Ok(value) = number.trim().parse::<f64>()
        {
            return Some(Dimension::Inches(value * factor));
        }
    }
    text.parse::<f64>()
        .ok()
        .map(|value| Dimension::Inches(value / 96.0))
}

/// Formats an explicitly requested length in inches: rounded to five decimals, with trailing zeros
/// and a trailing point trimmed.
fn show_inches(value: f64) -> String {
    let text = format!("{value:.5}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Formats a derived or intrinsic length at full precision, using the shortest decimal that round
/// trips: a plain decimal (always with a fractional digit) for magnitudes in `[0.1, 1e7)`, and
/// scientific notation otherwise.
fn show_number(value: f64) -> String {
    if value == 0.0 {
        return "0.0".to_string();
    }
    let negative = value.is_sign_negative();
    let plain = format!("{}", value.abs());
    let (integer_part, fraction_part) = plain.split_once('.').unwrap_or((plain.as_str(), ""));
    let mut digits = String::with_capacity(integer_part.len() + fraction_part.len());
    digits.push_str(integer_part);
    digits.push_str(fraction_part);
    let significant = digits.trim_start_matches('0');
    let leading_zeros = digits.len() - significant.len();
    // Both operands are lengths of a formatted decimal string, far below `isize::MAX`.
    #[allow(clippy::cast_possible_wrap)]
    let point = integer_part.len() as isize - leading_zeros as isize;
    let significant = significant.trim_end_matches('0');
    if significant.is_empty() {
        return "0.0".to_string();
    }
    let magnitude = value.abs();
    let body = if (0.1..1e7).contains(&magnitude) {
        format_fixed(significant, point)
    } else {
        format_scientific(significant, point - 1)
    };
    if negative { format!("-{body}") } else { body }
}

/// Renders significant digits in fixed-point form, `point` giving how many digits precede the
/// decimal separator.
fn format_fixed(significant: &str, point: isize) -> String {
    if point <= 0 {
        let zeros = "0".repeat(point.unsigned_abs());
        return format!("0.{zeros}{significant}");
    }
    let point = point.unsigned_abs();
    if significant.len() <= point {
        let padding = "0".repeat(point - significant.len());
        return format!("{significant}{padding}.0");
    }
    let head: String = significant.chars().take(point).collect();
    let tail: String = significant.chars().skip(point).collect();
    format!("{head}.{tail}")
}

/// Renders significant digits in scientific form `d.ddde±p`, with a single leading digit.
fn format_scientific(significant: &str, exponent: isize) -> String {
    let mut chars = significant.chars();
    let lead = chars.next().unwrap_or('0');
    let rest: String = chars.collect();
    let mantissa = if rest.is_empty() {
        format!("{lead}.0")
    } else {
        format!("{lead}.{rest}")
    };
    format!("{mantissa}e{exponent}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    fn attr_with(pairs: &[(&str, &str)]) -> Attr {
        Attr {
            attributes: pairs.iter().map(|&(k, v)| (k.into(), v.into())).collect(),
            ..Attr::default()
        }
    }

    /// A minimal JFIF `APP0` segment carrying an X/Y density of 300 under the given unit code.
    fn jfif(units: u8) -> [u8; 20] {
        [
            0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, units,
            0x01, 0x2c, 0x01, 0x2c, 0x00, 0x00,
        ]
    }

    #[test]
    fn image_metrics_reads_png_dimensions() {
        let png = [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x78, 0x00, 0x00, 0x00, 0x50,
        ];
        let ((width, height), (dpi_x, dpi_y)) = image_metrics(&png);
        assert_eq!((width, height), (120, 80));
        assert!(approx(dpi_x, 72.0) && approx(dpi_y, 72.0));
    }

    #[test]
    fn image_metrics_reads_jpeg_dimensions_and_jfif_density() {
        let jpeg = [
            0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x01,
            0x01, 0x2c, 0x01, 0x2c, 0x00, 0x00, 0xff, 0xc0, 0x00, 0x11, 0x08, 0x00, 0x50, 0x00,
            0x78, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        let ((width, height), (dpi_x, dpi_y)) = image_metrics(&jpeg);
        assert_eq!((width, height), (120, 80));
        assert!(approx(dpi_x, 300.0) && approx(dpi_y, 300.0));
    }

    #[test]
    fn image_metrics_reads_svg_then_falls_back_to_default() {
        let ((width, height), (dpi_x, dpi_y)) =
            image_metrics(b"<svg width=\"120\" height=\"80\"/>");
        assert_eq!((width, height), (120, 80));
        assert!(approx(dpi_x, 96.0) && approx(dpi_y, 96.0));

        let ((fallback_width, fallback_height), (fallback_x, fallback_y)) =
            image_metrics(b"not an image");
        assert_eq!((fallback_width, fallback_height), (100, 100));
        assert!(approx(fallback_x, 72.0) && approx(fallback_y, 72.0));
    }

    #[test]
    fn svg_dimensions_prefers_explicit_width_and_height() {
        assert_eq!(
            svg_dimensions(b"<svg width=\"120\" height=\"80\">"),
            Some((120, 80))
        );
        assert_eq!(
            svg_dimensions(b"<svg width='60' height='30'>"),
            Some((60, 30))
        );
    }

    #[test]
    fn svg_dimensions_converts_length_units() {
        assert_eq!(
            svg_dimensions(b"<svg width=\"1in\" height=\"2in\">"),
            Some((96, 192))
        );
    }

    #[test]
    fn svg_dimensions_ignores_attribute_name_suffixes() {
        assert_eq!(
            svg_dimensions(b"<svg stroke-width=\"3\" width=\"50\" height=\"40\">"),
            Some((50, 40))
        );
    }

    #[test]
    fn svg_dimensions_reads_view_box_without_explicit_size() {
        assert_eq!(
            svg_dimensions(b"<svg viewBox=\"0 0 200 100\">"),
            Some((200, 100))
        );
    }

    #[test]
    fn svg_dimensions_without_any_metric_is_none() {
        assert_eq!(svg_dimensions(b"<svg></svg>"), None);
        assert_eq!(svg_dimensions(b"no svg here"), None);
    }

    #[test]
    fn svg_length_pixels_covers_each_unit() {
        assert_eq!(svg_length_pixels("10px"), Some(10));
        assert_eq!(svg_length_pixels("72pt"), Some(96));
        assert_eq!(svg_length_pixels("1pc"), Some(16));
        assert_eq!(svg_length_pixels("1in"), Some(96));
        assert_eq!(svg_length_pixels("2.54cm"), Some(96));
        assert_eq!(svg_length_pixels("25.4mm"), Some(96));
        assert_eq!(svg_length_pixels("1em"), Some(16));
        assert_eq!(svg_length_pixels("42"), Some(42));
        assert_eq!(svg_length_pixels("nonsense"), None);
    }

    #[test]
    fn svg_number_pixels_rounds_to_nearest() {
        assert_eq!(svg_number_pixels("200"), Some(200));
        assert_eq!(svg_number_pixels("10.6"), Some(11));
        assert_eq!(svg_number_pixels("bad"), None);
    }

    #[test]
    fn jpeg_density_reads_units_and_rejects_others() {
        let (dpi_x, dpi_y) = jpeg_density(&jfif(0x01)).unwrap();
        assert!(approx(dpi_x, 300.0) && approx(dpi_y, 300.0));

        let (cm_x, cm_y) = jpeg_density(&jfif(0x02)).unwrap();
        assert!(approx(cm_x, 300.0 * 2.54) && approx(cm_y, 300.0 * 2.54));

        assert_eq!(jpeg_density(&jfif(0x00)), None);
        assert_eq!(jpeg_density(b"not a jpeg"), None);
    }

    #[test]
    fn image_size_scales_percentage_width() {
        let attr = attr_with(&[("width", "50%")]);
        assert_eq!(
            image_size(&attr, 100, 100, (72.0, 72.0)),
            " style:rel-width=\"50.0%\" style:rel-height=\"scale\" \
             svg:width=\"100.0pt\" svg:height=\"100.0pt\""
        );
    }

    #[test]
    fn image_size_resolves_absolute_dimensions() {
        let both = attr_with(&[("width", "2in"), ("height", "3in")]);
        assert_eq!(
            image_size(&both, 100, 100, (72.0, 72.0)),
            " svg:width=\"2in\" svg:height=\"3in\""
        );

        let width_only = attr_with(&[("width", "2in")]);
        assert_eq!(
            image_size(&width_only, 100, 100, (72.0, 72.0)),
            " svg:width=\"2in\" svg:height=\"2.0in\""
        );

        let height_only = attr_with(&[("height", "3in")]);
        assert_eq!(
            image_size(&height_only, 100, 100, (72.0, 72.0)),
            " svg:width=\"3.0in\" svg:height=\"3in\""
        );
    }

    #[test]
    fn image_size_uses_intrinsic_size_without_attributes() {
        let attr = Attr::default();
        assert_eq!(
            image_size(&attr, 200, 100, (72.0, 72.0)),
            " svg:width=\"200.0pt\" svg:height=\"100.0pt\""
        );
    }

    #[test]
    fn image_size_falls_back_when_intrinsic_axis_is_zero() {
        let width_only = attr_with(&[("width", "2in")]);
        assert_eq!(
            image_size(&width_only, 0, 100, (72.0, 72.0)),
            " svg:width=\"2in\" svg:height=\"100.0pt\""
        );

        let height_only = attr_with(&[("height", "3in")]);
        assert_eq!(
            image_size(&height_only, 100, 0, (72.0, 72.0)),
            " svg:width=\"100.0pt\" svg:height=\"3in\""
        );
    }

    #[test]
    fn image_size_parses_every_length_unit() {
        for unit in ["1in", "96px", "72pt", "6pc", "2.54cm", "25.4mm", "1em"] {
            let attr = attr_with(&[("width", unit)]);
            let rendered = image_size(&attr, 100, 100, (96.0, 96.0));
            assert!(rendered.contains("svg:width=") && !rendered.contains("rel-width"));
        }
        let bare = attr_with(&[("width", "192")]);
        assert!(image_size(&bare, 100, 100, (96.0, 96.0)).contains("svg:width=\"2in\""));
    }

    #[test]
    fn show_number_formats_fixed_and_scientific() {
        assert_eq!(show_number(0.0), "0.0");
        assert_eq!(show_number(2.0), "2.0");
        assert_eq!(show_number(1.5), "1.5");
        assert_eq!(show_number(0.5), "0.5");
        assert_eq!(show_number(0.25), "0.25");
        assert_eq!(show_number(12.5), "12.5");
        assert_eq!(show_number(300.0), "300.0");
        assert_eq!(show_number(0.05), "5.0e-2");
        assert_eq!(show_number(100_000_000.0), "1.0e8");
        assert_eq!(show_number(150_000_000.0), "1.5e8");
        assert_eq!(show_number(123_456_789.0), "1.23456789e8");
        assert_eq!(show_number(-3.5), "-3.5");
        assert_eq!(show_number(0.000_000_01), "1.0e-8");
    }

    #[test]
    fn show_inches_trims_trailing_zeros() {
        assert_eq!(show_inches(2.0), "2");
        assert_eq!(show_inches(1.5), "1.5");
        assert_eq!(show_inches(1.25), "1.25");
        assert_eq!(show_inches(0.5), "0.5");
        assert_eq!(show_inches(100.0), "100");
    }
}
