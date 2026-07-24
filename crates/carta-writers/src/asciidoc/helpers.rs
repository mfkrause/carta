//! Free helper functions for the `AsciiDoc` writer: escaping, image and table markup, and small predicates.

use std::fmt::Write as _;

use carta_ast::{
    Alignment, Attr, Block, Cell, ColWidth, Inline, ListAttributes, ListNumberStyle, QuoteType,
    Table, Target, Text,
};

use crate::common::{attribute_value, clean_prefix_len, is_uri_scheme, split_length_unit};

/// The URI scheme of a URL: the lowercase run before the first `:`, when that run is a valid scheme
/// (a letter followed by letters, digits, `+`, `-`, or `.`).
pub(super) fn url_scheme(url: &str) -> Option<&str> {
    let colon = url.find(':')?;
    let scheme = url.get(..colon)?;
    is_uri_scheme(scheme).then_some(scheme)
}

/// Whether a scheme is one the format auto-recognizes as a link, so its URL needs no `link:` prefix.
pub(super) fn is_autolink_scheme(scheme: &str) -> bool {
    ["http", "https", "ftp", "irc", "mailto"]
        .iter()
        .any(|known| scheme.eq_ignore_ascii_case(known))
}

/// The `image:`/`image::` argument list: the alt text (the URL's file stem when no alt is given),
/// the title, and a width/height descriptor.
pub(super) fn image_args(attr: &Attr, target: &Target, alt: &str) -> String {
    let alt = if alt.is_empty() {
        image_stem(&target.url)
    } else {
        alt.to_owned()
    };
    let mut parts = vec![alt];
    if !target.title.is_empty() {
        parts.push(format!("title=\"{}\"", target.title));
    }
    if let Some(size) = image_size(attr) {
        parts.push(size);
    }
    parts.join(",")
}

/// The default alt text for an image with none of its own: the file name of the target URL, minus
/// any directory and extension.
fn image_stem(url: &str) -> String {
    let file = url.rsplit(['/', '\\']).next().unwrap_or(url);
    let stem = file.rsplit_once('.').map_or(file, |(name, _)| name);
    stem.to_owned()
}

/// An image's size descriptor: a percentage width becomes `scaledwidth`, an absolute width or height
/// becomes `width`/`height` with any unit suffix stripped.
fn image_size(attr: &Attr) -> Option<String> {
    if let Some(width) = attribute_value(attr, "width") {
        if let Some(value) = width
            .strip_suffix('%')
            .and_then(|percent| percent.parse::<f64>().ok())
        {
            return Some(format!("scaledwidth={}%", format_decimal(value)));
        }
        return Some(format!("width={}", split_length_unit(width).0));
    }
    if let Some(height) = attribute_value(attr, "height") {
        return Some(format!("height={}", split_length_unit(height).0));
    }
    None
}

/// Format a number the way the format's scaled-width values appear: an integer keeps a single trailing
/// zero (`50` -> `50.0`), a fractional value keeps its digits (`50.5`).
fn format_decimal(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}

/// The bracketed role for a span: its id (`#id`) and classes (`.class`), space-joined. `None` when
/// the span carries neither.
pub(super) fn span_role(attr: &Attr) -> Option<String> {
    let mut parts = Vec::new();
    if !attr.id.is_empty() {
        parts.push(format!("#{}", attr.id));
    }
    for class in &attr.classes {
        parts.push(format!(".{class}"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

/// The admonition label for a div whose classes name one, uppercased.
pub(super) fn admonition(attr: &Attr) -> Option<&'static str> {
    attr.classes.iter().find_map(|class| match class.as_str() {
        "note" => Some("NOTE"),
        "tip" => Some("TIP"),
        "important" => Some("IMPORTANT"),
        "caution" => Some("CAUTION"),
        "warning" => Some("WARNING"),
        _ => None,
    })
}

/// The opening character for an emphasis/strong construct must be the unconstrained (doubled) form
/// when the preceding inline closes the left word boundary: a string ending in an alphanumeric, or
/// any non-space formatted inline.
pub(super) fn closes_left_boundary(before: Option<&Inline>) -> bool {
    match before {
        None | Some(Inline::Space | Inline::SoftBreak | Inline::LineBreak) => false,
        Some(Inline::Str(text)) => text.chars().last().is_some_and(char::is_alphanumeric),
        Some(_) => true,
    }
}

pub(super) fn closes_right_boundary(after: Option<&Inline>) -> bool {
    match after {
        None | Some(Inline::Space | Inline::SoftBreak | Inline::LineBreak) => false,
        Some(Inline::Str(text)) => text.chars().next().is_some_and(char::is_alphanumeric),
        Some(_) => true,
    }
}

/// The smart-quote glyphs wrapped in the format's typographic-quote backticks.
pub(super) fn quote_glyphs(kind: &QuoteType) -> (String, String) {
    match kind {
        QuoteType::SingleQuote => ("'`".to_owned(), "`'".to_owned()),
        QuoteType::DoubleQuote => ("\"`".to_owned(), "`\"".to_owned()),
    }
}

/// The attribute line preceding an ordered list's items, naming its numeral style and (when not one)
/// its start number. An example-numbered list takes no attribute line.
pub(super) fn ordered_style_line(attrs: &ListAttributes) -> Option<String> {
    let style = match attrs.style {
        ListNumberStyle::Example => return None,
        ListNumberStyle::DefaultStyle | ListNumberStyle::Decimal => "arabic",
        ListNumberStyle::LowerAlpha => "loweralpha",
        ListNumberStyle::UpperAlpha => "upperalpha",
        ListNumberStyle::LowerRoman => "lowerroman",
        ListNumberStyle::UpperRoman => "upperroman",
    };
    if attrs.start == 1 {
        Some(format!("[{style}]"))
    } else {
        Some(format!("[{style}, start={}]", attrs.start))
    }
}

/// The `[…]` options line introducing a table: an overall width percentage, per-column widths, and
/// a `header` option when the table has a header row.
pub(super) fn table_options_line(table: &Table, has_header: bool, has_footer: bool) -> String {
    let widths: Vec<f64> = table
        .col_specs
        .iter()
        .map(|spec| match spec.width {
            ColWidth::ColWidth(value) => value,
            ColWidth::ColWidthDefault => 0.0,
        })
        .collect();
    let total: f64 = widths.iter().sum();
    let mut attrs = Vec::new();
    if total > 0.0 {
        attrs.push(format!("width=\"{}%\"", percent_truncated(total)));
    }
    let mut percents: Vec<Option<f64>> = widths
        .iter()
        .map(|width| {
            if total > 0.0 && *width > 0.0 {
                Some((width / total * 100.0).floor().max(0.0))
            } else {
                None
            }
        })
        .collect();
    // Truncated percentages fall short of 100; the first sized column absorbs the shortfall.
    let assigned: f64 = percents.iter().flatten().sum();
    if assigned > 0.0
        && let Some(first) = percents.iter_mut().flatten().next()
    {
        *first += 100.0 - assigned;
    }
    let cols: Vec<String> = table
        .col_specs
        .iter()
        .zip(&percents)
        .map(|(spec, percent)| {
            let operator = alignment_operator(&spec.align)
                .map(|op| op.to_string())
                .unwrap_or_default();
            match percent {
                Some(percent) => format!("{operator}{percent:.0}%"),
                None => operator,
            }
        })
        .collect();
    attrs.push(format!("cols=\"{}\"", cols.join(",")));
    let mut options = Vec::new();
    if has_header {
        options.push("header");
    }
    if has_footer {
        options.push("footer");
    }
    if !options.is_empty() {
        attrs.push(format!("options=\"{}\"", options.join(",")));
    }
    format!("[{},]", attrs.join(","))
}

/// Detect a leading task-list checkbox on a list item's first block. Returns the literal marker
/// (`[ ]` or `[x]`) and the remaining inlines with the checkbox glyph and its trailing space
/// removed.
pub(super) fn task_checkbox(block: &Block) -> Option<(&'static str, Vec<Inline>)> {
    let (Block::Plain(inlines) | Block::Para(inlines)) = block else {
        return None;
    };
    let marker = match inlines.first() {
        Some(Inline::Str(glyph)) if glyph == "\u{2610}" => "[ ]",
        Some(Inline::Str(glyph)) if glyph == "\u{2612}" => "[x]",
        _ => return None,
    };
    match inlines.get(1) {
        Some(Inline::Space) => Some((marker, inlines.get(2..).unwrap_or(&[]).to_vec())),
        _ => None,
    }
}

/// A display equation rendered as its own delimited block.
pub(super) fn display_math_block(math: &str) -> String {
    format!("[latexmath]\n++++\n{math}\n++++")
}

/// Drop leading and trailing whitespace inlines from a run, since a block boundary already supplies
/// the separation.
pub(super) fn trim_surrounding_space(inlines: &[Inline]) -> &[Inline] {
    let is_space = |inline: &Inline| matches!(inline, Inline::Space | Inline::SoftBreak);
    let start = inlines.iter().position(|inline| !is_space(inline));
    match start {
        Some(start) => {
            let end = inlines
                .iter()
                .rposition(|inline| !is_space(inline))
                .unwrap_or(start);
            inlines.get(start..=end).unwrap_or(&[])
        }
        None => &[],
    }
}

/// A fraction in `0.0..=1.0` as a whole-number percentage string, truncated toward zero.
fn percent_truncated(fraction: f64) -> String {
    format!("{:.0}", (fraction * 100.0).floor().max(0.0))
}

/// The span prefix for a table cell: a colspan, a rowspan (`.n`), or both (`c.r`). Empty when the
/// cell spans a single column and row.
pub(super) fn cell_span(cell: &Cell) -> String {
    let col = (cell.col_span > 1).then(|| cell.col_span.to_string());
    let row = (cell.row_span > 1).then(|| format!(".{}", cell.row_span));
    match (col, row) {
        (Some(c), Some(r)) => format!("{c}{r}"),
        (Some(c), None) => c,
        (None, Some(r)) => r,
        (None, None) => String::new(),
    }
}

pub(super) fn alignment_operator(align: &Alignment) -> Option<char> {
    match align {
        Alignment::AlignLeft => Some('<'),
        Alignment::AlignCenter => Some('^'),
        Alignment::AlignRight => Some('>'),
        Alignment::AlignDefault => None,
    }
}

/// Whether a cell's content is a single text block, so it renders on the marker line rather than as
/// a block cell.
pub(super) fn is_simple_cell(blocks: &[Block]) -> bool {
    matches!(blocks, [] | [Block::Plain(_) | Block::Para(_)])
}

/// Render a code block: a `[source,…]` delimited block when the block carries classes (with a
/// `%linesnum` flag for `numberLines`), otherwise a literal `....` block.
pub(super) fn code_block(attr: &Attr, text: &str) -> String {
    let body = text.strip_suffix('\n').unwrap_or(text);
    if attr.classes.is_empty() {
        return format!("....\n{body}\n....");
    }
    let numbered = attr.classes.iter().any(|class| class == "numberLines");
    let languages: Vec<&str> = attr
        .classes
        .iter()
        .filter(|class| class.as_str() != "numberLines")
        .map(Text::as_str)
        .collect();
    let mut header = String::from("[source");
    if numbered {
        header.push_str("%linesnum");
    }
    if !languages.is_empty() {
        let _ = write!(header, ",{}", languages.join(","));
    }
    header.push(']');
    format!("{header}\n----\n{body}\n----")
}

/// Escape a run of plain text. A maximal run of formatting characters is wrapped together in a
/// single passthrough span (`++…++`); `+` is always replaced by its attribute reference since it
/// would otherwise begin a passthrough span itself.
pub(super) fn escape_text(text: &str) -> String {
    let mut out = String::new();
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if !run.is_empty() {
            let _ = write!(out, "++{run}++");
            run.clear();
        }
    };
    let is_trigger = |byte: u8| {
        matches!(
            byte,
            b'*' | b'_' | b'`' | b'#' | b'<' | b'>' | b'{' | b'[' | b']' | b'|' | b'\\' | b'+'
        )
    };
    let mut rest = text;
    loop {
        let clean = clean_prefix_len(rest, is_trigger);
        if clean > 0 {
            flush(&mut run, &mut out);
            let Some((head, tail)) = rest.split_at_checked(clean) else {
                out.push_str(rest);
                break;
            };
            out.push_str(head);
            rest = tail;
            continue;
        }
        let mut chars = rest.chars();
        let Some(ch) = chars.next() else { break };
        if is_formatting_char(ch) {
            run.push(ch);
        } else {
            flush(&mut run, &mut out);
            if ch == '+' {
                out.push_str("{plus}");
            } else {
                out.push(ch);
            }
        }
        rest = chars.as_str();
    }
    flush(&mut run, &mut out);
    out
}

/// Whether a character begins or participates in inline formatting and so must be passed through
/// literally. `}` is left alone: it is significant only paired with `{`, which is itself escaped.
fn is_formatting_char(ch: char) -> bool {
    matches!(
        ch,
        '*' | '_' | '`' | '#' | '<' | '>' | '{' | '[' | ']' | '|' | '\\'
    )
}

/// Indent every non-empty line of a body by a fixed prefix; blank separator lines stay empty.
pub(super) fn indent(body: &str, prefix: &str) -> String {
    let mut out = String::new();
    for (index, line) in body.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if line.is_empty() {
            continue;
        }
        out.push_str(prefix);
        out.push_str(line);
    }
    out
}
