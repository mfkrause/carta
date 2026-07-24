//! Lookup tables for TeX math conversion: symbols, Greek letters, named functions, accents,
//! spacing, and the styled-alphabet codepoint mapping.
//!
//! Every entry is keyed by the command name *without* its leading backslash. The unicode targets
//! are taken from the Unicode standard's mathematical character assignments.

/// The math class of a token, which governs inter-token spacing in inline output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Class {
    /// An ordinary atom (letters, digits, most symbols): no surrounding space.
    Ord,
    /// A binary operator (`+`, `\times`, …): a four-per-em space on each side.
    Bin,
    /// A relation (`=`, `\leq`, arrows, …): a three-per-em space on each side.
    Rel,
    /// An opening delimiter: no space.
    Open,
    /// A closing delimiter: no space.
    Close,
    /// Punctuation (`,`, `;`): a six-per-em space after.
    Punct,
    /// A large operator (`\sum`, `\int`, `\bigcup`, …): no surrounding space of its own, and it
    /// suppresses the space a binary operator or punctuation atom immediately after it would carry.
    Op,
}

/// A symbol's unicode rendering plus whether it renders as an italic variable.
pub(super) struct Symbol {
    pub text: &'static str,
    pub class: Class,
    /// True when the symbol is a variable-like letterlike glyph rendered in italics.
    pub italic: bool,
}

pub(super) const fn ord(text: &'static str) -> Symbol {
    Symbol {
        text,
        class: Class::Ord,
        italic: false,
    }
}
pub(super) const fn bin(text: &'static str) -> Symbol {
    Symbol {
        text,
        class: Class::Bin,
        italic: false,
    }
}
pub(super) const fn rel(text: &'static str) -> Symbol {
    Symbol {
        text,
        class: Class::Rel,
        italic: false,
    }
}
pub(super) const fn open(text: &'static str) -> Symbol {
    Symbol {
        text,
        class: Class::Open,
        italic: false,
    }
}
pub(super) const fn close(text: &'static str) -> Symbol {
    Symbol {
        text,
        class: Class::Close,
        italic: false,
    }
}
pub(super) const fn punct(text: &'static str) -> Symbol {
    Symbol {
        text,
        class: Class::Punct,
        italic: false,
    }
}
pub(super) const fn op(text: &'static str) -> Symbol {
    Symbol {
        text,
        class: Class::Op,
        italic: false,
    }
}
pub(super) const fn var(text: &'static str) -> Symbol {
    Symbol {
        text,
        class: Class::Ord,
        italic: true,
    }
}

mod table;

pub(crate) use table::symbol;

/// Greek letters: the command name maps to a unicode letter. The boolean is whether the letter is
/// set as an italic variable (true) or upright (false).
#[allow(clippy::match_same_arms)]
pub(super) fn greek(name: &str) -> Option<(&'static str, bool)> {
    let g = match name {
        "alpha" => ("\u{03B1}", true),
        "beta" => ("\u{03B2}", true),
        "gamma" => ("\u{03B3}", true),
        "delta" => ("\u{03B4}", true),
        "epsilon" => ("\u{03F5}", true),
        "varepsilon" => ("\u{03B5}", true),
        "zeta" => ("\u{03B6}", true),
        "eta" => ("\u{03B7}", true),
        "theta" => ("\u{03B8}", true),
        "vartheta" => ("\u{03D1}", true),
        "iota" => ("\u{03B9}", true),
        "kappa" => ("\u{03BA}", true),
        "lambda" => ("\u{03BB}", true),
        "mu" => ("\u{03BC}", true),
        "nu" => ("\u{03BD}", true),
        "xi" => ("\u{03BE}", true),
        "omicron" => ("\u{03BF}", true),
        "pi" => ("\u{03C0}", true),
        "varpi" => ("\u{03D6}", true),
        "rho" => ("\u{03C1}", true),
        // Variant rho/sigma have no plain Greek codepoint; they use mathematical-italic letters.
        "varrho" => ("\u{1D71A}", true),
        "sigma" => ("\u{03C3}", true),
        "varsigma" => ("\u{1D70D}", true),
        "tau" => ("\u{03C4}", true),
        "upsilon" => ("\u{03C5}", true),
        "phi" => ("\u{03D5}", true),
        "varphi" => ("\u{03C6}", true),
        "chi" => ("\u{03C7}", true),
        "psi" => ("\u{03C8}", true),
        "omega" => ("\u{03C9}", true),
        "Gamma" => ("\u{0393}", true),
        "Delta" => ("\u{0394}", true),
        "Theta" => ("\u{0398}", true),
        "Lambda" => ("\u{039B}", true),
        "Xi" => ("\u{039E}", true),
        "Pi" => ("\u{03A0}", true),
        "Sigma" => ("\u{03A3}", true),
        "Upsilon" => ("\u{03A5}", true),
        "Phi" => ("\u{03A6}", true),
        "Psi" => ("\u{03A8}", true),
        "Omega" => ("\u{03A9}", true),
        // Capital Greek letters whose glyph coincides with a Latin capital.
        "Alpha" => ("\u{0391}", true),
        "Beta" => ("\u{0392}", true),
        "Epsilon" => ("\u{0395}", true),
        "Zeta" => ("\u{0396}", true),
        "Eta" => ("\u{0397}", true),
        "Iota" => ("\u{0399}", true),
        "Kappa" => ("\u{039A}", true),
        "Mu" => ("\u{039C}", true),
        "Nu" => ("\u{039D}", true),
        "Omicron" => ("\u{039F}", true),
        "Rho" => ("\u{03A1}", true),
        "Tau" => ("\u{03A4}", true),
        "Chi" => ("\u{03A7}", true),
        // The upright-Greek family renders to the same letters as the plain Greek commands.
        "upalpha" => ("\u{03B1}", true),
        "upbeta" => ("\u{03B2}", true),
        "upgamma" => ("\u{03B3}", true),
        "updelta" => ("\u{03B4}", true),
        "upepsilon" => ("\u{03B5}", true),
        "upzeta" => ("\u{03B6}", true),
        "upeta" => ("\u{03B7}", true),
        "uptheta" => ("\u{03B8}", true),
        "upiota" => ("\u{03B9}", true),
        "upkappa" => ("\u{03BA}", true),
        "uplambda" => ("\u{03BB}", true),
        "upmu" => ("\u{03BC}", true),
        "upnu" => ("\u{03BD}", true),
        "upxi" => ("\u{03BE}", true),
        "upomicron" => ("\u{03BF}", true),
        "uppi" => ("\u{03C0}", true),
        "uprho" => ("\u{03C1}", true),
        "upsigma" => ("\u{03C3}", true),
        "uptau" => ("\u{03C4}", true),
        "upupsilon" => ("\u{03C5}", true),
        "upphi" => ("\u{03D5}", true),
        "upchi" => ("\u{03C7}", true),
        "uppsi" => ("\u{03C8}", true),
        "upomega" => ("\u{03C9}", true),
        _ => return None,
    };
    Some(g)
}

/// Named operators rendered upright (e.g. `\sin`, `\log`). The boolean is whether the operator
/// takes stacked limits: such operators with *both* a subscript and a superscript cannot be
/// linearised and force a verbatim fallback.
pub(super) fn named_function(name: &str) -> Option<(&'static str, bool)> {
    let f = match name {
        "sin" => ("sin", false),
        "cos" => ("cos", false),
        "tan" => ("tan", false),
        "cot" => ("cot", false),
        "sec" => ("sec", false),
        "csc" => ("csc", false),
        "arcsin" => ("arcsin", false),
        "arccos" => ("arccos", false),
        "arctan" => ("arctan", false),
        "sinh" => ("sinh", false),
        "cosh" => ("cosh", false),
        "tanh" => ("tanh", false),
        "coth" => ("coth", false),
        "log" => ("log", false),
        "ln" => ("ln", false),
        "lg" => ("lg", false),
        "exp" => ("exp", false),
        "deg" => ("deg", false),
        "dim" => ("dim", false),
        "hom" => ("hom", false),
        "ker" => ("ker", false),
        "arg" => ("arg", false),
        // Limit-class operators: stacked sub+sup forces fallback.
        "lim" => ("lim", true),
        "limsup" => ("limsup", true),
        "liminf" => ("liminf", true),
        "max" => ("max", true),
        "min" => ("min", true),
        "sup" => ("sup", true),
        "inf" => ("inf", true),
        "det" => ("det", true),
        "gcd" => ("gcd", true),
        "Pr" => ("Pr", true),
        _ => return None,
    };
    Some(f)
}

/// Big-operator symbols (`\sum`, `\prod`, …) that carry stacked limits: with *both* a subscript
/// and a superscript present, the expression cannot be linearised. `\int`/`\oint` set their
/// scripts to the side and are excluded.
pub(super) fn is_limit_operator(name: &str) -> bool {
    matches!(
        name,
        "sum"
            | "prod"
            | "coprod"
            | "bigcap"
            | "bigcup"
            | "bigsqcup"
            | "bigsqcap"
            | "bigvee"
            | "bigwedge"
    )
}

/// Whether a raw large-operator glyph written directly in the source carries stacked limits, the way
/// its `\command` spelling does. A direct glyph with *both* a sub- and a superscript then cannot be
/// linearised. The glyphs are the n-ary sum, product, and coproduct and
/// the big intersection, union, square-cup/cap, and logical or/and. The side-script big operators
/// (`∫ ∮ ⊕ ⨁`) are excluded.
pub(super) fn is_limit_glyph(c: char) -> bool {
    matches!(
        c,
        '\u{2211}' // ∑ n-ary summation
            | '\u{220F}' // ∏ n-ary product
            | '\u{2210}' // ∐ n-ary coproduct
            | '\u{22C2}' // ⋂ n-ary intersection
            | '\u{22C3}' // ⋃ n-ary union
            | '\u{22C0}' // ⋀ n-ary logical and
            | '\u{22C1}' // ⋁ n-ary logical or
            | '\u{2A06}' // ⨆ n-ary square union
            | '\u{2A05}' // ⨅ n-ary square intersection
    )
}

/// The precomposed glyph for a `\not`-negated relation, when a single negated codepoint exists. The
/// key is the base relation (a command name without backslash, or a single relation character like
/// `=`). A relation with no precomposed negation returns `None`; the caller then overlays a combining
/// long solidus (U+0338) on the base glyph.
pub(super) fn negated_relation(base: &str) -> Option<&'static str> {
    let glyph = match base {
        "=" => "\u{2260}",
        "<" => "\u{226E}",
        ">" => "\u{226F}",
        "equiv" => "\u{2262}",
        "in" => "\u{2209}",
        "ni" | "owns" => "\u{220C}",
        "subset" => "\u{2284}",
        "supset" => "\u{2285}",
        "subseteq" => "\u{2288}",
        "supseteq" => "\u{2289}",
        "simeq" => "\u{2244}",
        "approx" => "\u{2249}",
        "cong" => "\u{2246}",
        "le" | "leq" | "leqslant" => "\u{2270}",
        "ge" | "geq" | "geqslant" => "\u{2271}",
        "prec" => "\u{2280}",
        "succ" => "\u{2281}",
        "preceq" | "preccurlyeq" => "\u{22E0}",
        "succeq" | "succcurlyeq" => "\u{22E1}",
        "lesssim" => "\u{2274}",
        "gtrsim" => "\u{2275}",
        "parallel" => "\u{2226}",
        "sqsubseteq" => "\u{22E2}",
        "sqsupseteq" => "\u{22E3}",
        _ => return None,
    };
    Some(glyph)
}

/// Whether a `\not`-negated base has no struck-through form at all, so the whole `\not<base>`
/// expression is left verbatim rather than composed. These bases (set operators, the existential
/// quantifier, the divides bar, and the triangle relations) carry neither a precomposed negated
/// glyph nor a meaningful combining-solidus overlay, so striking them is suppressed.
pub(super) fn is_unnegatable(base: &str) -> bool {
    matches!(
        base,
        "mid" | "exists" | "cup" | "cap" | "lhd" | "rhd" | "triangleleft" | "triangleright"
    )
}

/// Whether a `\not` written over a command base (`\not\in`, `\not\sim`, `\not\alpha`, `\not\|`, …)
/// composes into a struck-through form. A command base negates only when it is a relation, struck
/// with its precomposed glyph (`\in` → ∉) or, lacking one, a combining solidus (`\vdash` → ⊬), or an
/// italic letterlike whose glyph takes the solidus (a Greek letter, or a slanted letterlike such as
/// `\ell`/`\imath`/`\aleph`). Every other command has no struck form and is left verbatim: an upright
/// letterlike (`\hbar`, `\Re`, `\nabla`), a delimiter (`\lvert`, `\langle`), the ordinary bar
/// commands (`\|`, `\Vert`), or a binary operator (`\setminus`, `\cup`, `\mid`). A literal character
/// base is not routed here: `\not|`/`\not(`/`\not=` always strike directly.
pub(super) fn command_negatable(name: &str) -> bool {
    if is_unnegatable(name) {
        return false;
    }
    if greek(name).is_some() {
        return true;
    }
    symbol(name).is_some_and(|sym| sym.class == Class::Rel || sym.italic)
}

/// The Typst markup for a `\not`-negated relation that has a dedicated negated token. A relation
/// without one returns `None`, and the caller overlays a combining long solidus on the base token.
pub(super) fn negated_relation_typst(base: &str) -> Option<&'static str> {
    let token = match base {
        "=" => "eq.not",
        "<" => "lt.not",
        ">" => "gt.not",
        "equiv" => "equiv.not",
        "in" => "in.not",
        "ni" | "owns" => "in.rev.not",
        "subset" => "subset.not",
        "supset" => "supset.not",
        "subseteq" => "subset.eq.not",
        "supseteq" => "supset.eq.not",
        "simeq" => "tilde.eq.not",
        "approx" => "approx.not",
        "cong" => "tilde.nequiv",
        "le" | "leq" | "leqslant" => "lt.eq.not",
        "ge" | "geq" | "geqslant" => "gt.eq.not",
        "prec" => "prec.not",
        "succ" => "succ.not",
        "preceq" | "preccurlyeq" => "prec.curly.eq.not",
        "succeq" | "succcurlyeq" => "succ.curly.eq.not",
        "lesssim" => "lt.tilde.not",
        "gtrsim" => "gt.tilde.not",
        "parallel" => "parallel.not",
        "sqsubseteq" => "subset.eq.sq.not",
        "sqsupseteq" => "supset.eq.sq.not",
        _ => return None,
    };
    Some(token)
}

/// Accent commands over a single base: the unicode combining mark appended to the base glyph.
pub(super) fn accent(name: &str) -> Option<char> {
    let mark = match name {
        "bar" => '\u{0304}',
        "hat" | "widehat" => '\u{0302}',
        "tilde" | "widetilde" => '\u{0303}',
        "vec" | "overrightarrow" => '\u{20D7}',
        "overleftarrow" => '\u{20D6}',
        "dot" => '\u{0307}',
        "check" => '\u{030C}',
        "breve" => '\u{0306}',
        "acute" => '\u{0301}',
        "grave" => '\u{0300}',
        "mathring" => '\u{030A}',
        _ => return None,
    };
    Some(mark)
}

/// Spacing commands: the literal text the spacing renders to inline.
pub(super) fn spacing(name: &str) -> Option<&'static str> {
    let s = match name {
        "," => "\u{2006}",             // thin space (six-per-em)
        ";" => "\u{2005}",             // four-per-em space
        ":" | ">" | " " => "\u{00A0}", // medium / non-breaking space
        "!" => "\u{200A}",             // negative thin space renders as a hair space
        "medspace" => "\u{205F}",      // medium math space
        "enspace" => "\u{2000}",       // en quad
        "quad" => "\u{200A}\u{2001}",  // hair + em quad
        "qquad" => "\u{200A}\u{2001}\u{2001}",
        _ => return None,
    };
    Some(s)
}

/// The leading codepoint of each styled alphabet block in the Unicode Mathematical Alphanumeric
/// Symbols range, computed for an uppercase `A`. A few code points in these blocks are "holes"
/// reserved for letterlike characters that live elsewhere; [`styled_letter`] patches those.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Alphabet {
    DoubleStruck,
    Script,
    Fraktur,
    /// The bold script block: a contiguous range with no letterlike holes.
    BoldScript,
    /// The bold fraktur block: a contiguous range with no letterlike holes.
    BoldFraktur,
}

/// Map a single ASCII letter or digit to its styled-alphabet codepoint, or `None` if the input is
/// not a single styleable character. Patches the Unicode holes where the styled glyph is a named
/// letterlike character.
pub(super) fn styled_letter(alphabet: Alphabet, ch: char) -> Option<String> {
    if ch.is_ascii_digit() {
        return styled_digit(alphabet, ch);
    }
    if !ch.is_ascii_alphabetic() {
        return None;
    }
    if let Some(patched) = hole(alphabet, ch) {
        return Some(patched.to_string());
    }
    let (upper_base, lower_base) = match alphabet {
        Alphabet::DoubleStruck => (0x1D538, 0x1D552),
        Alphabet::Script => (0x1D49C, 0x1D4B6),
        Alphabet::Fraktur => (0x1D504, 0x1D51E),
        Alphabet::BoldScript => (0x1D4D0, 0x1D4EA),
        Alphabet::BoldFraktur => (0x1D56C, 0x1D586),
    };
    let base = if ch.is_ascii_uppercase() {
        upper_base
    } else {
        lower_base
    };
    let offset = if ch.is_ascii_uppercase() {
        (ch as u32) - ('A' as u32)
    } else {
        (ch as u32) - ('a' as u32)
    };
    char::from_u32(base + offset).map(|c| c.to_string())
}

fn styled_digit(alphabet: Alphabet, ch: char) -> Option<String> {
    // Only double-struck has dedicated digit glyphs; script/fraktur digits stay plain ASCII.
    let base = match alphabet {
        Alphabet::DoubleStruck => 0x1D7D8,
        Alphabet::Script | Alphabet::Fraktur | Alphabet::BoldScript | Alphabet::BoldFraktur => {
            return Some(ch.to_string());
        }
    };
    let offset = (ch as u32) - ('0' as u32);
    char::from_u32(base + offset).map(|c| c.to_string())
}

/// The double-struck form of a letterlike glyph that lives outside the styled-alphabet block. The
/// double-struck alphabet covers the Latin letters and digits; a few other glyphs have dedicated
/// double-struck codepoints in the Letterlike Symbols block: the lower and upper double-struck gamma
/// and pi, and the double-struck n-ary summation. The key is the glyph itself, so both the command
/// form (`\gamma`, `\sum`) and a directly-typed glyph map alike.
pub(super) fn double_struck_special(c: char) -> Option<char> {
    let mapped = match c {
        '\u{03B3}' => '\u{213D}', // γ → ℽ
        '\u{0393}' => '\u{213E}', // Γ → ℾ
        '\u{03C0}' => '\u{213C}', // π → ℼ
        '\u{03A0}' => '\u{213F}', // Π → ℿ
        '\u{2211}' => '\u{2140}', // ∑ → ⅀
        _ => return None,
    };
    Some(mapped)
}

/// The letterlike "holes" in the styled blocks, where the glyph lives in the BMP instead.
fn hole(alphabet: Alphabet, ch: char) -> Option<char> {
    let c = match (alphabet, ch) {
        (Alphabet::DoubleStruck, 'C') => '\u{2102}',
        (Alphabet::DoubleStruck, 'H') => '\u{210D}',
        (Alphabet::DoubleStruck, 'N') => '\u{2115}',
        (Alphabet::DoubleStruck, 'P') => '\u{2119}',
        (Alphabet::DoubleStruck, 'Q') => '\u{211A}',
        (Alphabet::DoubleStruck, 'R') => '\u{211D}',
        (Alphabet::DoubleStruck, 'Z') => '\u{2124}',
        (Alphabet::Script, 'B') => '\u{212C}',
        (Alphabet::Script, 'E') => '\u{2130}',
        (Alphabet::Script, 'F') => '\u{2131}',
        (Alphabet::Script, 'H') => '\u{210B}',
        (Alphabet::Script, 'I') => '\u{2110}',
        (Alphabet::Script, 'L') => '\u{2112}',
        (Alphabet::Script, 'M') => '\u{2133}',
        (Alphabet::Script, 'R') => '\u{211B}',
        (Alphabet::Script, 'e') => '\u{212F}',
        (Alphabet::Script, 'g') => '\u{210A}',
        (Alphabet::Script, 'o') => '\u{2134}',
        (Alphabet::Fraktur, 'C') => '\u{212D}',
        (Alphabet::Fraktur, 'H') => '\u{210C}',
        (Alphabet::Fraktur, 'I') => '\u{2111}',
        (Alphabet::Fraktur, 'R') => '\u{211C}',
        (Alphabet::Fraktur, 'Z') => '\u{2128}',
        _ => return None,
    };
    Some(c)
}
