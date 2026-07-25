//! The set of bytes a pattern can begin a match with, read off the pattern text.
//!
//! Trying a regular-expression rule means entering the regex engine, and compiling the pattern the
//! first time. Most rules cannot match at most positions, so [`FirstBytes::of_pattern`] derives a
//! superset of the leading bytes a match could start with and the tokenizer rejects a hopeless rule
//! with one bit test instead. The analysis is deliberately partial: anything it does not model
//! yields [`FirstBytes::any`], which admits every byte and so only costs the test.
//!
//! Soundness rests on never *excluding* a byte the pattern could actually start with. Every
//! construct is therefore either modeled exactly or widened to "any"; a widened answer is slower,
//! never wrong.

/// A set of admissible leading bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FirstBytes {
    mask: [u64; 4],
}

impl FirstBytes {
    /// The empty set: no byte can start a match.
    const fn none() -> Self {
        FirstBytes { mask: [0; 4] }
    }

    /// The set admitting every byte, used wherever the analysis declines to model a construct.
    pub(crate) const fn any() -> Self {
        FirstBytes {
            mask: [u64::MAX; 4],
        }
    }

    fn is_any(self) -> bool {
        self.mask == [u64::MAX; 4]
    }

    fn insert(&mut self, byte: u8) {
        let index = usize::from(byte);
        if let Some(word) = self.mask.get_mut(index >> 6) {
            *word |= 1u64 << (index & 63);
        }
    }

    fn insert_range(&mut self, from: u8, to: u8) {
        for byte in from..=to {
            self.insert(byte);
        }
    }

    /// Add every byte that can lead a non-ASCII UTF-8 sequence.
    fn insert_non_ascii(&mut self) {
        self.insert_range(0x80, 0xFF);
    }

    /// Add the leading byte of `ch`. Case-insensitive matching folds by Unicode simple case folding,
    /// which can pair an ASCII letter with a multi-byte character (`k` with U+212A), so an
    /// insensitive match admits every non-ASCII lead rather than enumerating the pairings.
    fn insert_char(&mut self, ch: char, insensitive: bool) {
        let mut buffer = [0u8; 4];
        if let Some(&byte) = ch.encode_utf8(&mut buffer).as_bytes().first() {
            self.insert(byte);
        }
        if insensitive {
            if ch.is_ascii() {
                self.insert(ch.to_ascii_lowercase() as u8);
                self.insert(ch.to_ascii_uppercase() as u8);
            }
            self.insert_non_ascii();
        }
    }

    fn union(self, other: Self) -> Self {
        let mut mask = self.mask;
        for (word, extra) in mask.iter_mut().zip(other.mask) {
            *word |= extra;
        }
        FirstBytes { mask }
    }

    fn complement(self) -> Self {
        let mut mask = self.mask;
        for word in &mut mask {
            *word = !*word;
        }
        FirstBytes { mask }
    }

    /// Whether a match could begin with `byte`.
    pub(crate) fn admits(self, byte: u8) -> bool {
        let index = usize::from(byte);
        self.mask
            .get(index >> 6)
            .is_some_and(|word| word & (1u64 << (index & 63)) != 0)
    }

    /// The leading bytes a match of `pattern` can start with, or [`FirstBytes::any`] when the
    /// pattern uses a construct the analysis does not model.
    pub(crate) fn of_pattern(pattern: &str, insensitive: bool) -> Self {
        let mut parser = Parser {
            chars: pattern.as_bytes(),
            at: 0,
            insensitive,
        };
        let set = parser.alternation(0);
        match set {
            // A pattern the parser could not consume fully is not understood; admit everything.
            Some(set) if parser.at == parser.chars.len() => set,
            _ => FirstBytes::any(),
        }
    }
}

/// A recursive-descent scan over the pattern text, tracking only what the first-byte set needs.
struct Parser<'a> {
    chars: &'a [u8],
    at: usize,
    insensitive: bool,
}

/// What one pattern element contributes: the bytes it can start with, and whether it can also match
/// nothing (so the element after it can supply the first byte too).
struct Element {
    first: FirstBytes,
    optional: bool,
}

/// Nesting cap, so a pathological pattern cannot recurse without bound.
const MAX_DEPTH: usize = 24;

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.chars.get(self.at).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.at += 1;
        Some(byte)
    }

    fn eat(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.at += 1;
            return true;
        }
        false
    }

    /// A run of branches separated by `|`; the first-byte set is their union.
    fn alternation(&mut self, depth: usize) -> Option<FirstBytes> {
        if depth > MAX_DEPTH {
            return None;
        }
        let mut set = self.sequence(depth)?;
        while self.eat(b'|') {
            set = set.union(self.sequence(depth)?);
        }
        Some(set)
    }

    /// A run of elements. The result unions each leading element's set until one cannot match empty;
    /// a sequence whose every element is optional can itself match empty, which the caller widens.
    fn sequence(&mut self, depth: usize) -> Option<FirstBytes> {
        let mut set = FirstBytes::none();
        let mut settled = false;
        loop {
            match self.peek() {
                None | Some(b'|' | b')') => break,
                _ => {}
            }
            let element = self.element(depth)?;
            if settled {
                continue;
            }
            set = set.union(element.first);
            if !element.optional {
                settled = true;
            }
        }
        // An all-optional sequence can match empty, so nothing constrains the first byte.
        if settled {
            Some(set)
        } else {
            Some(FirstBytes::any())
        }
    }

    /// One element with its quantifier applied.
    fn element(&mut self, depth: usize) -> Option<Element> {
        let mut element = self.atom(depth)?;
        // `*`, `?` and `{0,…}` let the atom match nothing; `+` and `{n>0,…}` do not.
        match self.peek() {
            Some(b'*' | b'?') => {
                self.at += 1;
                element.optional = true;
            }
            Some(b'+') => self.at += 1,
            Some(b'{') => {
                let repeat = self.repeat_bounds()?;
                element.optional |= repeat;
            }
            _ => {}
        }
        // A lazy or possessive suffix does not change what can start a match.
        if matches!(self.peek(), Some(b'?' | b'+')) {
            self.at += 1;
        }
        Some(element)
    }

    /// Consume a `{n}` / `{n,}` / `{n,m}` bound, reporting whether it admits zero repetitions.
    fn repeat_bounds(&mut self) -> Option<bool> {
        let open = self.at;
        self.at += 1;
        let start = self.at;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.at += 1;
        }
        let low = self.chars.get(start..self.at)?;
        if self.eat(b',') {
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.at += 1;
            }
        }
        if !self.eat(b'}') {
            // Not a bound after all (a literal brace); reparse it as a literal.
            self.at = open;
            return None;
        }
        // An absent or zero lower bound means the atom may be skipped entirely.
        Some(low.is_empty() || low.iter().all(|digit| *digit == b'0'))
    }

    fn atom(&mut self, depth: usize) -> Option<Element> {
        match self.peek()? {
            b'(' => self.group(depth),
            b'[' => Some(Element {
                first: self.class()?,
                optional: false,
            }),
            b'.' => {
                self.at += 1;
                Some(Element {
                    first: FirstBytes::any(),
                    optional: false,
                })
            }
            // Anchors and word boundaries consume no input.
            b'^' | b'$' => {
                self.at += 1;
                Some(Element {
                    first: FirstBytes::none(),
                    optional: true,
                })
            }
            b'\\' => self.escape(),
            // A bare quantifier or closing brace here means the pattern is not shaped as expected.
            b'*' | b'+' | b'?' => None,
            _ => {
                let ch = self.literal_char()?;
                let mut first = FirstBytes::none();
                first.insert_char(ch, self.insensitive);
                Some(Element {
                    first,
                    optional: false,
                })
            }
        }
    }

    /// A parenthesised group: a capture, a non-capturing group, an inline flag setting, or a
    /// look-around (which consumes no input).
    fn group(&mut self, depth: usize) -> Option<Element> {
        self.at += 1;
        let mut look_around = false;
        let mut insensitive = self.insensitive;
        if self.eat(b'?') {
            match self.peek()? {
                // Look-ahead and look-behind assert without consuming.
                b'=' | b'!' => {
                    self.at += 1;
                    look_around = true;
                }
                b'<' => {
                    self.at += 1;
                    if matches!(self.peek(), Some(b'=' | b'!')) {
                        self.at += 1;
                        look_around = true;
                    } else {
                        // A named capture group; skip the name.
                        while !matches!(self.peek(), None | Some(b'>')) {
                            self.at += 1;
                        }
                        if !self.eat(b'>') {
                            return None;
                        }
                    }
                }
                b':' => self.at += 1,
                _ => {
                    // Inline flags, either `(?flags)` or `(?flags:…)`.
                    let mut negated = false;
                    loop {
                        match self.peek()? {
                            b'i' => insensitive = !negated,
                            b'-' => negated = true,
                            b'm' | b's' | b'U' | b'x' | b'R' => {}
                            b':' => {
                                self.at += 1;
                                break;
                            }
                            b')' => {
                                self.at += 1;
                                // A flag-only group sets the mode for what follows and matches empty.
                                self.insensitive = insensitive;
                                return Some(Element {
                                    first: FirstBytes::none(),
                                    optional: true,
                                });
                            }
                            _ => return None,
                        }
                        self.at += 1;
                    }
                }
            }
        }
        let outer = self.insensitive;
        self.insensitive = insensitive;
        let inner = self.alternation(depth + 1);
        self.insensitive = outer;
        let inner = inner?;
        if !self.eat(b')') {
            return None;
        }
        if look_around {
            // Widening a look-around to "matches empty" keeps the set a superset: the element after
            // it supplies the first byte, and the assertion can only narrow what actually matches.
            return Some(Element {
                first: FirstBytes::none(),
                optional: true,
            });
        }
        Some(Element {
            first: inner,
            optional: inner.is_any(),
        })
    }

    /// A `[…]` character class, including a leading `^` negation and POSIX `[:name:]` sets.
    fn class(&mut self) -> Option<FirstBytes> {
        self.at += 1;
        let negated = self.eat(b'^');
        let mut set = FirstBytes::none();
        let mut first_position = true;
        loop {
            let byte = self.peek()?;
            if byte == b']' && !first_position {
                self.at += 1;
                break;
            }
            first_position = false;
            // A POSIX class names a set of ASCII characters; `[:^name:]` negates within the class.
            if byte == b'[' && self.chars.get(self.at + 1) == Some(&b':') {
                let close = self.find_posix_close()?;
                let name = self.chars.get(self.at + 2..close)?;
                set = set.union(posix_class(name)?);
                self.at = close + 2;
                continue;
            }
            let low = self.class_char()?;
            // A `-` before the closing bracket is a literal, not a range.
            if self.peek() == Some(b'-') && self.chars.get(self.at + 1) != Some(&b']') {
                self.at += 1;
                let high = self.class_char()?;
                set = set.union(char_range(low, high, self.insensitive));
                continue;
            }
            set = set.union(single_char(low, self.insensitive));
        }
        // Negation is taken over bytes: complementing the leading-byte set of the listed characters
        // would be wrong for multi-byte members, so any negated class admits every non-ASCII lead.
        if negated {
            let mut complement = set.complement();
            complement.insert_non_ascii();
            return Some(complement);
        }
        Some(set)
    }

    /// Index of the `[` in the `:]` that closes a POSIX class opened at `self.at`.
    fn find_posix_close(&self) -> Option<usize> {
        let mut at = self.at + 2;
        while let Some(byte) = self.chars.get(at) {
            if *byte == b':' && self.chars.get(at + 1) == Some(&b']') {
                return Some(at);
            }
            at += 1;
        }
        None
    }

    /// One member of a character class: an escape or a literal character.
    fn class_char(&mut self) -> Option<ClassMember> {
        if self.peek()? == b'\\' {
            self.at += 1;
            let byte = self.peek()?;
            if let Some(set) = escape_class(byte) {
                self.at += 1;
                return Some(ClassMember::Set(set));
            }
            return Some(ClassMember::Char(self.escape_char()?));
        }
        Some(ClassMember::Char(self.literal_char()?))
    }

    /// An escape outside a character class.
    fn escape(&mut self) -> Option<Element> {
        self.at += 1;
        let byte = self.peek()?;
        // Zero-width assertions.
        if matches!(byte, b'b' | b'B' | b'A' | b'z' | b'Z' | b'<' | b'>') {
            self.at += 1;
            return Some(Element {
                first: FirstBytes::none(),
                optional: true,
            });
        }
        if let Some(set) = escape_class(byte) {
            self.at += 1;
            return Some(Element {
                first: set,
                optional: false,
            });
        }
        // A back-reference can start with anything the referenced group could.
        if byte.is_ascii_digit() || byte == b'k' || byte == b'g' {
            return None;
        }
        let ch = self.escape_char()?;
        let mut first = FirstBytes::none();
        first.insert_char(ch, self.insensitive);
        Some(Element {
            first,
            optional: false,
        })
    }

    /// The character an escape sequence denotes, positioned on the character after the backslash.
    ///
    /// Only the named escapes and escaped punctuation are modeled. Any other letter escape may name a
    /// character class (`\p{L}`, `\h`) rather than a literal, so it yields `None` and widens the set.
    fn escape_char(&mut self) -> Option<char> {
        let byte = self.bump()?;
        Some(match byte {
            b'n' => '\n',
            b't' => '\t',
            b'r' => '\r',
            b'f' => '\u{c}',
            b'a' => '\u{7}',
            b'e' => '\u{1b}',
            b'0' => '\0',
            b'x' => return self.hex_escape(2),
            b'u' => return self.hex_escape(4),
            b'U' => return self.hex_escape(8),
            _ if byte.is_ascii_alphanumeric() => return None,
            _ if byte < 0x80 => char::from(byte),
            _ => {
                self.at -= 1;
                return self.literal_char();
            }
        })
    }

    /// A `\xHH`-style escape, or a braced `\x{…}` one.
    fn hex_escape(&mut self, digits: usize) -> Option<char> {
        if self.eat(b'{') {
            let start = self.at;
            while !matches!(self.peek(), None | Some(b'}')) {
                self.at += 1;
            }
            let text = std::str::from_utf8(self.chars.get(start..self.at)?).ok()?;
            if !self.eat(b'}') {
                return None;
            }
            return char::from_u32(u32::from_str_radix(text, 16).ok()?);
        }
        let start = self.at;
        self.at = (start + digits).min(self.chars.len());
        let text = std::str::from_utf8(self.chars.get(start..self.at)?).ok()?;
        char::from_u32(u32::from_str_radix(text, 16).ok()?)
    }

    /// The next literal character, decoded from UTF-8.
    fn literal_char(&mut self) -> Option<char> {
        let rest = self.chars.get(self.at..)?;
        let ch = std::str::from_utf8(rest)
            .ok()
            .and_then(|text| text.chars().next())
            // A pattern byte sequence that is not valid UTF-8 from here is not analyzable.
            ?;
        self.at += ch.len_utf8();
        Some(ch)
    }
}

/// A class member: one character, or a whole set contributed by a shorthand escape.
#[derive(Clone, Copy)]
enum ClassMember {
    Char(char),
    Set(FirstBytes),
}

fn single_char(member: ClassMember, insensitive: bool) -> FirstBytes {
    match member {
        ClassMember::Set(set) => set,
        ClassMember::Char(ch) => {
            let mut set = FirstBytes::none();
            set.insert_char(ch, insensitive);
            set
        }
    }
}

/// The leading bytes of a `low-high` class range. A range spanning non-ASCII characters is widened
/// to every non-ASCII lead rather than reasoning about UTF-8 boundaries.
fn char_range(low: ClassMember, high: ClassMember, insensitive: bool) -> FirstBytes {
    let (ClassMember::Char(low), ClassMember::Char(high)) = (low, high) else {
        // A shorthand escape as a range endpoint is not a range; be conservative.
        return FirstBytes::any();
    };
    let mut set = FirstBytes::none();
    if low.is_ascii() && high.is_ascii() {
        let (low, high) = (low as u8, high as u8);
        if low <= high {
            set.insert_range(low, high);
            if insensitive {
                // Case folding pairs each letter in the range with its other case.
                for byte in low..=high {
                    set.insert(byte.to_ascii_lowercase());
                    set.insert(byte.to_ascii_uppercase());
                }
                set.insert_non_ascii();
            }
        }
        return set;
    }
    if low.is_ascii() {
        set.insert_range(low as u8, 0x7F);
    }
    set.insert_non_ascii();
    set
}

/// The set a single-letter shorthand escape denotes, or `None` when the byte is not one.
///
/// `\d`, `\w` and `\s` are Unicode-aware here, so each also admits every non-ASCII lead; only their
/// ASCII membership is modeled exactly.
fn escape_class(byte: u8) -> Option<FirstBytes> {
    let mut set = FirstBytes::none();
    match byte {
        b'd' => {
            set.insert_range(b'0', b'9');
            set.insert_non_ascii();
        }
        b'w' => {
            set.insert_range(b'0', b'9');
            set.insert_range(b'a', b'z');
            set.insert_range(b'A', b'Z');
            set.insert(b'_');
            set.insert_non_ascii();
        }
        b's' => {
            for byte in [b' ', b'\t', b'\n', b'\r', 0x0b, 0x0c] {
                set.insert(byte);
            }
            set.insert_non_ascii();
        }
        // The negated shorthands admit every non-ASCII lead along with the ASCII complement.
        b'D' | b'W' | b'S' => {
            let positive = escape_class(byte.to_ascii_lowercase())?;
            let mut negated = positive.complement();
            negated.insert_non_ascii();
            return Some(negated);
        }
        // `\p{…}` and `\P{…}` name Unicode properties; do not model them.
        _ => return None,
    }
    Some(set)
}

/// The set a POSIX `[:name:]` class denotes, or `None` for an unknown or negated name.
fn posix_class(name: &[u8]) -> Option<FirstBytes> {
    let mut set = FirstBytes::none();
    match name {
        b"alpha" => {
            set.insert_range(b'a', b'z');
            set.insert_range(b'A', b'Z');
        }
        b"digit" => set.insert_range(b'0', b'9'),
        b"alnum" => {
            set.insert_range(b'a', b'z');
            set.insert_range(b'A', b'Z');
            set.insert_range(b'0', b'9');
        }
        b"upper" => set.insert_range(b'A', b'Z'),
        b"lower" => set.insert_range(b'a', b'z'),
        b"space" => {
            for byte in [b' ', b'\t', b'\n', b'\r', 0x0b, 0x0c] {
                set.insert(byte);
            }
        }
        b"blank" => {
            set.insert(b' ');
            set.insert(b'\t');
        }
        b"punct" => {
            set.insert_range(b'!', b'/');
            set.insert_range(b':', b'@');
            set.insert_range(b'[', b'`');
            set.insert_range(b'{', b'~');
        }
        b"xdigit" => {
            set.insert_range(b'0', b'9');
            set.insert_range(b'a', b'f');
            set.insert_range(b'A', b'F');
        }
        b"cntrl" => {
            set.insert_range(0, 0x1F);
            set.insert(0x7F);
        }
        b"print" => set.insert_range(0x20, 0x7E),
        b"graph" => set.insert_range(0x21, 0x7E),
        b"word" => {
            set.insert_range(b'a', b'z');
            set.insert_range(b'A', b'Z');
            set.insert_range(b'0', b'9');
            set.insert(b'_');
        }
        b"ascii" => set.insert_range(0, 0x7F),
        _ => return None,
    }
    Some(set)
}

#[cfg(test)]
mod tests;
