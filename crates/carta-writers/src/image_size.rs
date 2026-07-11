//! Pixel dimensions read from an image's own header bytes, for the container writers that size
//! embedded pictures. Only the three raster formats a word-processor or e-book package embeds
//! directly are recognized; anything else, or a header too short to parse, reports `(0, 0)`.

/// The pixel dimensions of an image, read from its header. Returns `(0, 0)` for a format that is not
/// recognized or a header that is too short to parse.
pub(crate) fn image_dimensions(bytes: &[u8]) -> (u32, u32) {
    png_dimensions(bytes)
        .or_else(|| gif_dimensions(bytes))
        .or_else(|| jpeg_dimensions(bytes))
        .unwrap_or((0, 0))
}

/// The dimensions in a PNG's `IHDR` chunk, or `None` when the signature does not match.
pub(crate) fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..8) != Some(PNG_SIGNATURE) {
        return None;
    }
    Some((read_be_u32(bytes, 16)?, read_be_u32(bytes, 20)?))
}

/// The horizontal and vertical resolution of an image in dots per inch, read from its header. A
/// PNG's `pHYs` chunk in the meter unit and a JPEG's JFIF pixel-density fields are recognized; every
/// other image, or one carrying no resolution, reports the default of 72 dpi on both axes.
pub(crate) fn image_dpi(bytes: &[u8]) -> (u32, u32) {
    png_dpi(bytes)
        .or_else(|| jpeg_dpi(bytes))
        .unwrap_or((DEFAULT_DPI, DEFAULT_DPI))
}

/// The resolution in a PNG's `pHYs` chunk, or `None` when the signature does not match, no such chunk
/// precedes the image data, or the chunk does not record its unit as the meter.
fn png_dpi(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..8) != Some(PNG_SIGNATURE) {
        return None;
    }
    // Chunks follow the eight-byte signature, each as a big-endian length, a four-byte type, that
    // many data bytes, and a four-byte checksum. The scan stops once the image data begins.
    let mut offset = 8usize;
    loop {
        let length = read_be_u32(bytes, offset)? as usize;
        let kind = bytes.get(offset + 4..offset + 8)?;
        if kind == b"IDAT" || kind == b"IEND" {
            return None;
        }
        if kind == b"pHYs" {
            let data = offset + 8;
            let per_meter_x = read_be_u32(bytes, data)?;
            let per_meter_y = read_be_u32(bytes, data + 4)?;
            // A unit of 1 marks the resolution as pixels per meter; any other unit leaves it
            // dimensionless, carrying no dpi.
            if *bytes.get(data + 8)? != 1 {
                return None;
            }
            return Some((
                dpi_from_per_meter(per_meter_x),
                dpi_from_per_meter(per_meter_y),
            ));
        }
        offset = offset.checked_add(12)?.checked_add(length)?;
    }
}

/// Converts a resolution in pixels per meter to whole dots per inch, one inch being 0.0254 meters.
/// A zero resolution reports the default rather than nothing.
#[allow(clippy::cast_possible_truncation)] // Any real resolution divided down stays inside u32.
fn dpi_from_per_meter(per_meter: u32) -> u32 {
    let dpi = (u64::from(per_meter) * 254 / 10_000) as u32;
    if dpi == 0 { DEFAULT_DPI } else { dpi }
}

/// The pixel density recorded in a JPEG's JFIF `APP0` segment, in dots per inch, or `None` when the
/// signature does not match, no JFIF segment precedes the image scan, or the segment records only an
/// aspect ratio. The density is stored per inch or per centimeter; the latter is scaled to inches.
fn jpeg_dpi(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..2) != Some([0xff, 0xd8].as_slice()) {
        return None;
    }
    let mut offset = 2usize;
    loop {
        let mut marker = *bytes.get(offset)?;
        // A marker is introduced by one or more 0xff fill bytes.
        while marker == 0xff {
            offset = offset.checked_add(1)?;
            marker = *bytes.get(offset)?;
        }
        offset = offset.checked_add(1)?;
        // Restart, start-of-image, end-of-image and temporary markers carry no length payload.
        if (0xd0..=0xd9).contains(&marker) || marker == 0x01 {
            continue;
        }
        let length = usize::from(read_be_u16(bytes, offset)?);
        // The density lives in a header segment; the scan gives up once the entropy-coded scan
        // begins, so a file without a JFIF segment reports no resolution.
        if marker == 0xda {
            return None;
        }
        let data = offset.checked_add(2)?;
        if marker == 0xe0 && bytes.get(data..data.checked_add(5)?) == Some(b"JFIF\0".as_slice()) {
            let units = *bytes.get(data.checked_add(7)?)?;
            let x_density = read_be_u16(bytes, data.checked_add(8)?)?;
            let y_density = read_be_u16(bytes, data.checked_add(10)?)?;
            return jfif_dpi(units, x_density, y_density);
        }
        offset = offset.checked_add(length)?;
    }
}

/// Resolves a JFIF density pair to dots per inch by its unit code: 1 is dots per inch and used as
/// stored, 2 is dots per centimeter and scaled up by 2.54, and any other code (including 0, which
/// marks the pair as a bare aspect ratio) carries no absolute resolution. A zero density likewise
/// yields nothing, so the caller falls back to the default.
fn jfif_dpi(units: u8, x_density: u16, y_density: u16) -> Option<(u32, u32)> {
    let to_dpi = |density: u16| -> Option<u32> {
        let value = match units {
            1 => u32::from(density),
            2 => (u32::from(density) * 254).div_ceil(100),
            _ => return None,
        };
        (value != 0).then_some(value)
    };
    Some((to_dpi(x_density)?, to_dpi(y_density)?))
}

/// The eight-byte signature every PNG begins with.
const PNG_SIGNATURE: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// The resolution assumed for an image that records none, in dots per inch.
const DEFAULT_DPI: u32 = 72;

/// The dimensions in a GIF's logical screen descriptor, or `None` when the signature does not match.
pub(crate) fn gif_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let header = bytes.get(..6)?;
    if header != b"GIF87a".as_slice() && header != b"GIF89a".as_slice() {
        return None;
    }
    Some((
        u32::from(read_le_u16(bytes, 6)?),
        u32::from(read_le_u16(bytes, 8)?),
    ))
}

/// The dimensions in a JPEG's first frame header, or `None` when the marker structure does not match.
pub(crate) fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..2) != Some([0xff, 0xd8].as_slice()) {
        return None;
    }
    let mut offset = 2usize;
    loop {
        let mut marker = *bytes.get(offset)?;
        // A marker is introduced by one or more 0xff fill bytes.
        while marker == 0xff {
            offset = offset.checked_add(1)?;
            marker = *bytes.get(offset)?;
        }
        offset = offset.checked_add(1)?;
        // Restart, start-of-image, end-of-image and temporary markers carry no length payload.
        if (0xd0..=0xd9).contains(&marker) || marker == 0x01 {
            continue;
        }
        let length = usize::from(read_be_u16(bytes, offset)?);
        // A start-of-frame marker holds the frame dimensions; the four Huffman/arithmetic table
        // markers in the same range do not.
        let is_frame = (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc);
        if is_frame {
            let height = read_be_u16(bytes, offset + 3)?;
            let width = read_be_u16(bytes, offset + 5)?;
            return Some((u32::from(width), u32::from(height)));
        }
        offset = offset.checked_add(length)?;
    }
}

/// A big-endian `u32` at `offset`, or `None` when the slice is too short.
fn read_be_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let array: [u8; 4] = bytes.get(offset..offset + 4)?.try_into().ok()?;
    Some(u32::from_be_bytes(array))
}

/// A big-endian `u16` at `offset`, or `None` when the slice is too short.
fn read_be_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let array: [u8; 2] = bytes.get(offset..offset + 2)?.try_into().ok()?;
    Some(u16::from_be_bytes(array))
}

/// A little-endian `u16` at `offset`, or `None` when the slice is too short.
fn read_le_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let array: [u8; 2] = bytes.get(offset..offset + 2)?.try_into().ok()?;
    Some(u16::from_le_bytes(array))
}

#[cfg(test)]
mod tests {
    use super::{gif_dimensions, image_dimensions, image_dpi, jpeg_dimensions, png_dimensions};

    #[test]
    fn png_dpi_reads_the_phys_chunk_in_meter_units() {
        // A pHYs chunk recording 3780 pixels per meter on both axes resolves to 96 dpi.
        let bytes = [
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, // signature
            0, 0, 0, 13, b'I', b'H', b'D', b'R', // IHDR chunk header
            0, 0, 0, 2, 0, 0, 0, 3, // width, height
            0, 0, 0, 0, 0, 0, 0, 0, 0, // remaining IHDR data + start of CRC
            0, 0, 0, 9, b'p', b'H', b'Y', b's', // pHYs chunk header
            0, 0, 0x0e, 0xc4, // 3780 pixels per unit, x
            0, 0, 0x0e, 0xc4, // 3780 pixels per unit, y
            1,    // unit: meter
            0, 0, 0, 0, // CRC
        ];
        assert_eq!(image_dpi(&bytes), (96, 96));
    }

    #[test]
    fn jpeg_dpi_reads_the_jfif_density_in_inches() {
        // A JFIF APP0 segment recording 300 dots per inch on both axes.
        let bytes = [
            0xff, 0xd8, // start of image
            0xff, 0xe0, 0x00, 0x10, // APP0 marker, length 16
            b'J', b'F', b'I', b'F', 0x00, // identifier
            0x01, 0x01, // version 1.1
            0x01, // units: dots per inch
            0x01, 0x2c, // x density 300
            0x01, 0x2c, // y density 300
            0x00, 0x00, // thumbnail 0x0
        ];
        assert_eq!(image_dpi(&bytes), (300, 300));
    }

    #[test]
    fn jpeg_dpi_scales_centimeter_density_to_inches() {
        // 118 dots per centimeter is 299.72 dpi, resolving to 300.
        let bytes = [
            0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, //
            b'J', b'F', b'I', b'F', 0x00, 0x01, 0x01, //
            0x02, // units: dots per centimeter
            0x00, 0x76, 0x00, 0x76, // x, y density 118
            0x00, 0x00,
        ];
        assert_eq!(image_dpi(&bytes), (300, 300));
    }

    #[test]
    fn jpeg_dpi_without_units_falls_back_to_default() {
        // Units 0 marks the density pair as a bare aspect ratio, carrying no absolute resolution.
        let bytes = [
            0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, //
            b'J', b'F', b'I', b'F', 0x00, 0x01, 0x01, //
            0x00, // units: none (aspect ratio only)
            0x00, 0x01, 0x00, 0x01, // x, y density 1
            0x00, 0x00,
        ];
        assert_eq!(image_dpi(&bytes), (72, 72));
    }

    #[test]
    fn image_dpi_defaults_to_seventy_two_without_a_resolution() {
        let bytes = [
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, // signature
            0, 0, 0, 13, b'I', b'H', b'D', b'R', // IHDR chunk header
            0, 0, 0, 2, 0, 0, 0, 3, // width, height
            0, 0, 0, 0, 0, 0, 0, 0, 0, // remaining IHDR data
            0, 0, 0, 0, b'I', b'D', b'A', b'T', // image data begins, no pHYs seen
        ];
        assert_eq!(image_dpi(&bytes), (72, 72));
        assert_eq!(image_dpi(b"not a png"), (72, 72));
    }

    #[test]
    fn png_dimensions_reads_the_ihdr_header() {
        let bytes = [
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, // signature
            0, 0, 0, 13, b'I', b'H', b'D', b'R', // IHDR chunk header
            0, 0, 0, 2, // width
            0, 0, 0, 3, // height
        ];
        assert_eq!(png_dimensions(&bytes), Some((2, 3)));
        assert_eq!(png_dimensions(b"not a png"), None);
    }

    #[test]
    fn gif_dimensions_reads_the_screen_descriptor() {
        let bytes = [b'G', b'I', b'F', b'8', b'9', b'a', 0x0a, 0x00, 0x14, 0x00];
        assert_eq!(gif_dimensions(&bytes), Some((10, 20)));
        assert_eq!(gif_dimensions(b"GIF"), None);
        assert_eq!(gif_dimensions(b"XXXXXX....."), None);
    }

    #[test]
    fn jpeg_dimensions_skips_to_the_start_of_frame() {
        // An APP0 segment (with a length payload) precedes the start-of-frame marker, which the scan
        // steps over before reading the frame's height and width.
        let bytes = [
            0xff, 0xd8, // start of image
            0xff, 0xe0, 0x00, 0x04, 0x00, 0x00, // APP0 segment, length 4
            0xff, 0xc0, 0x00, 0x11, 0x08, // start of frame, precision 8
            0x00, 0x1e, // height 30
            0x00, 0x28, // width 40
        ];
        assert_eq!(jpeg_dimensions(&bytes), Some((40, 30)));
        assert_eq!(jpeg_dimensions(b"not a jpeg"), None);
    }

    #[test]
    fn image_dimensions_is_zero_for_an_unrecognized_format() {
        assert_eq!(image_dimensions(b"neither png nor gif nor jpeg"), (0, 0));
    }
}
