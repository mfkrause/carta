//! Inline-emission, formatting, symbol, and accent helpers for the LaTeX reader.

use carta_ast::{Attr, Block, Inline, Target, to_plain_text};

pub(super) fn flush_buf(buf: &mut String, out: &mut Vec<Inline>) {
    if !buf.is_empty() {
        out.push(Inline::Str(std::mem::take(buf).into()));
    }
}

/// Flushes the pending text buffer, then appends an inline, so buffered text stays ahead of it.
pub(super) fn emit(out: &mut Vec<Inline>, buf: &mut String, inline: Inline) {
    flush_buf(buf, out);
    out.push(inline);
}

pub(super) fn emit_all(out: &mut Vec<Inline>, buf: &mut String, inlines: Vec<Inline>) {
    flush_buf(buf, out);
    out.extend(inlines);
}

/// Appends a whitespace break, coalescing runs of spacing (which arise around dropped commands)
/// into a single break, with a soft line break taking precedence over a plain space.
pub(super) fn push_whitespace(out: &mut Vec<Inline>, ws: Inline) {
    match out.last() {
        // A trailing plain space is promoted when the new break is soft.
        Some(Inline::Space) if matches!(ws, Inline::SoftBreak) => {
            if let Some(last) = out.last_mut() {
                *last = Inline::SoftBreak;
            }
        }
        // Any existing break or space already separates; further spacing is swallowed.
        Some(Inline::LineBreak | Inline::SoftBreak | Inline::Space) => {}
        _ => out.push(ws),
    }
}

pub(super) fn trim_inlines(mut inlines: Vec<Inline>) -> Vec<Inline> {
    while matches!(inlines.first(), Some(Inline::Space | Inline::SoftBreak)) {
        inlines.remove(0);
    }
    while matches!(inlines.last(), Some(Inline::Space | Inline::SoftBreak)) {
        inlines.pop();
    }
    inlines
}

/// The intrinsic nesting level of a sectioning command, before the document-wide level offset:
/// `\part` is -1, `\chapter` 0, `\section` 1, and so on down to `\subparagraph` at 5.
pub(super) fn section_intrinsic(name: &str) -> Option<i32> {
    match name {
        "part" => Some(-1),
        "chapter" => Some(0),
        "section" => Some(1),
        "subsection" => Some(2),
        "subsubsection" => Some(3),
        "paragraph" => Some(4),
        "subparagraph" => Some(5),
        _ => None,
    }
}

/// Whether `env` typesets mathematics and so is inline content rather than a block environment.
pub(super) fn math_env(env: &str) -> bool {
    matches!(
        env,
        "math"
            | "displaymath"
            | "equation"
            | "equation*"
            | "align"
            | "align*"
            | "alignat"
            | "alignat*"
            | "gather"
            | "gather*"
            | "multline"
            | "multline*"
            | "flalign"
            | "flalign*"
            | "eqnarray"
            | "eqnarray*"
            | "split"
            | "cases"
    )
}

/// The formatter that wraps a braced group's inlines, for the simple font/emphasis commands.
pub(super) fn inline_wrapper(name: &str) -> Option<fn(Vec<Inline>) -> Inline> {
    match name {
        "emph" | "textit" | "textsl" | "italic" | "emphasize" => Some(Inline::Emph),
        "textbf" | "strong" => Some(Inline::Strong),
        "underline" | "uline" => Some(Inline::Underline),
        "textsc" => Some(Inline::SmallCaps),
        "sout" | "st" => Some(Inline::Strikeout),
        _ => None,
    }
}

/// The CSS class a font-family/shape/series command wraps its argument in, or `None` for a command
/// that is not one of these span-producing font switches.
pub(super) fn font_span_class(name: &str) -> Option<&'static str> {
    match name {
        "textrm" | "textnormal" => Some("roman"),
        "textsf" => Some("sans-serif"),
        "textup" => Some("upright"),
        "textmd" => Some("medium"),
        _ => None,
    }
}

/// Wraps inlines in a formatter, but lifts leading and trailing spacing out of the wrapper so it
/// stays between words rather than inside the formatted run.
pub(super) fn extract_spaces<F: FnOnce(Vec<Inline>) -> Inline>(
    mut inner: Vec<Inline>,
    wrap: F,
) -> Vec<Inline> {
    let leading =
        matches!(inner.first(), Some(Inline::Space | Inline::SoftBreak)).then(|| inner.remove(0));
    let trailing = if matches!(inner.last(), Some(Inline::Space | Inline::SoftBreak)) {
        inner.pop()
    } else {
        None
    };
    let mut result = Vec::new();
    result.extend(leading);
    result.push(wrap(inner));
    result.extend(trailing);
    result
}

pub(super) fn span_class(inlines: Vec<Inline>, class: &str) -> Inline {
    Inline::Span(
        Box::new(Attr {
            id: carta_ast::Text::default(),
            classes: vec![class.into()],
            attributes: Vec::new(),
        }),
        inlines,
    )
}

/// A cross-reference command becomes a link to the anchor, showing the raw target text.
pub(super) fn reference_link(name: &str, target: &str) -> Inline {
    let kind = match name {
        "autoref" | "cref" => "ref+label",
        "Cref" => "ref+Label",
        other => other,
    };
    Inline::Link(
        Box::new(Attr {
            id: carta_ast::Text::default(),
            classes: Vec::new(),
            attributes: vec![
                ("reference-type".into(), kind.into()),
                ("reference".into(), target.into()),
            ],
        }),
        vec![Inline::Str(format!("[{target}]").into())],
        Box::new(Target {
            url: format!("#{target}").into(),
            title: carta_ast::Text::default(),
        }),
    )
}

/// A font-switch command that formats the remainder of its enclosing group.
#[derive(Clone, Copy)]
pub(super) enum Switch {
    Strong,
    Emph,
    SmallCaps,
    Code,
}

impl Switch {
    pub(super) fn wrap(self, inner: Vec<Inline>) -> Inline {
        match self {
            Switch::Strong => Inline::Strong(inner),
            Switch::Emph => Inline::Emph(inner),
            Switch::SmallCaps => Inline::SmallCaps(inner),
            Switch::Code => Inline::Code(Box::default(), to_plain_text(&inner).into()),
        }
    }
}

pub(super) fn switch_kind(name: &str) -> Option<Switch> {
    Some(match name {
        "bf" | "bfseries" => Switch::Strong,
        "it" | "itshape" | "em" | "sl" | "slshape" => Switch::Emph,
        "scshape" => Switch::SmallCaps,
        "tt" => Switch::Code,
        _ => return None,
    })
}

/// Wraps a bare brace group's content in a null-attribute span to preserve its grouping. An empty
/// group vanishes; a group that already is a single null-attribute span is not wrapped again.
pub(super) fn group_span(inner: Vec<Inline>) -> Option<Inline> {
    match inner.first() {
        None => None,
        Some(Inline::Span(attr, _))
            if inner.len() == 1
                && attr.id.is_empty()
                && attr.classes.is_empty()
                && attr.attributes.is_empty() =>
        {
            inner.into_iter().next()
        }
        _ => Some(Inline::Span(Box::default(), inner)),
    }
}

/// A combining diacritic requested by an accent command.
#[derive(Clone, Copy)]
pub(super) enum Accent {
    Acute,
    Grave,
    Circumflex,
    Tilde,
    Diaeresis,
    Macron,
    DotAbove,
    Cedilla,
    Caron,
    Breve,
    DoubleAcute,
    Ring,
    Ogonek,
}

/// The accent for a control-word accent command (`\c`, `\v`, `\u`, `\H`, `\r`, `\k`). The
/// control-symbol accents (`\'`, `` \` ``, `\^`, …) are dispatched separately.
pub(super) fn word_accent(name: &str) -> Option<Accent> {
    match name {
        "c" => Some(Accent::Cedilla),
        "v" => Some(Accent::Caron),
        "u" => Some(Accent::Breve),
        "H" => Some(Accent::DoubleAcute),
        "r" => Some(Accent::Ring),
        "k" => Some(Accent::Ogonek),
        _ => None,
    }
}

/// The standalone combining mark for an accent, used when no precomposed character exists.
pub(super) fn combining_mark(accent: Accent) -> char {
    match accent {
        Accent::Acute => '\u{301}',
        Accent::Grave => '\u{300}',
        Accent::Circumflex => '\u{302}',
        Accent::Tilde => '\u{303}',
        Accent::Diaeresis => '\u{308}',
        Accent::Macron => '\u{304}',
        Accent::DotAbove => '\u{307}',
        Accent::Cedilla => '\u{327}',
        Accent::Caron => '\u{30c}',
        Accent::Breve => '\u{306}',
        Accent::DoubleAcute => '\u{30b}',
        Accent::Ring => '\u{30a}',
        Accent::Ogonek => '\u{328}',
    }
}

/// Resolves an accent's braced argument to its base text: a control sequence such as `\i` becomes its
/// glyph, and plain text is returned unchanged.
pub(super) fn resolve_accent_base(raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix('\\') {
        let word: String = rest.chars().take_while(char::is_ascii_alphabetic).collect();
        if !word.is_empty() {
            return symbol_text(&word).map_or(word, str::to_owned);
        }
    }
    raw.to_owned()
}

/// Applies an accent to its base text, producing the precomposed character when one exists and
/// otherwise the base followed by the standalone combining mark. An empty argument yields nothing.
pub(super) fn apply_accent(accent: Accent, base: Option<&str>) -> String {
    let base = base.unwrap_or("");
    let mut chars = base.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let rest: String = chars.collect();
    match combine_accent(accent, first) {
        Some(composed) => format!("{composed}{rest}"),
        None => format!("{first}{}{rest}", combining_mark(accent)),
    }
}

/// The precomposed character for a base letter and accent, for the common Latin-1/Latin-A cases.
#[allow(clippy::too_many_lines)]
pub(super) fn combine_accent(accent: Accent, base: char) -> Option<char> {
    let table: &[(char, char)] = match accent {
        Accent::Acute => &[
            ('a', 'á'),
            ('e', 'é'),
            ('i', 'í'),
            ('o', 'ó'),
            ('u', 'ú'),
            ('y', 'ý'),
            ('c', 'ć'),
            ('n', 'ń'),
            ('s', 'ś'),
            ('z', 'ź'),
            ('r', 'ŕ'),
            ('l', 'ĺ'),
            ('A', 'Á'),
            ('E', 'É'),
            ('I', 'Í'),
            ('O', 'Ó'),
            ('U', 'Ú'),
            ('Y', 'Ý'),
            ('C', 'Ć'),
            ('N', 'Ń'),
            ('S', 'Ś'),
            ('Z', 'Ź'),
        ],
        Accent::Grave => &[
            ('a', 'à'),
            ('e', 'è'),
            ('i', 'ì'),
            ('o', 'ò'),
            ('u', 'ù'),
            ('A', 'À'),
            ('E', 'È'),
            ('I', 'Ì'),
            ('O', 'Ò'),
            ('U', 'Ù'),
        ],
        Accent::Circumflex => &[
            ('a', 'â'),
            ('e', 'ê'),
            ('i', 'î'),
            ('o', 'ô'),
            ('u', 'û'),
            ('A', 'Â'),
            ('E', 'Ê'),
            ('I', 'Î'),
            ('O', 'Ô'),
            ('U', 'Û'),
        ],
        Accent::Tilde => &[
            ('a', 'ã'),
            ('o', 'õ'),
            ('n', 'ñ'),
            ('A', 'Ã'),
            ('O', 'Õ'),
            ('N', 'Ñ'),
        ],
        Accent::Diaeresis => &[
            ('a', 'ä'),
            ('e', 'ë'),
            ('i', 'ï'),
            ('o', 'ö'),
            ('u', 'ü'),
            ('y', 'ÿ'),
            ('A', 'Ä'),
            ('E', 'Ë'),
            ('I', 'Ï'),
            ('O', 'Ö'),
            ('U', 'Ü'),
        ],
        Accent::Macron => &[
            ('a', 'ā'),
            ('e', 'ē'),
            ('i', 'ī'),
            ('o', 'ō'),
            ('u', 'ū'),
            ('A', 'Ā'),
            ('E', 'Ē'),
            ('I', 'Ī'),
            ('O', 'Ō'),
            ('U', 'Ū'),
        ],
        Accent::DotAbove => &[('e', 'ė'), ('z', 'ż'), ('E', 'Ė'), ('Z', 'Ż')],
        Accent::Cedilla => &[
            ('c', 'ç'),
            ('s', 'ş'),
            ('t', 'ţ'),
            ('g', 'ģ'),
            ('C', 'Ç'),
            ('S', 'Ş'),
            ('T', 'Ţ'),
        ],
        Accent::Caron => &[
            ('c', 'č'),
            ('s', 'š'),
            ('z', 'ž'),
            ('r', 'ř'),
            ('e', 'ě'),
            ('n', 'ň'),
            ('d', 'ď'),
            ('t', 'ť'),
            ('l', 'ľ'),
            ('C', 'Č'),
            ('S', 'Š'),
            ('Z', 'Ž'),
            ('R', 'Ř'),
            ('E', 'Ě'),
            ('N', 'Ň'),
        ],
        Accent::Breve => &[
            ('a', 'ă'),
            ('e', 'ĕ'),
            ('g', 'ğ'),
            ('i', 'ĭ'),
            ('o', 'ŏ'),
            ('u', 'ŭ'),
            ('A', 'Ă'),
            ('G', 'Ğ'),
        ],
        Accent::DoubleAcute => &[('o', 'ő'), ('u', 'ű'), ('O', 'Ő'), ('U', 'Ű')],
        Accent::Ring => &[('a', 'å'), ('u', 'ů'), ('A', 'Å'), ('U', 'Ů')],
        Accent::Ogonek => &[
            ('a', 'ą'),
            ('e', 'ę'),
            ('i', 'į'),
            ('u', 'ų'),
            ('A', 'Ą'),
            ('E', 'Ę'),
        ],
    };
    table.iter().find(|(b, _)| *b == base).map(|(_, c)| *c)
}

/// The literal text a symbol or named-glyph command produces.
pub(super) fn symbol_text(name: &str) -> Option<&'static str> {
    let text = match name {
        "LaTeX" => "LaTeX",
        "TeX" => "TeX",
        "ldots" | "dots" => "\u{2026}",
        "textbackslash" => "\\",
        "textasciitilde" => "~",
        "textasciicircum" => "^",
        "textless" => "<",
        "textgreater" => ">",
        "textbullet" => "\u{2022}",
        "textquoteright" => "\u{2019}",
        "textquoteleft" => "\u{2018}",
        "textquotedblright" => "\u{201d}",
        "textquotedblleft" => "\u{201c}",
        "textregistered" => "\u{ae}",
        "textcopyright" | "copyright" => "\u{a9}",
        "textdegree" => "\u{b0}",
        "textdagger" => "\u{2020}",
        "S" | "textsection" => "\u{a7}",
        "P" | "textparagraph" => "\u{b6}",
        "pounds" | "textsterling" => "\u{a3}",
        "euro" => "\u{20ac}",
        "textyen" => "\u{a5}",
        "guillemotleft" => "\u{ab}",
        "guillemotright" => "\u{bb}",
        "aa" => "\u{e5}",
        "AA" => "\u{c5}",
        "ae" => "\u{e6}",
        "AE" => "\u{c6}",
        "oe" => "\u{153}",
        "OE" => "\u{152}",
        "o" => "\u{f8}",
        "O" => "\u{d8}",
        "ss" => "\u{df}",
        "l" => "\u{142}",
        "L" => "\u{141}",
        "i" => "\u{131}",
        "j" => "\u{237}",
        "textquotesingle" => "'",
        "textquotedbl" => "\"",
        "slash" => "/",
        "&" => "&",
        _ => return None,
    };
    Some(text)
}

/// The number of trailing braced arguments a dropped command consumes.
pub(super) fn command_arg_count(name: &str) -> usize {
    match name {
        "setlength" | "addtolength" | "setcounter" | "addtocounter" | "settowidth"
        | "definecolor" | "rule" | "newtheorem" => 2,
        "hspace" | "vspace" | "hskip" | "vskip" | "vphantom" | "hphantom" | "phantom"
        | "raisebox" | "pagestyle" | "thispagestyle" | "pagenumbering" | "documentclass"
        | "usepackage" | "RequirePackage" | "geometry" | "hypersetup" | "bibliographystyle"
        | "include" | "input" | "graphicspath" | "theoremstyle" | "captionsetup"
        | "bibliography" => 1,
        _ => 0,
    }
}

/// Splits raw source on a `\name` control word at brace depth zero, returning the between-parts.
pub(super) fn split_on_command(raw: &str, name: &str) -> Vec<String> {
    let marker: Vec<char> = format!("\\{name}").chars().collect();
    let chars: Vec<char> = raw.chars().collect();
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        match c {
            '{' => depth += 1,
            '}' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            '\\' if depth == 0 => {
                let matches_marker = marker
                    .iter()
                    .enumerate()
                    .all(|(k, m)| chars.get(i + k) == Some(m));
                let after = chars.get(i + marker.len());
                let is_word_boundary = after.is_none_or(|c| !c.is_ascii_alphabetic());
                if matches_marker && is_word_boundary {
                    parts.push(std::mem::take(&mut current));
                    i += marker.len();
                    continue;
                }
            }
            _ => {}
        }
        current.push(c);
        i += 1;
    }
    parts.push(current);
    parts
}

/// Converts a paragraph made up solely of images (and spacing) into a plain block, matching how a
/// bare image line reads inside a figure.
pub(super) fn demote_image_para(block: Block) -> Block {
    if let Block::Para(inlines) = &block {
        let has_image = inlines.iter().any(|i| matches!(i, Inline::Image(..)));
        let only_images = inlines.iter().all(|i| {
            matches!(
                i,
                Inline::Image(..) | Inline::Space | Inline::SoftBreak | Inline::LineBreak
            )
        });
        if has_image
            && only_images
            && let Block::Para(inlines) = block
        {
            return Block::Plain(inlines);
        }
    }
    block
}

/// Substitutes `#1`…`#9` in a macro body with the given argument strings.
pub(super) fn substitute_macro(body: &str, args: &[String]) -> String {
    let mut out = String::new();
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '#' {
            match chars.peek() {
                Some(d) if matches!(d, '1'..='9') => {
                    let idx = (*d as usize) - ('1' as usize);
                    chars.next();
                    if let Some(arg) = args.get(idx) {
                        out.push_str(arg);
                    }
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
    }
    out
}

/// Removes a backslash before a character that LaTeX escapes in URLs.
pub(super) fn unescape_url(url: &str) -> String {
    let mut out = String::new();
    let mut chars = url.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.clone().next() {
                Some(n) if matches!(n, '%' | '#' | '_' | '&' | '{' | '}' | '$' | '~') => {
                    out.push(n);
                    chars.next();
                }
                _ => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}
