//! TIFF image dimensions and resolution tags.

pub(super) struct Tiff<'a> {
    bytes: &'a [u8],
    entries: &'a [u8],
    little_endian: bool,
}

impl<'a> Tiff<'a> {
    pub(super) fn new(bytes: &'a [u8]) -> Option<Self> {
        let little_endian = match bytes.get(..4)? {
            b"II\x2a\0" => true,
            b"MM\0\x2a" => false,
            _ => return None,
        };
        let directory = usize::try_from(word(bytes.get(4..)?, little_endian)?).ok()?;
        let count = usize::from(short(bytes.get(directory..)?, little_endian)?);
        let start = directory.checked_add(2)?;
        let end = start.checked_add(count * 12)?;
        bytes.get(end..end.checked_add(4)?)?;
        Some(Self {
            bytes,
            entries: bytes.get(start..end)?,
            little_endian,
        })
    }

    pub(super) fn dimensions(&self) -> Option<(u32, u32)> {
        if !matches!(self.integer(259)?, 0 | 1 | 2 | 5 | 6 | 32773) {
            return None;
        }
        for tag in [258, 259, 262, 273, 277, 278, 279] {
            self.field(tag)?;
        }
        Some((self.integer(256)?, self.integer(257)?))
    }

    pub(super) fn density(&self) -> (f64, f64) {
        let factor = match self.integer(296) {
            Some(2) => 1.0,
            Some(3) => 2.54,
            _ => return (72.0, 72.0),
        };
        let resolution = |tag| {
            let value = (self.rational(tag)?.floor() * factor).floor();
            (value > 0.0).then_some(value)
        };
        (
            resolution(282).unwrap_or(72.0),
            resolution(283).unwrap_or(72.0),
        )
    }

    fn field(&self, tag: u16) -> Option<(u16, &'a [u8])> {
        let entry = self
            .entries
            .chunks_exact(12)
            .find(|entry| short(entry, self.little_endian) == Some(tag))?;
        let kind = short(entry.get(2..)?, self.little_endian)?;
        let count = usize::try_from(word(entry.get(4..)?, self.little_endian)?).ok()?;
        let size = match kind {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 => 4,
            5 | 10 | 12 => 8,
            _ => return None,
        };
        if count == 0 {
            return None;
        }
        let length = count.checked_mul(size)?;
        let data = if length <= 4 {
            entry.get(8..)?
        } else {
            let offset = usize::try_from(word(entry.get(8..)?, self.little_endian)?).ok()?;
            self.bytes.get(offset..)?
        };
        Some((kind, data.get(..length)?))
    }

    fn integer(&self, tag: u16) -> Option<u32> {
        let (kind, data) = self.field(tag)?;
        match kind {
            3 => short(data, self.little_endian).map(u32::from),
            4 => word(data, self.little_endian),
            _ => None,
        }
    }

    fn rational(&self, tag: u16) -> Option<f64> {
        let (5, data) = self.field(tag)? else {
            return None;
        };
        let numerator = word(data, self.little_endian)?;
        let denominator = word(data.get(4..)?, self.little_endian)?;
        (denominator != 0).then(|| f64::from(numerator) / f64::from(denominator))
    }
}

fn short(bytes: &[u8], little_endian: bool) -> Option<u16> {
    let value = bytes.get(..2)?.try_into().ok()?;
    Some(if little_endian {
        u16::from_le_bytes(value)
    } else {
        u16::from_be_bytes(value)
    })
}

fn word(bytes: &[u8], little_endian: bool) -> Option<u32> {
    let value = bytes.get(..4)?.try_into().ok()?;
    Some(if little_endian {
        u32::from_le_bytes(value)
    } else {
        u32::from_be_bytes(value)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(little_endian: bool, unit: u32) -> Vec<u8> {
        let short = |value: u16| {
            if little_endian {
                value.to_le_bytes()
            } else {
                value.to_be_bytes()
            }
        };
        let word = |value: u32| {
            if little_endian {
                value.to_le_bytes()
            } else {
                value.to_be_bytes()
            }
        };
        let tags = [
            (256, 4, 32),
            (257, 4, 16),
            (258, 3, 8),
            (259, 3, 1),
            (262, 3, 1),
            (273, 4, 174),
            (277, 3, 1),
            (278, 4, 16),
            (279, 4, 512),
            (282, 5, 158),
            (283, 5, 166),
            (296, 3, unit),
        ];
        let mut bytes = if little_endian {
            b"II".to_vec()
        } else {
            b"MM".to_vec()
        };
        bytes.extend(short(42));
        bytes.extend(word(8));
        bytes.extend(short(12));
        for (tag, kind, value) in tags {
            bytes.extend(short(tag));
            bytes.extend(short(kind));
            bytes.extend(word(1));
            if kind == 3 {
                bytes.extend(short(u16::try_from(value).unwrap()));
                bytes.extend([0, 0]);
            } else {
                bytes.extend(word(value));
            }
        }
        bytes.extend(word(0));
        for value in [299, 2, 59, 1] {
            bytes.extend(word(value));
        }
        bytes.resize(686, 0);
        bytes
    }

    #[test]
    fn dimensions_and_density_support_both_byte_orders_and_units() {
        for little_endian in [false, true] {
            for (unit, density) in [(1, (72.0, 72.0)), (2, (149.0, 59.0)), (3, (378.0, 149.0))] {
                let bytes = image(little_endian, unit);
                let metadata = Tiff::new(&bytes).unwrap();
                assert_eq!(metadata.dimensions(), Some((32, 16)));
                assert_eq!(metadata.density(), density);
                assert_eq!(
                    super::super::media::image_metrics(&bytes),
                    ((32, 16), density)
                );
            }
        }
    }

    #[test]
    fn truncated_directories_and_invalid_offsets_are_rejected() {
        let mut bytes = image(true, 2);
        for length in 0..158 {
            assert!(Tiff::new(&bytes[..length]).is_none(), "{length}");
        }
        bytes[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Tiff::new(&bytes).is_none());
    }

    #[test]
    fn unsupported_compression_does_not_supply_dimensions() {
        let mut bytes = image(true, 2);
        bytes[54..56].copy_from_slice(&8u16.to_le_bytes());
        assert_eq!(Tiff::new(&bytes).unwrap().dimensions(), None);
    }
}
