//! External links, internal links, bare URLs, and image embeds.

use carta_ast::{Attr, Block, Caption, Inline, QuoteType, Target, ToCompactString};

use super::emphasis::coalesce;
use super::{Parser, at, collect_range, find_char};

impl Parser {
    pub(super) fn external_link(
        &mut self,
        chars: &[char],
        i: usize,
    ) -> Option<(Vec<Inline>, usize)> {
        let close = find_char(chars, i + 1, ']')?;
        let inner = collect_range(chars, i + 1, close);
        let (url, label) = match inner.split_once(|c: char| c.is_whitespace()) {
            Some((u, rest)) => (u.to_string(), rest.trim_start().to_string()),
            None => (inner.clone(), String::new()),
        };
        if !is_url(&url) {
            return None;
        }
        // A labelless bracketed URL running into a letter or digit is no link: the bracket stays literal, the URL continues past `]`.
        if label.is_empty() && at(chars, close + 1).is_some_and(char::is_alphanumeric) {
            return None;
        }
        let text = if label.is_empty() {
            self.link_counter += 1;
            vec![Inline::Str(self.link_counter.to_compact_string())]
        } else {
            self.parse_inlines(&label)
        };
        Some((
            vec![Inline::Link(
                Box::default(),
                text,
                Box::new(Target {
                    url: encode_url_target(&url).into(),
                    title: carta_ast::Text::default(),
                }),
            )],
            close + 1,
        ))
    }

    pub(super) fn internal_link(
        &mut self,
        chars: &[char],
        i: usize,
    ) -> Option<(Vec<Inline>, usize)> {
        // Target ends at the first `|` or `]]`; nesting is untracked, so an inner `]]` can close an unpiped target.
        let start = i + 2;
        let (target_end, has_pipe) = scan_link_target(chars, start)?;
        let target = collect_range(chars, start, target_end).trim().to_string();

        // With a pipe, the label runs to this link's `]]`, stepping over nested `[[ … ]]`.
        let (label_content, close) = if has_pipe {
            let label_start = target_end + 1;
            let close = find_link_close(chars, label_start)?;
            (Some(collect_range(chars, label_start, close)), close)
        } else {
            (None, target_end)
        };

        if let Some(ns) = namespace_of(&target) {
            if ns == "category" {
                let text = match &label_content {
                    Some(label) if !label.trim().is_empty() => self.parse_inlines(label),
                    _ => self.parse_inlines(&target),
                };
                let title = title_text(&text);
                let attr = Attr {
                    id: carta_ast::Text::default(),
                    classes: vec!["wikilink".into()],
                    attributes: Vec::new(),
                };
                self.categories.push(Inline::Link(
                    Box::new(attr),
                    text,
                    Box::new(Target {
                        url: wikilink_url(&target).into(),
                        title: title.into(),
                    }),
                ));
                return Some((Vec::new(), close + 2));
            }
            // A file/image embed may decline (unrepresentable parameter); the markup then falls through to the wikilink path.
            if matches!(ns.as_str(), "file" | "image")
                && !strip_namespace(&target).is_empty()
                && let Some(image) = self.image_embed(&target, label_content.as_deref())
            {
                return Some((vec![image], close + 2));
            }
        }
        let mut after = close + 2;
        let mut trail = String::new();
        while let Some(c) = at(chars, after) {
            if c.is_ascii_alphabetic() {
                trail.push(c);
                after += 1;
            } else {
                break;
            }
        }
        let mut label = match &label_content {
            // An empty label invokes the pipe trick: the display text is derived from the target.
            Some(l) if l.trim().is_empty() => self.pipe_trick_label(&target),
            Some(l) => self.parse_inlines(l),
            None => self.parse_inlines(&target),
        };
        let title = title_text(&label);
        if !trail.is_empty() {
            label.push(Inline::Str(trail.into()));
            label = coalesce(label);
        }
        let attr = Attr {
            id: carta_ast::Text::default(),
            classes: vec!["wikilink".into()],
            attributes: Vec::new(),
        };
        let url = wikilink_url(&target);
        Some((
            vec![Inline::Link(
                Box::new(attr),
                label,
                Box::new(Target {
                    url: url.into(),
                    title: title.into(),
                }),
            )],
            after,
        ))
    }

    /// The display text the pipe trick derives from an empty-label link's target: the part after the
    /// first colon when the target is namespaced (so `Help:Contents` shows as `Contents`), otherwise
    /// no text at all.
    fn pipe_trick_label(&mut self, target: &str) -> Vec<Inline> {
        match target.split_once(':') {
            Some((_, rest)) => self.parse_inlines(rest),
            None => Vec::new(),
        }
    }

    /// Builds the image for a `[[File:…|…]]` / `[[Image:…|…]]` embed. The page name (with the
    /// namespace stripped) is the source; the `WxHpx` parameters set width/height; recognized
    /// placement and option keywords are dropped; the last remaining parameter is the caption,
    /// defaulting to the file name. A lone embed in its own paragraph later becomes a figure
    /// (see [`lone_image_figure`]).
    fn image_embed(&mut self, target: &str, params: Option<&str>) -> Option<Inline> {
        let url = wikilink_url(strip_namespace(target));
        let mut attributes: Vec<(String, String)> = Vec::new();
        let mut caption: Option<String> = None;
        if let Some(params) = params {
            for part in params.split('|') {
                let option = part.trim();
                if image_param_declines(option) {
                    return None;
                }
                if let Some((width, height)) = image_size(option) {
                    attributes.retain(|(key, _)| key != "width" && key != "height");
                    attributes.push(("width".to_string(), width));
                    if let Some(height) = height {
                        attributes.push(("height".to_string(), height));
                    }
                } else if is_image_keyword(option) || is_recognized_image_attr(option) {
                    // Placement/framing keywords and recognized `key=value` carry no caption; unrecognized `key=value` is caption text.
                } else {
                    caption = Some(part.to_string());
                }
            }
        }
        let caption = caption.unwrap_or_else(|| url.clone());
        let alt = self.parse_inlines(&caption);
        let title = title_text(&alt);
        let attr = Attr {
            id: carta_ast::Text::default(),
            classes: Vec::new(),
            attributes: attributes
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        };
        Some(Inline::Image(
            Box::new(attr),
            alt,
            Box::new(Target {
                url: url.into(),
                title: title.into(),
            }),
        ))
    }
}

/// Whether `name` (compared case-insensitively) is a recognized URL scheme. Beyond the shared
/// registry, this format additionally autolinks the `doi` and `javascript` schemes.
pub(super) fn is_scheme(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    crate::url_schemes::is_scheme(&lower) || lower == "doi" || lower == "javascript"
}

/// Whether `text` begins with a recognized scheme followed by a colon: the test a bracketed
/// `[url label]` target must pass to be a link.
fn is_url(text: &str) -> bool {
    match text.split_once(':') {
        Some((scheme, _)) => {
            !scheme.is_empty()
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
                && is_scheme(scheme)
        }
        None => false,
    }
}

/// The length of a `scheme:` prefix at `i` (the scheme name plus its colon) when the name is a
/// recognized scheme, else `None`. The scheme name is the run of letters, digits, `+`, `-`, and `.`
/// before the colon.
fn url_scheme_len(chars: &[char], i: usize) -> Option<usize> {
    let mut j = i;
    let mut name = String::new();
    while let Some(c) = at(chars, j) {
        if c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.') {
            name.push(c);
            j += 1;
        } else {
            break;
        }
    }
    if name.is_empty() || at(chars, j) != Some(':') || !is_scheme(&name) {
        return None;
    }
    Some(j - i + 1)
}

/// Reads a bare URL beginning at a word boundary. The URL runs to the next space or angle bracket,
/// after which trailing punctuation and unbalanced brackets are trimmed back. The displayed text
/// keeps the characters literally while the link target percent-encodes the unsafe ones. Returns the
/// autolink and the index just past the consumed URL.
pub(super) fn bare_url(chars: &[char], i: usize) -> Option<(Inline, usize)> {
    let scheme_len = url_scheme_len(chars, i)?;
    let mut j = i + scheme_len;
    while let Some(c) = at(chars, j) {
        if c.is_whitespace() || matches!(c, '<' | '>') {
            break;
        }
        // A run of two or more apostrophes opens emphasis, so it also ends the URL.
        if c == '\'' && at(chars, j + 1) == Some('\'') {
            break;
        }
        j += 1;
    }
    if j <= i + scheme_len {
        return None;
    }
    let mut display = collect_range(chars, i, j);
    trim_url_trailing(&mut display);
    if display.is_empty() {
        return None;
    }
    let consumed = display.chars().count();
    let target = encode_url_target(&display);
    Some((
        Inline::Link(
            Box::default(),
            vec![Inline::Str(display.into())],
            Box::new(Target {
                url: target.into(),
                title: carta_ast::Text::default(),
            }),
        ),
        i + consumed,
    ))
}

/// Trims a URL's trailing characters that read as sentence punctuation or unbalanced brackets: the
/// always-trimmed set never legitimately ends a URL, and a closing bracket is trimmed only when it
/// outnumbers its opener so a balanced `(a)` or `[a]` survives.
fn trim_url_trailing(url: &mut String) {
    while let Some(last) = url.chars().last() {
        let always = matches!(
            last,
            '.' | ',' | ';' | ':' | '!' | '?' | '"' | '*' | '~' | '\'' | '|'
        );
        let unbalanced = match last {
            ')' => url.matches(')').count() > url.matches('(').count(),
            ']' => url.matches(']').count() > url.matches('[').count(),
            '}' => url.matches('}').count() > url.matches('{').count(),
            _ => false,
        };
        if always || unbalanced {
            url.pop();
        } else {
            break;
        }
    }
}

/// Percent-encodes the characters a wikitext link target escapes, leaving the rest intact.
fn encode_url_target(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for ch in url.chars() {
        match ch {
            ' ' => out.push_str("%20"),
            '"' => out.push_str("%22"),
            '`' => out.push_str("%60"),
            '^' => out.push_str("%5E"),
            '[' => out.push_str("%5B"),
            ']' => out.push_str("%5D"),
            '{' => out.push_str("%7B"),
            '}' => out.push_str("%7D"),
            '|' => out.push_str("%7C"),
            other => out.push(other),
        }
    }
    out
}

/// Builds a wikilink target URL from a page name: each run of whitespace collapses to a single
/// underscore, every other character is kept as written.
fn wikilink_url(target: &str) -> String {
    let mut out = String::new();
    let mut pending = false;
    for ch in target.chars() {
        if ch.is_whitespace() {
            pending = true;
        } else {
            if pending {
                out.push('_');
                pending = false;
            }
            out.push(ch);
        }
    }
    out
}

/// Flatten inline content into the plain string stored as a link or image title. Markup wrappers
/// unwrap to their contents and breaks collapse to a space, as for any plain-text flattening, but a
/// [`Inline::Quoted`] node renders the matching curly quote glyphs around its contents so a curled
/// quotation survives into the title text.
fn title_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    push_title_text(inlines, &mut out);
    out
}

fn push_title_text(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Str(text) | Inline::Code(_, text) | Inline::Math(_, text) => out.push_str(text),
            Inline::Space | Inline::SoftBreak | Inline::LineBreak => out.push(' '),
            Inline::Quoted(QuoteType::SingleQuote, xs) => {
                out.push('\u{2018}');
                push_title_text(xs, out);
                out.push('\u{2019}');
            }
            Inline::Quoted(QuoteType::DoubleQuote, xs) => {
                out.push('\u{201c}');
                push_title_text(xs, out);
                out.push('\u{201d}');
            }
            Inline::Emph(xs)
            | Inline::Underline(xs)
            | Inline::Strong(xs)
            | Inline::Strikeout(xs)
            | Inline::Superscript(xs)
            | Inline::Subscript(xs)
            | Inline::SmallCaps(xs)
            | Inline::Cite(_, xs)
            | Inline::Link(_, xs, _)
            | Inline::Image(_, xs, _)
            | Inline::Span(_, xs) => push_title_text(xs, out),
            Inline::RawInline(..) | Inline::Note(_) => {}
        }
    }
}

fn namespace_of(target: &str) -> Option<String> {
    if target.starts_with(':') {
        return None;
    }
    let (before, _) = target.split_once(':')?;
    Some(before.trim().to_lowercase())
}

/// The page name with a leading `namespace:` prefix removed.
fn strip_namespace(target: &str) -> &str {
    match target.split_once(':') {
        Some((_, rest)) => rest.trim(),
        None => target,
    }
}

/// Parses an image size parameter (`<w>px`, `x<h>px`, or `<w>x<h>px`) into its width and optional
/// height. The width is the digits before an `x` (empty when the form is `x<h>px`); the height is
/// the digits after it. Returns `None` for any parameter that is not a pixel size.
fn image_size(param: &str) -> Option<(String, Option<String>)> {
    let digits = param.strip_suffix("px")?;
    match digits.split_once('x') {
        Some((width, height)) => {
            let valid = width.chars().all(|c| c.is_ascii_digit())
                && !height.is_empty()
                && height.chars().all(|c| c.is_ascii_digit());
            valid.then(|| (width.to_string(), Some(height.to_string())))
        }
        None => (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
            .then(|| (digits.to_string(), None)),
    }
}

/// Whether an image parameter forces the embed to decline, so the markup becomes an ordinary
/// wikilink instead of an image. A `thumbtime` parameter (with or without a value) and an `upright`
/// parameter that carries an explicit value have no image representation; a bare `upright` keyword
/// is a normal sizing hint and does not decline.
fn image_param_declines(param: &str) -> bool {
    match param.split_once('=') {
        Some((key, _)) => {
            let key = key.trim().to_ascii_lowercase();
            key == "thumbtime" || key == "upright"
        }
        None => param.trim().eq_ignore_ascii_case("thumbtime"),
    }
}

/// Whether an image parameter is a recognized `key=value` attribute (`alt`, `link`, `class`,
/// `page`) that is consumed without contributing caption text. Any other `key=value` becomes
/// caption text.
fn is_recognized_image_attr(param: &str) -> bool {
    match param.split_once('=') {
        Some((key, _)) => matches!(
            key.trim().to_ascii_lowercase().as_str(),
            "alt" | "link" | "class" | "page"
        ),
        None => false,
    }
}

/// Whether an image parameter is a recognized placement, framing, or alignment keyword that
/// carries no caption text.
fn is_image_keyword(param: &str) -> bool {
    matches!(
        param.to_ascii_lowercase().as_str(),
        "thumb"
            | "thumbnail"
            | "frame"
            | "framed"
            | "frameless"
            | "border"
            | "left"
            | "right"
            | "center"
            | "centre"
            | "none"
            | "upright"
            | "baseline"
            | "sub"
            | "super"
            | "top"
            | "text-top"
            | "middle"
            | "bottom"
            | "text-bottom"
    )
}

/// Wraps a paragraph whose only content is an image in a figure, moving the image's description to
/// the figure caption; any other paragraph is returned unchanged.
pub(super) fn para_or_figure(inlines: Vec<Inline>) -> Block {
    match lone_image_figure(&inlines) {
        Some(figure) => figure,
        None => Block::Para(inlines),
    }
}

/// As [`para_or_figure`], for a context (a list item) whose tight content is a [`Block::Plain`].
pub(super) fn plain_or_figure(inlines: Vec<Inline>) -> Block {
    match lone_image_figure(&inlines) {
        Some(figure) => figure,
        None => Block::Plain(inlines),
    }
}

/// Builds a figure from a paragraph that holds a single image (ignoring surrounding whitespace),
/// or `None` when the paragraph is anything else.
fn lone_image_figure(inlines: &[Inline]) -> Option<Block> {
    let mut significant = inlines.iter().filter(|inline| {
        !matches!(
            inline,
            Inline::Space | Inline::SoftBreak | Inline::LineBreak
        )
    });
    let Inline::Image(attr, alt, target) = significant.next()? else {
        return None;
    };
    if significant.next().is_some() {
        return None;
    }
    let caption = Caption {
        short: None,
        long: vec![Block::Plain(alt.clone())],
    };
    let image = Inline::Image(attr.clone(), Vec::new(), target.clone());
    Some(Block::Figure(
        Box::default(),
        Box::new(caption),
        vec![Block::Plain(vec![image])],
    ))
}

/// Scans an internal link's target from `start`: it ends at the first `|` or the first `]]`,
/// whichever comes first, with no nesting tracked. Returns the end index and whether a `|` (rather
/// than `]]`) was the delimiter, or `None` if neither appears.
fn scan_link_target(chars: &[char], start: usize) -> Option<(usize, bool)> {
    let mut i = start;
    while let Some(c) = at(chars, i) {
        if c == '|' {
            return Some((i, true));
        }
        if c == ']' && at(chars, i + 1) == Some(']') {
            return Some((i, false));
        }
        i += 1;
    }
    None
}

/// Finds the `]]` that closes an internal link whose label may hold nested `[[ … ]]` links, stepping
/// over each balanced inner pair so only the outer close is returned.
fn find_link_close(chars: &[char], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = start;
    while let Some(c) = at(chars, i) {
        if c == '[' && at(chars, i + 1) == Some('[') {
            depth += 1;
            i += 2;
        } else if c == ']' && at(chars, i + 1) == Some(']') {
            if depth == 0 {
                return Some(i);
            }
            depth -= 1;
            i += 2;
        } else {
            i += 1;
        }
    }
    None
}
