//! Legacy pictorial-font glyph substitution for the docx reader.

use crate::xml::Element;

/// A legacy font whose printable-ASCII slots hold glyphs unrelated to the letters' code points, so a
/// run styled with it must have its text remapped to the Unicode characters those glyphs stand for.
#[derive(Debug, Clone, Copy)]
pub(super) enum SymbolFont {
    Symbol,
    Wingdings,
}

impl SymbolFont {
    /// The Unicode replacement for a single character, or `None` when the character is kept as-is:
    /// either it lies outside the printable-ASCII range the font remaps, or the font leaves that slot
    /// unassigned (an empty table entry), in which case the original character stands.
    fn map(self, ch: char) -> Option<&'static str> {
        let code = ch as u32;
        if !(0x20..=0x7E).contains(&code) {
            return None;
        }
        let index = (code - 0x20) as usize;
        let table = match self {
            SymbolFont::Symbol => &SYMBOL_TABLE,
            SymbolFont::Wingdings => &WINGDINGS_TABLE,
        };
        table.get(index).copied().filter(|slot| !slot.is_empty())
    }

    /// Remaps every character of a run's text to its Unicode equivalent.
    pub(super) fn substitute(self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for ch in text.chars() {
            match self.map(ch) {
                Some(replacement) => out.push_str(replacement),
                None => out.push(ch),
            }
        }
        out
    }
}

/// The legacy pictorial font a run's properties select through their `rFonts` ascii or high-ANSI
/// slot, if any. The complex-script slot is not consulted: it governs a separate script run.
pub(super) fn symbol_font(properties: &Element) -> Option<SymbolFont> {
    let fonts = properties.child("rFonts")?;
    for slot in ["ascii", "hAnsi"] {
        match fonts.attr(slot) {
            Some("Symbol") => return Some(SymbolFont::Symbol),
            Some("Wingdings") => return Some(SymbolFont::Wingdings),
            _ => {}
        }
    }
    None
}

/// Adobe Symbol's printable-ASCII slots (`0x20`–`0x7E`) mapped to the Unicode characters they render.
#[rustfmt::skip]
static SYMBOL_TABLE: [&str; 95] = [
    "\u{a0}", "!", "\u{2200}", "#", "\u{2203}", "%", "&", "\u{220b}", "(", ")", "\u{2217}", "+",
    ",", "\u{2212}", ".", "/", "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", ":", ";", "<",
    "=", ">", "?", "\u{2245}", "\u{391}", "\u{392}", "\u{3a7}", "\u{2206}", "\u{395}", "\u{3a6}",
    "\u{393}", "\u{397}", "\u{399}", "\u{3d1}", "\u{39a}", "\u{39b}", "\u{39c}", "\u{39d}",
    "\u{39f}", "\u{3a0}", "\u{398}", "\u{3a1}", "\u{3a3}", "\u{3a4}", "\u{3a5}", "\u{3c2}",
    "\u{2126}", "\u{39e}", "\u{3a8}", "\u{396}", "[", "\u{2234}", "]", "\u{22a5}", "_", "\u{f8e5}",
    "\u{3b1}", "\u{3b2}", "\u{3c7}", "\u{3b4}", "\u{3b5}", "\u{3c6}", "\u{3b3}", "\u{3b7}",
    "\u{3b9}", "\u{3d5}", "\u{3ba}", "\u{3bb}", "\u{3bc}", "\u{3bd}", "\u{3bf}", "\u{3c0}",
    "\u{3b8}", "\u{3c1}", "\u{3c3}", "\u{3c4}", "\u{3c5}", "\u{3d6}", "\u{3c9}", "\u{3be}",
    "\u{3c8}", "\u{3b6}", "{", "|", "}", "\u{223c}",
];

/// Wingdings' printable-ASCII slots (`0x20`–`0x7E`) mapped to the Unicode characters they render.
#[rustfmt::skip]
static WINGDINGS_TABLE: [&str; 95] = [
    "", "\u{1f589}", "\u{2702}", "\u{2701}", "\u{1f453}", "\u{1f56d}", "\u{1f56e}", "\u{1f56f}",
    "\u{1f57f}", "\u{2706}", "\u{1f582}", "\u{1f583}", "\u{1f4ea}", "\u{1f4eb}", "\u{1f4ec}",
    "\u{1f4ed}", "\u{1f4c1}", "\u{1f4c2}", "\u{1f4c4}", "\u{1f5cf}", "\u{1f5d0}", "\u{1f5c4}",
    "\u{231b}", "\u{1f5ae}", "\u{1f5b0}", "\u{1f5b2}", "\u{1f5b3}", "\u{1f5b4}", "\u{1f5ab}",
    "\u{1f5ac}", "\u{2707}", "\u{270d}", "\u{1f58e}", "\u{270c}", "\u{1f44c}", "\u{1f44d}",
    "\u{1f44e}", "\u{261c}", "\u{261e}", "\u{261d}", "\u{261f}", "\u{1f590}", "\u{263a}",
    "\u{1f610}", "\u{2639}", "\u{1f4a3}", "\u{2620}", "\u{1f3f3}", "\u{1f3f1}", "\u{2708}",
    "\u{263c}", "\u{1f4a7}", "\u{2744}", "\u{1f546}", "\u{271e}", "\u{1f548}", "\u{2720}",
    "\u{2721}", "\u{262a}", "\u{262f}", "\u{950}", "\u{2638}", "\u{2648}", "\u{2649}", "\u{264a}",
    "\u{264b}", "\u{264c}", "\u{264d}", "\u{264e}", "\u{264f}", "\u{2650}", "\u{2651}", "\u{2652}",
    "\u{2653}", "\u{1f670}", "\u{1f675}", "\u{25cf}", "\u{1f53e}", "\u{25a0}", "\u{25a1}",
    "\u{1f790}", "\u{2751}", "\u{2752}", "\u{2b27}", "\u{29eb}", "\u{25c6}", "\u{2756}",
    "\u{2b25}", "\u{2327}", "\u{2bb9}", "\u{2318}", "\u{1f3f5}", "\u{1f3f6}", "\u{1f676}",
    "\u{1f677}",
];
