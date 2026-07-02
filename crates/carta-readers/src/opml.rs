//! Outline reader: parses a nested outline of `<outline>` elements into the document model.
//!
//! Each outline becomes a header whose level is its nesting depth (a top-level outline is level 1,
//! its child level 2, and so on). The header inlines come from the outline's `text` attribute,
//! parsed as a fragment of HTML inline markup (so `<strong>`, `<em>`, `<code>`, links, and the like
//! become their inline constructs); the outline's `_note` attribute is parsed as markdown blocks. An
//! outline of `type="link"` wraps its heading content in a link to its `url`. The document metadata
//! is drawn from the document head: `title`, `ownerName` (as the author list), and `dateModified`
//! (as the date), each taken as plain text.
//!
//! XML is parsed by a small hand-written scanner over the subset the format uses — elements,
//! attributes with entity decoding, self-closing tags, and nesting. The scanner is panic-free on
//! malformed input: unrecognized or unbalanced markup is skipped rather than rejected.

use std::collections::BTreeMap;

use carta_ast::{Block, Document, Inline, MetaValue, QuoteType, Target};
use carta_core::{Reader, ReaderOptions, Result, presets};

use crate::commonmark::CommonmarkReader;
use crate::html::parse_inline_fragment;

/// Parses an outline document into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpmlReader;

impl Reader for OpmlReader {
    fn read(&self, input: &str, _options: &ReaderOptions) -> Result<Document> {
        let nodes = parse_nodes(input);
        let mut blocks = Vec::new();
        let head = find_child(&nodes, "head");
        let body = find_child(&nodes, "body");
        for node in body.map(element_children).unwrap_or_default() {
            emit_outline(node, 1, &mut blocks)?;
        }
        Ok(Document {
            api_version: carta_ast::ApiVersion::default(),
            meta: build_meta(head),
            blocks,
        })
    }
}

/// A parsed XML element with its decoded attributes and its element children. Text nodes are not
/// retained: the format carries its content in attributes.
#[derive(Debug)]
struct Element {
    name: String,
    attributes: BTreeMap<String, String>,
    children: Vec<Element>,
}

fn element_children(element: &Element) -> Vec<&Element> {
    element.children.iter().collect()
}

/// The first descendant search is shallow by design: `head` and `body` are direct children of the
/// document root, found among the top-level parse and the root `opml` element's children.
fn find_child<'a>(nodes: &'a [Element], name: &str) -> Option<&'a Element> {
    for node in nodes {
        if node.name == name {
            return Some(node);
        }
        if let Some(found) = node.children.iter().find(|child| child.name == name) {
            return Some(found);
        }
    }
    None
}

fn emit_outline(outline: &Element, level: i32, blocks: &mut Vec<Block>) -> Result<()> {
    if outline.name != "outline" {
        return Ok(());
    }
    let heading = outline
        .attributes
        .get("text")
        .map(|text| smart_inlines(parse_inline_fragment(text)))
        .unwrap_or_default();
    let heading = if is_link_outline(outline) {
        let url = outline.attributes.get("url").cloned().unwrap_or_default();
        vec![Inline::Link(
            Box::default(),
            heading,
            Box::new(Target {
                url,
                title: String::new(),
            }),
        )]
    } else {
        heading
    };
    blocks.push(Block::Header(level, Box::default(), heading));
    if let Some(note) = outline.attributes.get("_note") {
        let parsed = CommonmarkReader.read(note, &note_options())?;
        blocks.extend(parsed.blocks);
    }
    for child in &outline.children {
        emit_outline(child, level + 1, blocks)?;
    }
    Ok(())
}

/// An outline of `type="link"` (case-insensitive) names a hyperlink: its heading content is wrapped
/// in a link to the outline's `url`, which may be absent (an empty target).
fn is_link_outline(outline: &Element) -> bool {
    outline
        .attributes
        .get("type")
        .is_some_and(|kind| kind.eq_ignore_ascii_case("link"))
}

/// Reader options for a `_note` body: the extended Markdown dialect's full extension set (so smart
/// typography, definition lists, and the other Markdown-flavored constructs are on) with greedy
/// paragraphs, so a bare following line continues the paragraph rather than opening a new block.
fn note_options() -> ReaderOptions {
    let mut options = ReaderOptions::default();
    options.extensions = presets::MARKDOWN;
    options.greedy_paragraphs = true;
    options
}

fn build_meta(head: Option<&Element>) -> BTreeMap<String, MetaValue> {
    let mut meta = BTreeMap::new();
    // The element's text content, or `None` when the element is absent. A present element with empty
    // or whitespace-only content is distinguished from an absent one, which matters for the author
    // list: a present `ownerName` always contributes an entry, even an empty one.
    let value = |name: &str| -> Option<&str> {
        head.and_then(|head| head.children.iter().find(|child| child.name == name))
            .map(|element| {
                element
                    .attributes
                    .get("__text")
                    .map(String::as_str)
                    .unwrap_or_default()
            })
    };
    let title = tokenize_meta(value("title").unwrap_or_default());
    let date = tokenize_meta(value("dateModified").unwrap_or_default());
    let author = match value("ownerName") {
        Some(owner) => vec![MetaValue::MetaInlines(tokenize_meta(owner))],
        None => Vec::new(),
    };
    meta.insert("title".to_owned(), MetaValue::MetaInlines(title));
    meta.insert("author".to_owned(), MetaValue::MetaList(author));
    meta.insert("date".to_owned(), MetaValue::MetaInlines(date));
    meta
}

/// Tokenize a metadata value into inlines, preserving boundary whitespace. Each maximal
/// non-whitespace run becomes one `Str`; each maximal whitespace run becomes a single break — a
/// `SoftBreak` when the run spans a line ending, otherwise a `Space`. Leading and trailing
/// whitespace is kept, unlike inline body text where it is trimmed. Smart typography is not applied:
/// metadata values keep their straight quotes, hyphens, and dots verbatim.
fn tokenize_meta(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    let mut word = String::new();
    while let Some(ch) = chars.next() {
        if ch.is_whitespace() {
            if !word.is_empty() {
                out.push(Inline::Str(std::mem::take(&mut word)));
            }
            let mut has_newline = ch == '\n' || ch == '\r';
            while let Some(&next) = chars.peek() {
                if !next.is_whitespace() {
                    break;
                }
                has_newline |= next == '\n' || next == '\r';
                chars.next();
            }
            out.push(if has_newline {
                Inline::SoftBreak
            } else {
                Inline::Space
            });
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        out.push(Inline::Str(word));
    }
    out
}

/// Parse the top-level elements of a document. Anything outside an element (prolog, stray text) is
/// skipped.
fn parse_nodes(input: &str) -> Vec<Element> {
    let chars: Vec<char> = input.chars().collect();
    let mut pos = 0;
    let mut nodes = Vec::new();
    while let Some(element) = next_element(&chars, &mut pos) {
        nodes.push(element);
    }
    nodes
}

/// Scan the next element starting at or after `pos`. Returns `None` at end of input. Comments,
/// processing instructions, declarations, and DOCTYPE are skipped; text between elements is
/// captured into the parent via [`parse_children`].
fn next_element(chars: &[char], pos: &mut usize) -> Option<Element> {
    loop {
        skip_to_tag(chars, pos);
        if *pos >= chars.len() {
            return None;
        }
        if skip_non_element(chars, pos) {
            continue;
        }
        return parse_element(chars, pos);
    }
}

/// Skip past characters until the next `<`.
fn skip_to_tag(chars: &[char], pos: &mut usize) {
    while let Some(&ch) = chars.get(*pos) {
        if ch == '<' {
            return;
        }
        *pos += 1;
    }
}

/// If the tag at `pos` is a comment, processing instruction, declaration, or DOCTYPE, skip it and
/// return `true`. A closing tag is also consumed here so a caller scanning siblings stops.
fn skip_non_element(chars: &[char], pos: &mut usize) -> bool {
    if starts_with(chars, *pos, "<!--") {
        skip_until(chars, pos, "-->");
        return true;
    }
    if starts_with(chars, *pos, "<?") {
        skip_until(chars, pos, "?>");
        return true;
    }
    if starts_with(chars, *pos, "<!") {
        skip_until(chars, pos, ">");
        return true;
    }
    false
}

/// Parse one element whose `<` is at `pos`, including its children up to the matching close tag.
fn parse_element(chars: &[char], pos: &mut usize) -> Option<Element> {
    if chars.get(*pos) != Some(&'<') {
        return None;
    }
    *pos += 1;
    let name = read_name(chars, pos);
    if name.is_empty() {
        skip_until(chars, pos, ">");
        return None;
    }
    let mut attributes = BTreeMap::new();
    loop {
        skip_whitespace(chars, pos);
        match chars.get(*pos) {
            None => {
                return Some(Element {
                    name,
                    attributes,
                    children: Vec::new(),
                });
            }
            Some('/') => {
                *pos += 1;
                skip_until(chars, pos, ">");
                return Some(Element {
                    name,
                    attributes,
                    children: Vec::new(),
                });
            }
            Some('>') => {
                *pos += 1;
                break;
            }
            Some(_) => {
                if let Some((key, value)) = read_attribute(chars, pos) {
                    attributes.insert(key, value);
                } else {
                    *pos += 1;
                }
            }
        }
    }
    let (children, text) = parse_children(chars, pos);
    if !text.is_empty() {
        attributes.insert("__text".to_owned(), text);
    }
    Some(Element {
        name,
        attributes,
        children,
    })
}

/// Parse the content of an open element up to its matching `</name>`: nested elements become
/// children, and the concatenated raw text (entity-decoded) is returned for leaf elements.
fn parse_children(chars: &[char], pos: &mut usize) -> (Vec<Element>, String) {
    let mut children = Vec::new();
    let mut text = String::new();
    loop {
        let mut run = String::new();
        while let Some(&ch) = chars.get(*pos) {
            if ch == '<' {
                break;
            }
            run.push(ch);
            *pos += 1;
        }
        text.push_str(&decode_entities(&run));
        if *pos >= chars.len() {
            break;
        }
        if starts_with(chars, *pos, "</") {
            *pos += 2;
            let _ = read_name(chars, pos);
            skip_until(chars, pos, ">");
            break;
        }
        if skip_non_element(chars, pos) {
            continue;
        }
        if let Some(child) = parse_element(chars, pos) {
            children.push(child);
        } else {
            skip_to_tag(chars, pos);
            *pos = (*pos).saturating_add(1);
        }
    }
    // The raw text is returned untrimmed: a metadata value's boundary whitespace is significant and
    // is turned into boundary `Space`/`SoftBreak` inlines by [`tokenize_meta`].
    (children, text)
}

fn read_name(chars: &[char], pos: &mut usize) -> String {
    let mut name = String::new();
    while let Some(&ch) = chars.get(*pos) {
        if ch.is_whitespace() || ch == '>' || ch == '/' {
            break;
        }
        name.push(ch);
        *pos += 1;
    }
    name
}

/// Read one `key="value"` (or single-quoted) attribute. Returns `None` when the cursor is not at a
/// name character.
fn read_attribute(chars: &[char], pos: &mut usize) -> Option<(String, String)> {
    let key = read_attr_name(chars, pos);
    if key.is_empty() {
        return None;
    }
    skip_whitespace(chars, pos);
    if chars.get(*pos) != Some(&'=') {
        return Some((key, String::new()));
    }
    *pos += 1;
    skip_whitespace(chars, pos);
    let Some(&quote @ ('"' | '\'')) = chars.get(*pos) else {
        return Some((key, String::new()));
    };
    *pos += 1;
    let mut raw = String::new();
    while let Some(&ch) = chars.get(*pos) {
        if ch == quote {
            *pos += 1;
            break;
        }
        raw.push(ch);
        *pos += 1;
    }
    Some((key, decode_entities(&raw)))
}

fn read_attr_name(chars: &[char], pos: &mut usize) -> String {
    let mut name = String::new();
    while let Some(&ch) = chars.get(*pos) {
        if ch.is_whitespace() || ch == '=' || ch == '>' || ch == '/' {
            break;
        }
        name.push(ch);
        *pos += 1;
    }
    name
}

fn skip_whitespace(chars: &[char], pos: &mut usize) {
    while let Some(&ch) = chars.get(*pos) {
        if !ch.is_whitespace() {
            return;
        }
        *pos += 1;
    }
}

fn starts_with(chars: &[char], pos: usize, prefix: &str) -> bool {
    prefix
        .chars()
        .enumerate()
        .all(|(offset, expected)| chars.get(pos + offset) == Some(&expected))
}

/// Advance the cursor past the next occurrence of `marker`, consuming the marker. If the marker is
/// absent the cursor moves to the end of input.
fn skip_until(chars: &[char], pos: &mut usize, marker: &str) {
    let marker_len = marker.chars().count();
    while *pos < chars.len() {
        if starts_with(chars, *pos, marker) {
            *pos += marker_len;
            return;
        }
        *pos += 1;
    }
}

/// Decode the XML entity references the format uses: the five named entities and numeric character
/// references in decimal and hexadecimal. An unrecognized or malformed reference is left verbatim.
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut pos = 0;
    while let Some(&ch) = chars.get(pos) {
        if ch != '&' {
            out.push(ch);
            pos += 1;
            continue;
        }
        let Some(end) = (pos + 1..chars.len()).find(|&index| chars.get(index) == Some(&';')) else {
            out.push('&');
            pos += 1;
            continue;
        };
        let body: String = chars.get(pos + 1..end).unwrap_or_default().iter().collect();
        if let Some(decoded) = decode_reference(&body) {
            out.push_str(&decoded);
            pos = end + 1;
        } else {
            out.push('&');
            pos += 1;
        }
    }
    out
}

fn decode_reference(body: &str) -> Option<String> {
    match body {
        "amp" => Some("&".to_owned()),
        "lt" => Some("<".to_owned()),
        "gt" => Some(">".to_owned()),
        "quot" => Some("\"".to_owned()),
        "apos" => Some("'".to_owned()),
        _ => {
            let code =
                if let Some(hex) = body.strip_prefix("#x").or_else(|| body.strip_prefix("#X")) {
                    u32::from_str_radix(hex, 16).ok()?
                } else if let Some(dec) = body.strip_prefix('#') {
                    dec.parse().ok()?
                } else {
                    return None;
                };
            char::from_u32(code).map(|ch| ch.to_string())
        }
    }
}

/// Apply smart typography to an inline tree: straight double and single quotes become curly quotes
/// (paired into `Quoted` spans where they enclose a run, otherwise directional glyphs); runs of
/// hyphens fold to en/em dashes; runs of three dots fold to an ellipsis. Container inlines are
/// transformed recursively; the content of a code span is transformed textually (its quotes become
/// directional glyphs rather than `Quoted` spans). Quote pairing does not cross a non-text inline:
/// such an inline is a hard boundary for the pairing search.
fn smart_inlines(inlines: Vec<Inline>) -> Vec<Inline> {
    let folded = inlines.into_iter().map(fold_inline).collect();
    pair_quotes(folded)
}

/// Recurse into one inline applying the textual smart transforms (dashes, dots, and — in code and
/// string contexts — directional quote glyphs). Quote *pairing* into `Quoted` spans is left to
/// [`pair_quotes`], which sees the whole run.
fn fold_inline(inline: Inline) -> Inline {
    match inline {
        Inline::Str(text) => Inline::Str(fold_text(&text)),
        Inline::Code(attr, text) => Inline::Code(attr, smart_code(&text)),
        Inline::Emph(children) => Inline::Emph(smart_inlines(children)),
        Inline::Underline(children) => Inline::Underline(smart_inlines(children)),
        Inline::Strong(children) => Inline::Strong(smart_inlines(children)),
        Inline::Strikeout(children) => Inline::Strikeout(smart_inlines(children)),
        Inline::Superscript(children) => Inline::Superscript(smart_inlines(children)),
        Inline::Subscript(children) => Inline::Subscript(smart_inlines(children)),
        Inline::SmallCaps(children) => Inline::SmallCaps(smart_inlines(children)),
        Inline::Quoted(kind, children) => Inline::Quoted(kind, smart_inlines(children)),
        Inline::Span(attr, children) => Inline::Span(attr, smart_inlines(children)),
        Inline::Link(attr, children, target) => Inline::Link(attr, smart_inlines(children), target),
        Inline::Image(attr, children, target) => {
            Inline::Image(attr, smart_inlines(children), target)
        }
        other => other,
    }
}

/// Fold the dash and dot runs of a plain text string: `---` and longer fold to em/en dashes, `...`
/// folds to an ellipsis. Straight quotes are left untouched here — they are resolved by
/// [`pair_quotes`], which can see their surrounding context across the whole run.
fn fold_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '-' => {
                let mut len = 1;
                while chars.peek() == Some(&'-') {
                    chars.next();
                    len += 1;
                }
                out.push_str(&fold_dash_run(len));
            }
            '.' => {
                let mut len = 1;
                while chars.peek() == Some(&'.') {
                    chars.next();
                    len += 1;
                }
                out.push_str(&fold_ellipsis_run(len));
            }
            other => out.push(other),
        }
    }
    out
}

/// Smart-transform the verbatim content of a code span: fold dash and dot runs, and curl its
/// quotes. A code span holds only a string, so a matched quote pair renders as its two directional
/// glyphs (left then right) rather than a `Quoted` node; an unmatched quote becomes a directional
/// glyph. The same opener/closer pairing drives both, so `'q'` curls to `‘q’` and a lone leading
/// `'open` to `’open`.
fn smart_code(text: &str) -> String {
    let folded = fold_text(text);
    let mut run: Vec<RunTok> = Vec::new();
    for ch in folded.chars() {
        if ch == '\'' || ch == '"' {
            run.push(RunTok::Quote(ch));
        } else {
            run.push(RunTok::Char(ch));
        }
    }
    let mut items = classify_run(&run);
    match_quotes(&mut items);
    let mut out = String::with_capacity(folded.len());
    for (index, item) in items.iter().enumerate() {
        match item {
            Item::Text(text) => out.push_str(text),
            Item::Break(_) => {}
            Item::Quote(quote) => out.push(match quote.partner {
                // The opener of a matched pair turns to the left glyph, its closer to the right
                // glyph; an unmatched quote keeps its directional fallback.
                Some(partner) if partner > index => paired_code_glyph(quote.ch, true),
                Some(_) => paired_code_glyph(quote.ch, false),
                None => quote.glyph,
            }),
        }
    }
    out
}

/// Fold a run of `len` hyphens into em (`—`) and en (`–`) dashes, greedily preferring em dashes:
/// the run is built from as many em dashes as fit, then the remainder closes it. A remainder of
/// two is one en dash, a remainder of one is a single literal hyphen, and a remainder of zero
/// leaves the em dashes alone. So `--` is one en dash, `---` one em dash, `----` an em dash plus a
/// hyphen, and `-----` an em dash plus an en dash.
fn fold_dash_run(len: usize) -> String {
    let (em, remainder) = match len % 3 {
        // A remainder of one borrows nothing: the run is `len / 3` em dashes then a lone hyphen.
        1 => (len / 3, "-"),
        2 => (len / 3, "\u{2013}"),
        _ => (len / 3, ""),
    };
    let mut out = String::with_capacity(em * 3 + remainder.len());
    out.extend(std::iter::repeat_n('\u{2014}', em));
    out.push_str(remainder);
    out
}

/// Fold a run of `len` dots into one ellipsis (`…`) per group of three, leaving any trailing one or
/// two dots literal.
fn fold_ellipsis_run(len: usize) -> String {
    let mut out = String::with_capacity(len);
    out.extend(std::iter::repeat_n('\u{2026}', len / 3));
    out.extend(std::iter::repeat_n('.', len % 3));
    out
}

/// One position in a flattened text run: an ordinary character, a quote delimiter, or a break
/// (a space or a soft/hard line break, which the inline tree carries as its own node).
enum RunTok {
    Char(char),
    Quote(char),
    Break(Inline),
}

/// Resolve straight quotes across the inline sequence. Within each maximal run of text inlines
/// (`Str` plus break nodes), pair a quote opener with a later closer of the same kind into a
/// `Quoted` span; any quote that does not pair becomes a directional glyph. A non-text inline ends
/// the current run and is itself a word-like boundary for the flanking of adjacent quotes.
fn pair_quotes(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut run: Vec<RunTok> = Vec::new();
    for inline in inlines {
        match inline {
            Inline::Str(text) => {
                for ch in text.chars() {
                    if ch == '\'' || ch == '"' {
                        run.push(RunTok::Quote(ch));
                    } else {
                        run.push(RunTok::Char(ch));
                    }
                }
            }
            brk @ (Inline::Space | Inline::SoftBreak | Inline::LineBreak) => {
                run.push(RunTok::Break(brk));
            }
            barrier => {
                out.extend(resolve_run(&std::mem::take(&mut run)));
                out.push(barrier);
            }
        }
    }
    out.extend(resolve_run(&run));
    out
}

/// Whether the character before a quote permits it to open a span: the start of the run, whitespace,
/// or one of a small set of leading characters (a dash glyph, a dot, a backslash, a currency sign,
/// an ellipsis, or an already-curled quote). A quote glued to a letter, a digit, or an opening
/// bracket does not satisfy this — there it reads as a closer or apostrophe instead.
fn open_context(before: Option<char>) -> bool {
    match before {
        None => true,
        Some(ch) => {
            ch.is_whitespace()
                || matches!(
                    ch,
                    '"' | '\''
                        | '$'
                        | '-'
                        | '.'
                        | '\\'
                        | '\u{2013}'
                        | '\u{2014}'
                        | '\u{2018}'
                        | '\u{2019}'
                        | '\u{201c}'
                        | '\u{201d}'
                        | '\u{2026}'
                )
        }
    }
}

/// Whether a quote opens a span here: its preceding character permits opening and a non-whitespace
/// character follows it. A quote followed by whitespace (or the run's end) cannot open — it reads as
/// a closing glyph or apostrophe.
fn opens_quote(before: Option<char>, after: Option<char>) -> bool {
    open_context(before) && after.is_some_and(|next| !next.is_whitespace())
}

/// Whether a quote at this position may end a quoted span. A double quote always closes an open
/// double quote. A single quote closes only when it is not glued to a following alphanumeric — that
/// case is an apostrophe inside or after a word (`it's`, `dogs'`), not a closing quote.
fn can_close_quote(ch: char, after: Option<char>) -> bool {
    if ch == '"' {
        return true;
    }
    !after.is_some_and(char::is_alphanumeric)
}

/// The directional glyph an unpaired straight quote becomes. A single quote always becomes the right
/// single glyph (`’`), which doubles as the apostrophe. A double quote becomes the left glyph (`“`)
/// only where it reads as an opener — its preceding character permits opening (start of run,
/// whitespace, a dash, or one of the other leading characters) and a non-space character follows it;
/// otherwise it becomes the right glyph (`”`).
fn directional_quote(ch: char, before: Option<char>, after: Option<char>) -> char {
    if ch == '\'' {
        return '\u{2019}';
    }
    if opens_quote(before, after) {
        '\u{201c}'
    } else {
        '\u{201d}'
    }
}

/// The directional glyph a paired quote contributes inside a code span, where a pair is rendered as
/// its two directional glyphs rather than a `Quoted` node: the left glyph (`‘`/`“`) on open, the
/// right glyph (`’`/`”`) on close.
fn paired_code_glyph(ch: char, open: bool) -> char {
    match (ch, open) {
        ('\'', true) => '\u{2018}',
        ('\'', false) => '\u{2019}',
        (_, true) => '\u{201c}',
        (_, false) => '\u{201d}',
    }
}

/// One position in the run after quote classification: settled text, a break node, or a quote with
/// the context flags that decide whether it may open or close a span and the glyph it falls back to.
enum Item {
    Text(String),
    Break(Inline),
    Quote(QuoteItem),
}

/// A classified quote delimiter: its kind, whether its surrounding characters let it open or close a
/// span, the glyph it becomes when it stays unmatched, and (once matching runs) the index of the
/// partner it pairs with.
struct QuoteItem {
    ch: char,
    can_open: bool,
    can_close: bool,
    glyph: char,
    partner: Option<usize>,
}

/// Resolve a single flattened text run into inlines by pairing its quotes. A first pass classifies
/// each quote by its context; a second matches openers to closers; the matched pairs become
/// `Quoted` spans and every unmatched quote becomes its directional glyph.
fn resolve_run(run: &[RunTok]) -> Vec<Inline> {
    let mut items = classify_run(run);
    match_quotes(&mut items);
    render_items(&items, &mut 0)
}

/// Classify the run into [`Item`]s: consecutive characters coalesce into one text item, breaks pass
/// through, and each quote records whether its context lets it open or close and the glyph it falls
/// back to.
fn classify_run(run: &[RunTok]) -> Vec<Item> {
    let context = run_context(run);
    let mut items = Vec::new();
    for (index, tok) in run.iter().enumerate() {
        match tok {
            RunTok::Char(ch) => match items.last_mut() {
                Some(Item::Text(text)) => text.push(*ch),
                _ => items.push(Item::Text(ch.to_string())),
            },
            RunTok::Break(brk) => items.push(Item::Break(brk.clone())),
            RunTok::Quote(ch) => {
                let (before, after) = context.get(index).copied().unwrap_or((None, None));
                items.push(Item::Quote(QuoteItem {
                    ch: *ch,
                    can_open: opens_quote(before, after),
                    can_close: can_close_quote(*ch, after),
                    glyph: directional_quote(*ch, before, after),
                    partner: None,
                }));
            }
        }
    }
    items
}

/// Match quote openers to closers across the classified run with a stack of still-open quotes.
/// Scanning left to right, a quote of a kind already open closes that span (recorded as a mutual
/// partner link), abandoning any inner openers of the other kind that never closed — so a span does
/// not straddle a closed inner span. A quote with no open partner of its kind opens a new span where
/// its context permits; quotes of one kind do not nest within their own kind. A single quote never
/// forms an empty pair, so `''` stays two apostrophes.
fn match_quotes(items: &mut [Item]) {
    let mut open: Vec<usize> = Vec::new();
    for index in 0..items.len() {
        let Some(Item::Quote(quote)) = items.get(index) else {
            continue;
        };
        let (ch, can_open, can_close) = (quote.ch, quote.can_open, quote.can_close);
        let open_same = open.iter().rposition(|&i| quote_at(items, i) == ch);
        if can_close
            && let Some(stack_pos) = open_same
            && let Some(&opener) = open.get(stack_pos)
            && !(ch == '\'' && opener + 1 == index)
        {
            open.truncate(stack_pos);
            set_partner(items, opener, index);
            set_partner(items, index, opener);
        } else if open_same.is_none() && can_open {
            open.push(index);
        }
    }
}

/// The kind of the quote item at `index`, or a placeholder that matches nothing.
fn quote_at(items: &[Item], index: usize) -> char {
    match items.get(index) {
        Some(Item::Quote(quote)) => quote.ch,
        _ => '\0',
    }
}

fn set_partner(items: &mut [Item], index: usize, partner: usize) {
    if let Some(Item::Quote(quote)) = items.get_mut(index) {
        quote.partner = Some(partner);
    }
}

/// Render the classified, matched items into inlines starting at `*cursor`, consuming items until
/// the run ends or a closing quote whose opener precedes `*cursor` is reached. A matched opening
/// quote recurses to gather its span's content into a `Quoted`; an unmatched quote contributes its
/// directional glyph; text and breaks pass through.
fn render_items(items: &[Item], cursor: &mut usize) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    let mut pending = String::new();
    let flush = |pending: &mut String, out: &mut Vec<Inline>| {
        if !pending.is_empty() {
            out.push(Inline::Str(std::mem::take(pending)));
        }
    };
    while let Some(item) = items.get(*cursor) {
        match item {
            Item::Text(text) => {
                pending.push_str(text);
                *cursor += 1;
            }
            Item::Break(brk) => {
                flush(&mut pending, &mut out);
                out.push(brk.clone());
                *cursor += 1;
            }
            Item::Quote(quote) => match quote.partner {
                Some(partner) if partner > *cursor => {
                    flush(&mut pending, &mut out);
                    let ch = quote.ch;
                    *cursor += 1;
                    let inner = render_items(items, cursor);
                    // Step past the closing partner that ended the recursion.
                    *cursor += 1;
                    out.push(Inline::Quoted(quote_kind(ch), inner));
                }
                Some(_) => {
                    // The closing partner of an open span: stop so the opener's frame collects it.
                    break;
                }
                None => {
                    pending.push(quote.glyph);
                    *cursor += 1;
                }
            },
        }
    }
    flush(&mut pending, &mut out);
    out
}

/// For each token in the run, the character immediately before and after it (skipping nothing —
/// breaks count as spaces, run edges as `None`). Used to decide quote flanking with full context.
fn run_context(run: &[RunTok]) -> Vec<(Option<char>, Option<char>)> {
    let plain: Vec<Option<char>> = run
        .iter()
        .map(|tok| match tok {
            RunTok::Char(ch) | RunTok::Quote(ch) => Some(*ch),
            RunTok::Break(_) => Some(' '),
        })
        .collect();
    (0..run.len())
        .map(|i| {
            let before = i
                .checked_sub(1)
                .and_then(|j| plain.get(j))
                .copied()
                .flatten();
            let after = plain.get(i + 1).copied().flatten();
            (before, after)
        })
        .collect()
}

fn quote_kind(ch: char) -> QuoteType {
    if ch == '\'' {
        QuoteType::SingleQuote
    } else {
        QuoteType::DoubleQuote
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(input: &str) -> Document {
        OpmlReader
            .read(input, &ReaderOptions::default())
            .expect("outline input parses")
    }

    fn headers(document: &Document) -> Vec<(i32, String)> {
        document
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Header(level, _, inlines) => Some((*level, inline_text(inlines))),
                _ => None,
            })
            .collect()
    }

    fn inline_text(inlines: &[Inline]) -> String {
        inlines
            .iter()
            .map(|inline| match inline {
                Inline::Str(text) => text.as_str(),
                Inline::Space => " ",
                _ => "",
            })
            .collect()
    }

    #[test]
    fn nesting_assigns_header_levels() {
        let document = read(
            "<opml><body>\
             <outline text=\"A\">\
             <outline text=\"B\"><outline text=\"C\"/></outline>\
             </outline>\
             </body></opml>",
        );
        assert_eq!(
            headers(&document),
            [
                (1, "A".to_owned()),
                (2, "B".to_owned()),
                (3, "C".to_owned()),
            ]
        );
    }

    #[test]
    fn sibling_outlines_share_a_level() {
        let document = read("<opml><body><outline text=\"A\"/><outline text=\"B\"/></body></opml>");
        assert_eq!(
            headers(&document),
            [(1, "A".to_owned()), (1, "B".to_owned())]
        );
    }

    #[test]
    fn note_attribute_parses_as_markdown() {
        let document = read("<opml><body><outline text=\"H\" _note=\"**b**\"/></body></opml>");
        assert!(matches!(
            document.blocks.first(),
            Some(Block::Header(1, _, _))
        ));
        let Some(Block::Para(inlines)) = document.blocks.get(1) else {
            panic!("expected the note to parse into a paragraph");
        };
        assert!(matches!(inlines.first(), Some(Inline::Strong(_))));
    }

    #[test]
    fn text_attribute_tokenizes_on_whitespace() {
        let document = read("<opml><body><outline text=\"Hello   World\"/></body></opml>");
        let Some(Block::Header(_, _, inlines)) = document.blocks.first() else {
            panic!("expected a header");
        };
        assert!(matches!(
            inlines.as_slice(),
            [Inline::Str(first), Inline::Space, Inline::Str(second)]
                if first == "Hello" && second == "World"
        ));
    }

    fn first_header_inlines(input: &str) -> Vec<Inline> {
        let document = read(input);
        match document.blocks.into_iter().next() {
            Some(Block::Header(_, _, inlines)) => inlines,
            _ => panic!("expected a header"),
        }
    }

    fn outline(text: &str) -> String {
        format!("<opml><body><outline text=\"{text}\"/></body></opml>")
    }

    #[test]
    fn text_attribute_parses_inline_html_markup() {
        let inlines = first_header_inlines(&outline(
            "&lt;strong&gt;Bold&lt;/strong&gt; and &lt;em&gt;it&lt;/em&gt;",
        ));
        assert_eq!(
            inlines,
            vec![
                Inline::Strong(vec![Inline::Str("Bold".to_owned())]),
                Inline::Space,
                Inline::Str("and".to_owned()),
                Inline::Space,
                Inline::Emph(vec![Inline::Str("it".to_owned())]),
            ]
        );
    }

    #[test]
    fn text_attribute_decodes_entities_twice_then_parses_code() {
        // The XML layer decodes the attribute once (`&amp;amp;` becomes `&amp;`); the inline parse
        // decodes again (`&amp;` becomes `&`) and reads the `<code>` element.
        let inlines = first_header_inlines(&outline("a &lt;code&gt;c&lt;/code&gt; b &amp;amp; z"));
        assert_eq!(
            inlines,
            vec![
                Inline::Str("a".to_owned()),
                Inline::Space,
                Inline::Code(Box::default(), "c".to_owned()),
                Inline::Space,
                Inline::Str("b".to_owned()),
                Inline::Space,
                Inline::Str("&".to_owned()),
                Inline::Space,
                Inline::Str("z".to_owned()),
            ]
        );
    }

    #[test]
    fn text_attribute_parses_nested_markup() {
        let inlines = first_header_inlines(&outline(
            "&lt;strong&gt;&lt;em&gt;both&lt;/em&gt;&lt;/strong&gt;",
        ));
        assert_eq!(
            inlines,
            vec![Inline::Strong(vec![Inline::Emph(vec![Inline::Str(
                "both".to_owned()
            )])])]
        );
    }

    #[test]
    fn text_attribute_parses_superscript_and_subscript() {
        let inlines = first_header_inlines(&outline(
            "x&lt;sup&gt;2&lt;/sup&gt;&lt;sub&gt;n&lt;/sub&gt;",
        ));
        assert_eq!(
            inlines,
            vec![
                Inline::Str("x".to_owned()),
                Inline::Superscript(vec![Inline::Str("2".to_owned())]),
                Inline::Subscript(vec![Inline::Str("n".to_owned())]),
            ]
        );
    }

    #[test]
    fn text_attribute_parses_an_anchor_into_a_link() {
        let inlines = first_header_inlines(&outline(
            "&lt;a href=&quot;http://e.com&quot;&gt;l&lt;/a&gt;",
        ));
        let Some(Inline::Link(_, label, target)) = inlines.first() else {
            panic!("expected a link");
        };
        assert_eq!(label, &vec![Inline::Str("l".to_owned())]);
        assert_eq!(target.url, "http://e.com");
    }

    #[test]
    fn named_character_reference_in_text_decodes_once_decoded() {
        // `&amp;copy;` survives the XML decode as `&copy;`, which the inline parse turns into ©.
        let inlines = first_header_inlines(&outline("c &amp;copy; r"));
        assert_eq!(
            inlines,
            vec![
                Inline::Str("c".to_owned()),
                Inline::Space,
                Inline::Str("\u{a9}".to_owned()),
                Inline::Space,
                Inline::Str("r".to_owned()),
            ]
        );
    }

    #[test]
    fn link_outline_wraps_heading_in_a_link_to_its_url() {
        let document = read(
            "<opml><body><outline type=\"link\" text=\"Site\" url=\"http://e.com/p\"/></body></opml>",
        );
        let Some(Block::Header(1, _, inlines)) = document.blocks.first() else {
            panic!("expected a header");
        };
        let Some(Inline::Link(_, label, target)) = inlines.first() else {
            panic!("expected a link heading");
        };
        assert_eq!(label, &vec![Inline::Str("Site".to_owned())]);
        assert_eq!(target.url, "http://e.com/p");
        assert_eq!(target.title, "");
    }

    #[test]
    fn link_outline_without_url_links_to_an_empty_target() {
        let document = read("<opml><body><outline type=\"LINK\" text=\"Site\"/></body></opml>");
        let Some(Block::Header(_, _, inlines)) = document.blocks.into_iter().next() else {
            panic!("expected a header");
        };
        let Some(Inline::Link(_, _, target)) = inlines.first() else {
            panic!("expected a link heading");
        };
        assert_eq!(target.url, "");
    }

    #[test]
    fn non_link_outline_with_a_url_keeps_a_plain_heading() {
        let document =
            read("<opml><body><outline text=\"Site\" url=\"http://e.com/p\"/></body></opml>");
        let Some(Block::Header(_, _, inlines)) = document.blocks.first() else {
            panic!("expected a header");
        };
        assert_eq!(inlines.as_slice(), [Inline::Str("Site".to_owned())]);
    }

    #[test]
    fn missing_text_attribute_yields_an_empty_heading() {
        let document = read("<opml><body><outline/></body></opml>");
        assert_eq!(headers(&document), [(1, String::new())]);
    }

    #[test]
    fn single_quoted_attributes_are_read() {
        let document = read("<opml><body><outline text='quoted'/></body></opml>");
        assert_eq!(headers(&document), [(1, "quoted".to_owned())]);
    }

    #[test]
    fn comments_instructions_and_doctype_are_skipped() {
        let document = read(
            "<?xml version=\"1.0\"?><!DOCTYPE opml><opml><!-- c -->\
             <body><outline text=\"A\"/></body></opml>",
        );
        assert_eq!(headers(&document), [(1, "A".to_owned())]);
    }

    #[test]
    fn metadata_is_drawn_from_the_head() {
        let document = read(
            "<opml><head><title>T</title><ownerName>Me</ownerName>\
             <dateModified>2020</dateModified></head><body></body></opml>",
        );
        assert!(matches!(
            document.meta.get("title"),
            Some(MetaValue::MetaInlines(inlines)) if inline_text(inlines) == "T"
        ));
        assert!(matches!(
            document.meta.get("date"),
            Some(MetaValue::MetaInlines(inlines)) if inline_text(inlines) == "2020"
        ));
        let Some(MetaValue::MetaList(authors)) = document.meta.get("author") else {
            panic!("expected an author list");
        };
        assert!(matches!(
            authors.first(),
            Some(MetaValue::MetaInlines(inlines)) if inline_text(inlines) == "Me"
        ));
    }

    #[test]
    fn absent_owner_yields_an_empty_author_list() {
        let document = read("<opml><head><title>T</title></head><body></body></opml>");
        assert!(matches!(
            document.meta.get("author"),
            Some(MetaValue::MetaList(authors)) if authors.is_empty()
        ));
    }

    #[test]
    fn named_entities_decode() {
        assert_eq!(
            decode_entities("a &amp; b &lt;c&gt; &quot;d&quot; &apos;e&apos;"),
            "a & b <c> \"d\" 'e'"
        );
    }

    #[test]
    fn numeric_entities_decode_in_decimal_and_hex() {
        assert_eq!(decode_entities("&#65;&#x42;&#X43;"), "ABC");
    }

    #[test]
    fn malformed_or_unknown_references_are_left_verbatim() {
        assert_eq!(decode_entities("&amp"), "&amp");
        assert_eq!(decode_entities("&nosuch;"), "&nosuch;");
        assert_eq!(decode_entities("&#zz;"), "&#zz;");
        assert_eq!(decode_entities("bare & text"), "bare & text");
    }

    #[test]
    fn malformed_markup_does_not_panic() {
        let _ = read("<opml><body><outline text=\"x\"><outline text=\"y\"></body>");
        let _ = read("<<<>>><opml attr");
        let _ = read("");
    }

    fn title_inlines(document: &Document) -> Vec<Inline> {
        match document.meta.get("title") {
            Some(MetaValue::MetaInlines(inlines)) => inlines.clone(),
            _ => panic!("expected title inlines"),
        }
    }

    #[test]
    fn text_attribute_pairs_double_quotes_into_a_quoted_span() {
        let inlines = first_header_inlines(&outline("&quot;hi&quot;"));
        assert_eq!(
            inlines,
            vec![Inline::Quoted(
                QuoteType::DoubleQuote,
                vec![Inline::Str("hi".to_owned())]
            )]
        );
    }

    #[test]
    fn text_attribute_pairs_single_quotes_into_a_quoted_span() {
        let inlines = first_header_inlines(&outline("&apos;hi&apos;"));
        assert_eq!(
            inlines,
            vec![Inline::Quoted(
                QuoteType::SingleQuote,
                vec![Inline::Str("hi".to_owned())]
            )]
        );
    }

    #[test]
    fn text_attribute_curls_an_apostrophe() {
        let inlines = first_header_inlines(&outline("it&apos;s"));
        assert_eq!(inlines, vec![Inline::Str("it\u{2019}s".to_owned())]);
    }

    #[test]
    fn text_attribute_folds_dashes_and_ellipsis() {
        let inlines = first_header_inlines(&outline("a---b--c...d"));
        // Three hyphens fold to an em dash, two to an en dash, three dots to an ellipsis.
        assert_eq!(
            inlines,
            vec![Inline::Str("a\u{2014}b\u{2013}c\u{2026}d".to_owned())]
        );
    }

    #[test]
    fn dash_runs_fold_greedily_to_em_dashes() {
        assert_eq!(fold_dash_run(1), "-");
        assert_eq!(fold_dash_run(2), "\u{2013}");
        assert_eq!(fold_dash_run(3), "\u{2014}");
        assert_eq!(fold_dash_run(4), "\u{2014}-");
        assert_eq!(fold_dash_run(5), "\u{2014}\u{2013}");
        assert_eq!(fold_dash_run(6), "\u{2014}\u{2014}");
        assert_eq!(fold_dash_run(7), "\u{2014}\u{2014}-");
    }

    #[test]
    fn ellipsis_runs_fold_per_group_of_three() {
        assert_eq!(fold_ellipsis_run(1), ".");
        assert_eq!(fold_ellipsis_run(2), "..");
        assert_eq!(fold_ellipsis_run(3), "\u{2026}");
        assert_eq!(fold_ellipsis_run(4), "\u{2026}.");
        assert_eq!(fold_ellipsis_run(6), "\u{2026}\u{2026}");
    }

    #[test]
    fn text_attribute_resolves_an_unpaired_double_quote_directionally() {
        // An opener-context quote followed by a word becomes the left glyph; one with no following
        // word becomes the right glyph.
        let opener = first_header_inlines(&outline("&quot;open only"));
        assert_eq!(
            opener.first(),
            Some(&Inline::Str("\u{201c}open".to_owned()))
        );
        let closer = first_header_inlines(&outline("close only&quot;"));
        assert_eq!(closer.last(), Some(&Inline::Str("only\u{201d}".to_owned())));
    }

    #[test]
    fn double_quotes_do_not_nest_within_their_own_kind() {
        // The inner double quote closes the outer span rather than nesting; the rest stay glyphs.
        let inlines = first_header_inlines(&outline("&quot;a &quot;b&quot; c&quot;"));
        assert_eq!(
            inlines,
            vec![
                Inline::Quoted(
                    QuoteType::DoubleQuote,
                    vec![Inline::Str("a".to_owned()), Inline::Space]
                ),
                Inline::Str("b\u{201d}".to_owned()),
                Inline::Space,
                Inline::Str("c\u{201d}".to_owned()),
            ]
        );
    }

    #[test]
    fn a_different_quote_kind_nests() {
        let inlines = first_header_inlines(&outline("&quot;a &apos;b&apos; c&quot;"));
        assert_eq!(
            inlines,
            vec![Inline::Quoted(
                QuoteType::DoubleQuote,
                vec![
                    Inline::Str("a".to_owned()),
                    Inline::Space,
                    Inline::Quoted(QuoteType::SingleQuote, vec![Inline::Str("b".to_owned())]),
                    Inline::Space,
                    Inline::Str("c".to_owned()),
                ]
            )]
        );
    }

    #[test]
    fn two_straight_single_quotes_stay_apostrophes() {
        let inlines = first_header_inlines(&outline("&apos;&apos;"));
        assert_eq!(inlines, vec![Inline::Str("\u{2019}\u{2019}".to_owned())]);
    }

    #[test]
    fn code_span_curls_quotes_into_glyph_pairs() {
        let inlines = first_header_inlines(&outline("&lt;code&gt;&apos;q&apos;&lt;/code&gt;"));
        // A matched pair inside a code span renders as its left and right glyphs, not a Quoted node.
        assert_eq!(
            inlines,
            vec![Inline::Code(Box::default(), "\u{2018}q\u{2019}".to_owned())]
        );
    }

    #[test]
    fn code_span_curls_an_apostrophe_and_folds_dashes() {
        let inlines = first_header_inlines(&outline("&lt;code&gt;it&apos;s --- x&lt;/code&gt;"));
        assert_eq!(
            inlines,
            vec![Inline::Code(
                Box::default(),
                "it\u{2019}s \u{2014} x".to_owned()
            )]
        );
    }

    #[test]
    fn smart_typography_recurses_into_inline_markup() {
        let inlines = first_header_inlines(&outline("&lt;em&gt;&quot;hi&quot;&lt;/em&gt;"));
        assert_eq!(
            inlines,
            vec![Inline::Emph(vec![Inline::Quoted(
                QuoteType::DoubleQuote,
                vec![Inline::Str("hi".to_owned())]
            )])]
        );
    }

    #[test]
    fn note_body_uses_the_markdown_preset() {
        // A definition list is a Markdown-dialect construct absent from bare CommonMark; its presence
        // confirms the note body is parsed with the extended Markdown extension set.
        let document = read(
            "<opml><body><outline text=\"H\" _note=\"Term&#10;:   Definition\"/></body></opml>",
        );
        assert!(
            document
                .blocks
                .iter()
                .any(|block| matches!(block, Block::DefinitionList(_))),
            "expected the note to parse a definition list"
        );
    }

    #[test]
    fn note_body_applies_smart_typography() {
        let document = read("<opml><body><outline text=\"H\" _note=\"it&apos;s\"/></body></opml>");
        let Some(Block::Para(inlines)) = document.blocks.get(1) else {
            panic!("expected a note paragraph");
        };
        assert_eq!(inlines, &vec![Inline::Str("it\u{2019}s".to_owned())]);
    }

    #[test]
    fn metadata_keeps_straight_quotes_dashes_and_dots() {
        // Document metadata is not smart-transformed: its punctuation stays verbatim.
        let document = read(
            "<opml><head><title>&quot;a&quot; --- it&apos;s ...</title></head><body></body></opml>",
        );
        assert_eq!(
            title_inlines(&document),
            vec![
                Inline::Str("\"a\"".to_owned()),
                Inline::Space,
                Inline::Str("---".to_owned()),
                Inline::Space,
                Inline::Str("it's".to_owned()),
                Inline::Space,
                Inline::Str("...".to_owned()),
            ]
        );
    }

    #[test]
    fn metadata_preserves_boundary_whitespace_as_space() {
        let document = read("<opml><head><title>  a b  </title></head><body></body></opml>");
        assert_eq!(
            title_inlines(&document),
            vec![
                Inline::Space,
                Inline::Str("a".to_owned()),
                Inline::Space,
                Inline::Str("b".to_owned()),
                Inline::Space,
            ]
        );
    }

    #[test]
    fn metadata_turns_an_internal_newline_into_a_soft_break() {
        let document =
            read("<opml><head><title>line one\nline two</title></head><body></body></opml>");
        assert_eq!(
            title_inlines(&document),
            vec![
                Inline::Str("line".to_owned()),
                Inline::Space,
                Inline::Str("one".to_owned()),
                Inline::SoftBreak,
                Inline::Str("line".to_owned()),
                Inline::Space,
                Inline::Str("two".to_owned()),
            ]
        );
    }

    #[test]
    fn present_but_empty_owner_contributes_an_empty_author() {
        // A present `ownerName`, even with empty content, yields one author entry — distinct from an
        // absent element, which yields none.
        let document = read("<opml><head><ownerName></ownerName></head><body></body></opml>");
        let Some(MetaValue::MetaList(authors)) = document.meta.get("author") else {
            panic!("expected an author list");
        };
        assert_eq!(authors, &vec![MetaValue::MetaInlines(Vec::new())]);
    }
}
