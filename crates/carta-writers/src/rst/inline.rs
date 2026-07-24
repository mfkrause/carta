//! Inline rendering for the reStructuredText writer: tokens, phrase markup, links, images, and escaping.

use carta_ast::{Attr, Block, Inline, QuoteType, Target, to_plain_text};

use crate::common::{
    Piece, ascii_punctuation, clean_prefix_len, indent_block, is_known_scheme, is_uri_scheme,
    label_matches_url, quote_marks,
};

use super::State;
use super::block::dimension_options;

/// An inline-rendering unit: an unbreakable text run carrying whether each of its edges is RST markup
/// (so that edge may need a `\ ` separator from an abutting run), a breakable space, a soft line break
/// from the source, or a forced line break. A word that only opens markup (e.g. `*one`) has a markup
/// leading edge but a plain trailing edge, so the two edges are tracked separately.
#[derive(Debug, Clone)]
pub(super) enum Token {
    Word {
        text: String,
        lead_complex: bool,
        trail_complex: bool,
        lead: char,
    },
    /// A zero-width boundary that prints nothing but, like markup, needs a `\ ` separator when it
    /// meets adjacent markup (a raw inline whose target format is not being emitted).
    Marker,
    Space,
    /// A breakable space originating from a soft line break in the source, distinct from a plain
    /// space so the fill engine can preserve the break when asked to.
    Soft,
    Hard,
}

/// Build a markup or plain word whose separator-boundary character is the first character of its
/// rendered text. Escaped plain text uses [`plain_word`] instead, since escaping can prepend a
/// backslash that is not the character RST would actually see.
fn word(text: String, complex: bool) -> Token {
    edge_word(text, complex, complex)
}

/// Build a word whose leading and trailing edges may carry markup independently. A boundary word that
/// only opens markup marks its leading edge complex and its trailing edge plain, and vice versa, so an
/// interior word abutting it on the plain side is not parted by a spurious `\ ` separator.
fn edge_word(text: String, lead_complex: bool, trail_complex: bool) -> Token {
    let lead = text.chars().next().unwrap_or('\0');
    Token::Word {
        text,
        lead_complex,
        trail_complex,
        lead,
    }
}

fn is_word_token(token: &Token) -> bool {
    matches!(token, Token::Word { .. })
}

/// Whether an inline sequence yields any visible output. An empty string and a breaking space
/// render to nothing, as does a formatting wrapper around blank content; anything else (a
/// non-empty string, a hard break, code, math, media, or a nested link) is content. A link or
/// image whose target and title are empty is dropped entirely when its label is blank, since there
/// is then nothing to anchor a reference to.
fn renders_visible(inlines: &[Inline]) -> bool {
    inlines.iter().any(|inline| match inline {
        Inline::Str(text) => !text.is_empty(),
        Inline::Space | Inline::SoftBreak => false,
        Inline::Emph(children)
        | Inline::Underline(children)
        | Inline::Strong(children)
        | Inline::SmallCaps(children)
        | Inline::Span(_, children)
        | Inline::Cite(_, children) => renders_visible(children),
        _ => true,
    })
}

impl State {
    /// Render inlines to a single flat line: spaces and forced breaks collapse to one space, with
    /// `\ ` separators inserted between adjacent markup boundaries. Used for content that must stay on
    /// one line (headers and the inside of inline markup).
    pub(super) fn flat(&mut self, inlines: &[Inline]) -> String {
        self.flat_nested(inlines, false)
    }

    /// Render a definition-list term: like [`flat`](Self::flat), but a forced line break stays a real
    /// newline so a term that spans lines is kept split across them.
    pub(super) fn term_line(&mut self, inlines: &[Inline]) -> String {
        let mut out = String::new();
        for piece in to_pieces(self.tokens_nested(inlines, false)) {
            match piece {
                Piece::Text(text) => out.push_str(&text),
                Piece::Space | Piece::Soft => out.push(' '),
                Piece::Hard => out.push('\n'),
            }
        }
        out
    }

    fn flat_nested(&mut self, inlines: &[Inline], in_emphasis: bool) -> String {
        flatten(self.tokens_nested(inlines, in_emphasis))
    }

    pub(super) fn tokens(&mut self, inlines: &[Inline]) -> Vec<Token> {
        self.tokens_nested(inlines, false)
    }

    fn tokens_nested(&mut self, inlines: &[Inline], in_emphasis: bool) -> Vec<Token> {
        let mut out = Vec::new();
        for inline in inlines {
            self.token(inline, in_emphasis, &mut out);
        }
        out
    }

    /// Render one inline. `in_emphasis` is set when the surrounding context is already an emphasis,
    /// strong, or similar phrase markup: RST cannot nest such markup, so a nested member of that
    /// family contributes its content as plain text rather than reopening markers.
    fn token(&mut self, inline: &Inline, in_emphasis: bool, out: &mut Vec<Token>) {
        match inline {
            Inline::Str(text) => out.push(Token::Word {
                text: escape(text, self.smart),
                lead_complex: false,
                trail_complex: false,
                lead: text.chars().next().unwrap_or('\0'),
            }),
            Inline::Space => out.push(Token::Space),
            Inline::SoftBreak => out.push(Token::Soft),
            Inline::LineBreak => out.push(Token::Hard),
            Inline::Emph(inlines) | Inline::Underline(inlines) => {
                self.phrase(inlines, in_emphasis, "*", "*", PhraseKind::Emph, out);
            }
            Inline::Strong(inlines) => {
                self.phrase(inlines, in_emphasis, "**", "**", PhraseKind::Strong, out);
            }
            Inline::Strikeout(inlines) => {
                let (open, close) = if in_emphasis {
                    ("", "")
                } else {
                    ("[STRIKEOUT:", "]")
                };
                self.wrapped(inlines, open, close, true, true, out);
            }
            Inline::Superscript(inlines) => {
                self.phrase(inlines, in_emphasis, ":sup:`", "`", PhraseKind::Leaf, out);
            }
            Inline::Subscript(inlines) => {
                self.phrase(inlines, in_emphasis, ":sub:`", "`", PhraseKind::Leaf, out);
            }
            Inline::SmallCaps(inlines) => {
                if in_emphasis {
                    for child in inlines {
                        self.token(child, true, out);
                    }
                } else {
                    out.push(word(self.flat_nested(inlines, false), true));
                }
            }
            Inline::Quoted(kind, inlines) => {
                let (open, close) = if self.smart {
                    match kind {
                        QuoteType::SingleQuote => ('\'', '\''),
                        QuoteType::DoubleQuote => ('"', '"'),
                    }
                } else {
                    quote_marks(kind)
                };
                self.wrapped(
                    inlines,
                    &open.to_string(),
                    &close.to_string(),
                    in_emphasis,
                    false,
                    out,
                );
            }
            Inline::Cite(_, inlines) | Inline::Span(_, inlines) => {
                let inner = self.tokens_nested(inlines, in_emphasis);
                out.extend(inner);
            }
            Inline::Code(_, text) => {
                let trimmed = text.trim_matches(' ');
                let rendered = if trimmed.is_empty() {
                    "````".to_owned()
                } else if trimmed.contains('`') {
                    literal_role(trimmed)
                } else {
                    format!("``{trimmed}``")
                };
                out.push(word(rendered, true));
            }
            Inline::Math(_, tex) => out.push(word(format!(":math:`{tex}`"), true)),
            Inline::RawInline(format, text) => {
                if format.0.eq_ignore_ascii_case("rst") {
                    out.push(word(text.to_string(), false));
                } else if format.0.eq_ignore_ascii_case("latex")
                    || format.0.eq_ignore_ascii_case("tex")
                {
                    out.push(word(format!(":raw-latex:`{text}`"), true));
                } else {
                    out.push(Token::Marker);
                }
            }
            Inline::Link(_, label, target) => self.link(label, target, out),
            Inline::Image(attr, alt, target) => {
                if in_emphasis {
                    for child in alt {
                        self.token(child, true, out);
                    }
                } else {
                    self.image(attr, alt, target, None, out);
                }
            }
            Inline::Note(blocks) => self.note(blocks, out),
        }
    }

    /// Render a phrase-markup inline (emphasis, strong, strikeout, super/subscript). Inside an
    /// existing phrase the markers are dropped and the content rendered inline. Otherwise the content
    /// is wrapped in the open/close markers; for emphasis and strong a child that cannot sit inside
    /// those markers (a link, or a strong span inside an emphasis) interrupts the run, closing the
    /// markers, rendering on its own, then reopening for the remainder.
    fn phrase(
        &mut self,
        inlines: &[Inline],
        in_emphasis: bool,
        open: &str,
        close: &str,
        kind: PhraseKind,
        out: &mut Vec<Token>,
    ) {
        if matches!(kind, PhraseKind::Leaf) {
            let body = self.flat_nested(inlines, true);
            let rendered = if in_emphasis {
                body
            } else {
                format!("{open}{body}{close}")
            };
            out.push(word(rendered, true));
            return;
        }
        // Inside an existing phrase the markers fall away, but breakout splitting stays in force.
        let (open, close) = if in_emphasis { ("", "") } else { (open, close) };
        let breakouts: Vec<usize> = inlines
            .iter()
            .enumerate()
            .filter(|(_, child)| breaks_out(child, kind))
            .map(|(index, _)| index)
            .collect();
        if breakouts.is_empty() {
            self.flush_phrase(inlines, open, close, out, false, false);
            return;
        }
        let mut run_start = 0;
        for (position, &index) in breakouts.iter().enumerate() {
            let segment = inlines.get(run_start..index).unwrap_or(&[]);
            self.flush_phrase(segment, open, close, out, position > 0, true);
            if let Some(child) = inlines.get(index) {
                self.token(child, in_emphasis, out);
            }
            run_start = index + 1;
        }
        let segment = inlines.get(run_start..).unwrap_or(&[]);
        self.flush_phrase(segment, open, close, out, true, false);
    }

    /// Wrap one uninterrupted run of phrase content in its markers, keeping any leading or trailing
    /// whitespace outside the markers (RST markup may not be padded by spaces on the inside). When the
    /// run abuts a broken-out child, a `\ ` null separator is placed just inside the marker on that
    /// side; an otherwise empty run between two such children collapses to a single `*\ *` placeholder.
    fn flush_phrase(
        &mut self,
        segment: &[Inline],
        open: &str,
        close: &str,
        out: &mut Vec<Token>,
        lead_break: bool,
        trail_break: bool,
    ) {
        let Some(split) = split_run(segment, lead_break, trail_break) else {
            if segment.is_empty() && lead_break && trail_break {
                out.push(word(format!("{open}\\ {close}"), true));
            } else {
                for inline in segment {
                    self.token(inline, true, out);
                }
            }
            return;
        };
        for inline in split.lead {
            if lead_break && matches!(inline, Inline::SoftBreak | Inline::LineBreak) {
                continue;
            }
            self.token(inline, true, out);
        }
        // Markers fuse to the first and last words; the words between stay separately breakable so long phrases reflow.
        let opening = format!("{open}{}", split.lead_sep);
        let closing = format!("{}{close}", split.trail_sep);
        let body = self.tokens_nested(split.middle, true);
        wrap_run(body, &opening, &closing, true, out);
        for inline in split.trail {
            if trail_break && matches!(inline, Inline::SoftBreak | Inline::LineBreak) {
                continue;
            }
            self.token(inline, true, out);
        }
    }

    /// Wrap an inline run in fixed open/close delimiters, leaving the words between them separately
    /// breakable so a long span can reflow and a source line break inside it survives. `complex`
    /// marks the fused boundary words as markup (so neighbouring text is parted by a `\ ` separator);
    /// smart-quote glyphs are plain text and pass `false`.
    fn wrapped(
        &mut self,
        inlines: &[Inline],
        open: &str,
        close: &str,
        in_emphasis: bool,
        complex: bool,
        out: &mut Vec<Token>,
    ) {
        let body = self.tokens_nested(inlines, in_emphasis);
        wrap_run(body, open, close, complex, out);
    }

    fn link(&mut self, label: &[Inline], target: &Target, out: &mut Vec<Token>) {
        let plain = to_plain_text(label);
        if target.url.is_empty() && target.title.is_empty() && !renders_visible(label) {
            return;
        }
        if let [Inline::Image(attr, alt, image_target)] = label {
            self.image(attr, alt, image_target, Some(&target.url), out);
            return;
        }
        if target.url == format!("mailto:{plain}") {
            out.push(word(plain, true));
            return;
        }
        if label_matches_url(&plain, &target.url) && is_standalone_uri(&target.url) {
            out.push(word(target.url.to_string(), true));
            return;
        }
        // Render the label once, then classify: a separate probing render would compound exponentially down a chain of nested links.
        let breakouts: Vec<usize> = label
            .iter()
            .enumerate()
            .filter(|(_, child)| matches!(child, Inline::Link(..)))
            .map(|(index, _)| index)
            .collect();
        let mut rendered = Vec::new();
        if breakouts.is_empty() {
            self.link_run(label, target, &mut rendered);
        } else {
            let mut run_start = 0;
            for &index in &breakouts {
                let segment = label.get(run_start..index).unwrap_or(&[]);
                self.link_run(segment, target, &mut rendered);
                if let Some(child) = label.get(index) {
                    self.token(child, true, &mut rendered);
                }
                run_start = index + 1;
            }
            let segment = label.get(run_start..).unwrap_or(&[]);
            self.link_run(segment, target, &mut rendered);
        }
        // A wordless label cannot anchor a reference; collapse to an empty-label reference.
        if rendered.iter().any(is_word_token) {
            out.extend(rendered);
        } else {
            out.push(word(format!("` <{}>`__", target.url), true));
        }
    }

    /// Render one run of link label that holds no nested link, wrapping it as `` `text <url>`__ `` with
    /// the label words left breakable so the fill engine may wrap between them. An empty run renders
    /// nothing.
    fn link_run(&mut self, label: &[Inline], target: &Target, out: &mut Vec<Token>) {
        let label_tokens = self.tokens_nested(label, true);
        let Some(first) = label_tokens.iter().position(is_word_token) else {
            return;
        };
        let suffix = format!(" <{}>`__", target.url);
        let last = label_tokens
            .iter()
            .rposition(is_word_token)
            .unwrap_or(first);
        for (index, token) in label_tokens.into_iter().enumerate() {
            match token {
                Token::Word { text, .. } if index == first && index == last => {
                    out.push(edge_word(format!("`{text}{suffix}"), true, true));
                }
                Token::Word { text, .. } if index == first => {
                    out.push(edge_word(format!("`{text}"), true, false));
                }
                Token::Word { text, .. } if index == last => {
                    out.push(edge_word(format!("{text}{suffix}"), false, true));
                }
                other => out.push(other),
            }
        }
    }

    /// Render an image. A link nested in the alt text cannot live inside a substitution, so it
    /// interrupts the run: the alt splits around each link, each surrounding run becomes its own image
    /// substitution, and the link renders inline between the references.
    fn image(
        &mut self,
        attr: &Attr,
        alt: &[Inline],
        target: &Target,
        link: Option<&str>,
        out: &mut Vec<Token>,
    ) {
        if link.is_none()
            && target.url.is_empty()
            && target.title.is_empty()
            && !renders_visible(alt)
        {
            return;
        }
        let breakouts: Vec<usize> = alt
            .iter()
            .enumerate()
            .filter(|(_, child)| matches!(child, Inline::Link(..)))
            .map(|(index, _)| index)
            .collect();
        if breakouts.is_empty() {
            let name = self.substitution_name(to_plain_text(alt));
            self.register_image(attr, &name, target, link, out);
            return;
        }
        let mut run_start = 0;
        for (position, &index) in breakouts.iter().enumerate() {
            let segment = alt.get(run_start..index).unwrap_or(&[]);
            self.image_run(attr, segment, target, out, position > 0, true);
            if let Some(child) = alt.get(index) {
                self.token(child, false, out);
            }
            run_start = index + 1;
        }
        let segment = alt.get(run_start..).unwrap_or(&[]);
        self.image_run(attr, segment, target, out, true, false);
    }

    /// Emit one run of alt text sitting beside a broken-out link as its own image substitution. Spaces
    /// at the run's edge stay outside the reference; where the run abuts the link without a space, a
    /// `\ ` null separator is folded into the substitution name so the reference reads cleanly.
    fn image_run(
        &mut self,
        attr: &Attr,
        segment: &[Inline],
        target: &Target,
        out: &mut Vec<Token>,
        lead_break: bool,
        trail_break: bool,
    ) {
        let Some(split) = split_run(segment, lead_break, trail_break) else {
            for inline in segment {
                self.token(inline, false, out);
            }
            return;
        };
        for inline in split.lead {
            self.token(inline, false, out);
        }
        let candidate = format!("{}{}{}", split.lead_sep, split.plain, split.trail_sep);
        let name = self.substitution_name(candidate);
        self.register_image(attr, &name, target, None, out);
        for inline in split.trail {
            self.token(inline, false, out);
        }
    }

    /// The substitution name for an image labelled `plain`: its own label when that is non-empty and
    /// not already taken, otherwise a generated `image`-plus-counter name. The counter advances on
    /// every fallback, so an empty or repeated label always yields a fresh name.
    pub(super) fn substitution_name(&mut self, plain: String) -> String {
        let name = if plain.is_empty() || self.used_names.contains(&plain) {
            self.fallback_count += 1;
            format!("image{}", self.fallback_count)
        } else {
            plain
        };
        self.used_names.push(name.clone());
        name
    }

    fn register_image(
        &mut self,
        attr: &Attr,
        name: &str,
        target: &Target,
        link: Option<&str>,
        out: &mut Vec<Token>,
    ) {
        let mut definition = format!(".. |{name}| image:: {}", target.url);
        for option in dimension_options(attr) {
            definition.push_str("\n   ");
            definition.push_str(&option);
        }
        if let Some(url) = link {
            definition.push_str("\n   :target: ");
            definition.push_str(url);
        }
        self.substitutions.push(definition);
        out.push(word(format!("|{name}|"), true));
    }

    fn note(&mut self, blocks: &[Block], out: &mut Vec<Token>) {
        let index = self.footnotes.len();
        self.footnotes.push(String::new());
        let number = index + 1;
        let body = self.blocks_to_string(blocks, self.width.saturating_sub(3), false);
        let entry = if body.is_empty() {
            format!(".. [{number}]")
        } else {
            format!(".. [{number}]\n{}", indent_block(&body, "   ", "   "))
        };
        if let Some(slot) = self.footnotes.get_mut(index) {
            *slot = entry;
        }
        out.push(word(format!(" [{number}]_"), false));
    }
}

/// Build the piece stream for the fill engine from inline tokens, inserting a `\ ` separator between
/// adjacent markup boundaries that RST would not otherwise recognize.
pub(super) fn to_pieces(tokens: Vec<Token>) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut pending: Option<(bool, char)> = None;
    for token in tokens {
        match token {
            Token::Word {
                text,
                lead_complex,
                trail_complex,
                lead,
            } => {
                let Some(last) = text.chars().last() else {
                    continue;
                };
                if let Some((previous_trail_complex, previous_last)) = pending
                    && separator_needed(previous_trail_complex, previous_last, lead_complex, lead)
                {
                    out.push(Piece::text("\\ "));
                }
                out.push(Piece::text(text));
                pending = Some((trail_complex, last));
            }
            Token::Marker => {
                if pending.is_some_and(|(previous_complex, _)| previous_complex) {
                    out.push(Piece::text("\\ "));
                }
                pending = Some((false, MARKER_BOUNDARY));
            }
            Token::Space => {
                out.push(Piece::Space);
                pending = None;
            }
            Token::Soft => {
                out.push(Piece::Soft);
                pending = None;
            }
            Token::Hard => {
                out.push(Piece::Hard);
                pending = None;
            }
        }
    }
    out
}

/// Emit `body` wrapped in `opening`/`closing` delimiters while keeping its internal spaces breakable:
/// the opening fuses to the first word token and the closing to the last, so a long run reflows with
/// the delimiters anchored to their boundary words. `complex` marks the boundary words as markup so
/// the `\ ` null-separator rules apply around them. A body with no word token collapses to a single
/// flattened word carrying both delimiters.
fn wrap_run(body: Vec<Token>, opening: &str, closing: &str, complex: bool, out: &mut Vec<Token>) {
    let first = body.iter().position(is_word_token);
    let last = body.iter().rposition(is_word_token);
    match (first, last) {
        (Some(first), Some(last)) => {
            for (index, token) in body.into_iter().enumerate() {
                match token {
                    Token::Word { text, .. } if index == first && index == last => {
                        out.push(edge_word(
                            format!("{opening}{text}{closing}"),
                            complex,
                            complex,
                        ));
                    }
                    Token::Word { text, .. } if index == first => {
                        out.push(edge_word(format!("{opening}{text}"), complex, false));
                    }
                    Token::Word { text, .. } if index == last => {
                        out.push(edge_word(format!("{text}{closing}"), false, complex));
                    }
                    other => out.push(other),
                }
            }
        }
        _ => out.push(word(
            format!("{opening}{}{closing}", flatten(body)),
            complex,
        )),
    }
}

/// Flatten inline tokens to a single line: spaces and forced breaks become one space, with the same
/// `\ ` separators [`to_pieces`] inserts between adjacent markup boundaries.
pub(super) fn flatten(tokens: Vec<Token>) -> String {
    let mut out = String::new();
    for piece in to_pieces(tokens) {
        match piece {
            Piece::Text(text) => out.push_str(&text),
            Piece::Space | Piece::Soft | Piece::Hard => out.push(' '),
        }
    }
    out
}

/// The boundary character a [`Token::Marker`] presents to a following run: a value that is neither a
/// safe follower nor a safe preceder, so adjacent markup is always separated from it.
const MARKER_BOUNDARY: char = '\0';

/// Whether a `\ ` separator is needed between two adjacent inline runs: a markup run meeting a
/// character that cannot legally follow it, or one preceded by a character that cannot legally
/// precede it.
fn separator_needed(
    previous_trail_complex: bool,
    previous_last: char,
    current_lead_complex: bool,
    current_first: char,
) -> bool {
    (previous_trail_complex && !is_safe_follower(current_first))
        || (current_lead_complex && !is_safe_preceder(previous_last))
}

/// A phrase run partitioned around its non-space core, with the null-separator decision for each
/// edge that abuts a broken-out child.
struct RunSplit<'a> {
    lead: &'a [Inline],
    middle: &'a [Inline],
    trail: &'a [Inline],
    plain: String,
    lead_sep: &'static str,
    trail_sep: &'static str,
}

/// Partition a run into leading whitespace, its non-space core, and trailing whitespace, returning
/// `None` when the run holds no non-space content. `lead_sep`/`trail_sep` carry the `\ ` null
/// separator for an edge that abuts a broken-out child where the core character would otherwise read
/// as continuing markup.
fn split_run(segment: &[Inline], lead_break: bool, trail_break: bool) -> Option<RunSplit<'_>> {
    let is_space = |inline: &Inline| {
        matches!(
            inline,
            Inline::Space | Inline::SoftBreak | Inline::LineBreak
        )
    };
    let first = segment.iter().position(|inline| !is_space(inline))?;
    let last = segment
        .iter()
        .rposition(|inline| !is_space(inline))
        .unwrap_or(first);
    let middle = segment.get(first..=last).unwrap_or(&[]);
    let plain = to_plain_text(middle);
    let lead_sep =
        if lead_break && first == 0 && plain.chars().next().is_some_and(|c| !is_safe_follower(c)) {
            "\\ "
        } else {
            ""
        };
    let trail_sep = if trail_break
        && last + 1 == segment.len()
        && plain.chars().last().is_some_and(|c| !is_safe_preceder(c))
    {
        "\\ "
    } else {
        ""
    };
    Some(RunSplit {
        lead: segment.get(..first).unwrap_or(&[]),
        middle,
        trail: segment.get(last + 1..).unwrap_or(&[]),
        plain,
        lead_sep,
        trail_sep,
    })
}

/// Characters that may directly precede an inline-markup start-string.
const OPENERS: &[char] = &['-', ':', '/', '\'', '"', '<', '(', '[', '{'];

/// Characters that may directly follow an inline-markup end-string.
const CLOSERS: &[char] = &[
    '-', '.', ',', ':', ';', '!', '?', '\'', '"', ')', ']', '}', '>',
];

/// Characters that may directly follow an inline-markup end without a separator: whitespace (any
/// space, including a non-breaking space), a backslash or slash, or any end-string closer.
fn is_safe_follower(ch: char) -> bool {
    ch.is_whitespace() || ch == '\\' || ch == '/' || CLOSERS.contains(&ch)
}

/// Characters that may directly precede an inline-markup start without a separator: whitespace (any
/// space, including a non-breaking space) or any start-string opener.
fn is_safe_preceder(ch: char) -> bool {
    ch.is_whitespace() || OPENERS.contains(&ch)
}

/// Escape the characters of a text run that RST would otherwise read as markup. A backslash is always
/// doubled. A `*`, backtick, or `|` is escaped where it could open or close inline markup given its
/// neighbors. A `_` is a reference marker: it is escaped everywhere except where it is buried directly
/// before an alphanumeric and is not itself opening at a word boundary.
pub(super) fn escape(text: &str, smart: bool) -> String {
    let is_trigger =
        |byte: u8| matches!(byte, b'\\' | b'*' | b'`' | b'|' | b'_') || (smart && byte >= 0x80);
    let mut out = String::new();
    let mut prev: Option<char> = None;
    let mut rest = text;
    loop {
        let clean = clean_prefix_len(rest, is_trigger);
        let Some((head, tail)) = rest.split_at_checked(clean) else {
            out.push_str(rest);
            break;
        };
        out.push_str(head);
        prev = head.chars().next_back().or(prev);
        let mut chars = tail.chars();
        let Some(ch) = chars.next() else { break };
        let next = chars.clone().next();
        match ch {
            '\\' => out.push_str("\\\\"),
            '*' | '`' | '|' => {
                if flanking_escape(prev, next) {
                    out.push('\\');
                }
                out.push(ch);
            }
            '_' => {
                if underscore_escape(prev, next) {
                    out.push('\\');
                }
                out.push(ch);
            }
            // With `smart`, Unicode punctuation collapses to its ASCII form.
            _ => match smart.then(|| ascii_punctuation(ch)).flatten() {
                Some(ascii) => out.push_str(ascii),
                None => out.push(ch),
            },
        }
        prev = Some(ch);
        rest = chars.as_str();
    }
    out
}

/// Whether a `*`, backtick, or `|` could be read as an inline-markup delimiter: a start-string sits at
/// a boundary or opener and is not followed by whitespace; an end-string follows non-whitespace and is
/// not followed by other text. A run boundary counts as both an opener and a closer.
fn flanking_escape(prev: Option<char>, next: Option<char>) -> bool {
    let could_start = prev.is_none_or(|c| c.is_whitespace() || OPENERS.contains(&c))
        && next.is_none_or(|c| !c.is_whitespace());
    let could_end = prev.is_some_and(|c| !c.is_whitespace())
        && next.is_none_or(|c| c.is_whitespace() || CLOSERS.contains(&c));
    could_start || could_end
}

/// Whether a `_` needs escaping: only a `_` buried directly before an alphanumeric, with a preceding
/// non-whitespace, non-opener character, is safe to leave bare.
fn underscore_escape(prev: Option<char>, next: Option<char>) -> bool {
    let buried = next.is_some_and(char::is_alphanumeric)
        && prev.is_some_and(|c| !c.is_whitespace() && !OPENERS.contains(&c));
    !buried
}

/// Render inline code that contains a backtick as a `:literal:` role. A backtick is backslash-escaped
/// when exactly one of its neighbors is whitespace or a content edge, the positions where it would
/// otherwise merge with the role's own delimiters.
fn literal_role(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut body = String::new();
    for (index, &ch) in chars.iter().enumerate() {
        if ch == '\\' {
            body.push_str("\\\\");
            continue;
        }
        if ch == '`' {
            let before_space = index == 0
                || chars
                    .get(index.wrapping_sub(1))
                    .is_some_and(|c| c.is_whitespace());
            let after_space = chars.get(index + 1).is_none_or(|c| c.is_whitespace());
            if before_space != after_space {
                body.push('\\');
            }
        }
        body.push(ch);
    }
    format!(":literal:`{body}`")
}

/// Whether a link whose visible text equals its address may be written as a bare standalone URI,
/// which RST recognizes only for an address carrying a registered scheme and built solely from URI
/// characters.
fn is_standalone_uri(url: &str) -> bool {
    let Some((scheme, _)) = url.split_once(':') else {
        return false;
    };
    is_uri_scheme(scheme) && is_known_scheme(scheme) && url.chars().all(is_uri_char)
}

/// Whether a character may appear in a standalone URI reference.
fn is_uri_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || "-._~:/?#@!$&'()*+,;=%".contains(ch)
}

/// The phrase-markup family being rendered: leaf markup that admits no break-out (strikeout,
/// super/subscript), emphasis, or strong.
#[derive(Debug, Clone, Copy)]
enum PhraseKind {
    Leaf,
    Emph,
    Strong,
}

/// Whether an inline must be lifted out of surrounding emphasis or strong markup: a link always, and
/// a strong span when it sits inside emphasis.
fn breaks_out(inline: &Inline, kind: PhraseKind) -> bool {
    matches!(inline, Inline::Link(..))
        || (matches!(kind, PhraseKind::Emph) && matches!(inline, Inline::Strong(_)))
}
