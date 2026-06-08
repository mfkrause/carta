//! Named and numeric character-reference decoding shared by the text readers. The named-entity
//! table is generated at build time from the vendored character-reference data.

include!(concat!(env!("OUT_DIR"), "/entities_table.rs"));

/// Convert a numeric character reference's code point to a character: the null code point and any
/// value outside the Unicode scalar range both map to the replacement character.
pub(crate) fn code_point(code: u32) -> char {
    if code == 0 {
        return '\u{fffd}';
    }
    char::from_u32(code).unwrap_or('\u{fffd}')
}

/// Look up a named character reference (keyed without the leading `&` or trailing `;`), returning its
/// replacement text.
pub(crate) fn lookup_named(name: &str) -> Option<&'static str> {
    ENTITIES
        .binary_search_by(|(candidate, _)| (*candidate).cmp(name))
        .ok()
        .and_then(|index| ENTITIES.get(index))
        .map(|(_, characters)| *characters)
}
