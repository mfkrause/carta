//! `MediaWiki` writer: renders the document model to `MediaWiki` markup.
//!
//! Inline content is not wrapped — a soft break renders as a single space and block structure is
//! conveyed through `MediaWiki`'s line-oriented markup. Output carries no trailing newline; the caller
//! appends one. This format has no public specification, so its rules are stated directly here.

use std::fmt::Write as _;

use oxidoc_ast::{
    Alignment, Attr, Block, Cell, Document, Format, Inline, ListAttributes, ListNumberStyle,
    MathType, Row, Table, TableBody, Target, to_plain_text,
};
use oxidoc_core::{Result, Writer, WriterOptions};

use crate::common::{attribute_value, escape_xml, is_known_attribute, is_uri_scheme, quote_marks};

/// Renders a document to `MediaWiki` markup.
#[derive(Debug, Default, Clone, Copy)]
pub struct MediawikiWriter;

impl Writer for MediawikiWriter {
    fn write(&self, document: &Document, _options: &WriterOptions) -> Result<String> {
        let mut state = State::default();
        let body = state.blocks(&document.blocks);
        let out = if state.has_notes {
            format!("{}\n\n<references />", body.trim_end_matches('\n'))
        } else {
            body
        };
        Ok(out.trim_end_matches('\n').to_owned())
    }
}

/// Tracks whether any footnote was emitted, so the trailing `<references />` block is added only when
/// the document actually uses notes.
#[derive(Debug, Default)]
struct State {
    has_notes: bool,
}

impl State {
    /// Render a block sequence. Every block but a `Plain` contributes a trailing blank line before
    /// the next block; blocks that render to nothing are dropped.
    fn blocks(&mut self, blocks: &[Block]) -> String {
        let parts: Vec<String> = blocks
            .iter()
            .filter_map(|block| {
                let core = self.block(block);
                if core.is_empty() {
                    return None;
                }
                Some(if matches!(block, Block::Plain(_)) {
                    core
                } else {
                    format!("{core}\n")
                })
            })
            .collect();
        parts.join("\n")
    }

    fn block(&mut self, block: &Block) -> String {
        match block {
            Block::Plain(inlines) | Block::Para(inlines) => self.inlines(inlines),
            Block::Header(level, attr, inlines) => self.header(*level, attr, inlines),
            Block::CodeBlock(attr, text) => code_block(attr, text),
            Block::RawBlock(format, text) => raw_passthrough(format, text),
            Block::BlockQuote(blocks) => {
                let body = self.blocks(blocks);
                format!("<blockquote>{}\n</blockquote>", body.trim_end_matches('\n'))
            }
            Block::BulletList(items) => self.list(block, '*', items),
            Block::OrderedList(_, items) => self.list(block, '#', items),
            Block::DefinitionList(items) => self.definition_list(items),
            Block::HorizontalRule => "-----".to_owned(),
            Block::Table(table) => self.table(table),
            Block::Figure(attr, _, blocks) => self.figure(attr, blocks),
            Block::Div(attr, blocks) => {
                let body = self.blocks(blocks);
                format!("<div{}>\n{body}\n</div>", render_attr(attr))
            }
            Block::LineBlock(lines) => self.line_block(lines),
        }
    }

    fn header(&mut self, level: i32, attr: &Attr, inlines: &[Inline]) -> String {
        let depth = level.clamp(1, 6);
        let equals = "=".repeat(depth.unsigned_abs() as usize);
        let text = self.inlines(inlines);
        let heading = format!("{equals} {text} {equals}");
        if attr.id.is_empty() || attr.id == section_anchor(inlines) {
            heading
        } else {
            format!("<span id=\"{}\"></span>\n{heading}", escape_attr(&attr.id))
        }
    }

    fn figure(&mut self, attr: &Attr, blocks: &[Block]) -> String {
        let merged = Attr {
            id: attr.id.clone(),
            classes: std::iter::once("figure".to_owned())
                .chain(attr.classes.iter().cloned())
                .collect(),
            attributes: attr.attributes.clone(),
        };
        let body = self.blocks(blocks);
        format!("<div{}>\n{body}\n</div>", render_attr(&merged))
    }

    fn line_block(&mut self, lines: &[Vec<Inline>]) -> String {
        let rendered: Vec<String> = lines.iter().map(|line| self.inlines(line)).collect();
        rendered.join("<br />\n")
    }

    fn definition_list(&mut self, items: &[(Vec<Inline>, Vec<Vec<Block>>)]) -> String {
        let mut lines = Vec::new();
        for (term, definitions) in items {
            lines.push(format!("; {}", self.inlines(term)));
            for definition in definitions {
                let body = self.blocks(definition);
                lines.push(format!(": {}", body.trim_end_matches('\n')));
            }
        }
        lines.join("\n")
    }

    /// Render a bullet or ordered list, choosing the compact prefix notation when the whole list is
    /// simple (single-block items, with default-style ordered sublists) and HTML tags otherwise.
    /// `marker` is the compact-notation prefix character for this list kind (`*` or `#`).
    fn list(&mut self, block: &Block, marker: char, items: &[Vec<Block>]) -> String {
        if is_simple_list(block) {
            self.compact_list(marker, items, "")
        } else {
            self.html_list(block)
        }
    }

    /// Render a list in the compact prefix notation. `parent` is the accumulated marker run of the
    /// enclosing levels; this level appends its own marker to it on every line.
    fn compact_list(&mut self, marker: char, items: &[Vec<Block>], parent: &str) -> String {
        let prefix = format!("{parent}{marker}");
        let mut lines = Vec::new();
        for item in items {
            if item.is_empty() {
                lines.push(prefix.clone());
                continue;
            }
            for inner in item {
                match inner {
                    Block::Plain(inlines) | Block::Para(inlines) => {
                        lines.push(format!("{prefix} {}", self.inlines(inlines)));
                    }
                    Block::BulletList(sub) => lines.push(self.compact_list('*', sub, &prefix)),
                    Block::OrderedList(_, sub) => lines.push(self.compact_list('#', sub, &prefix)),
                    other => lines.push(self.block(other)),
                }
            }
        }
        lines.join("\n")
    }

    fn html_list(&mut self, block: &Block) -> String {
        let (open, close, items) = match block {
            Block::BulletList(items) => ("<ul>".to_owned(), "</ul>", items),
            Block::OrderedList(attrs, items) => (ordered_open_tag(attrs), "</ol>", items),
            _ => return String::new(),
        };
        let rendered: Vec<String> = items.iter().map(|item| self.html_item(item)).collect();
        format!("{open}\n{}{close}", rendered.join("\n"))
    }

    fn html_item(&mut self, item: &[Block]) -> String {
        let parts: Vec<String> = item
            .iter()
            .map(|block| self.html_item_block(block))
            .collect();
        format!("<li>{}</li>", parts.join("\n"))
    }

    /// Render one block inside an HTML list item: a paragraph is wrapped in `<p>`, a plain block
    /// contributes its bare inline content, a nested list stays in HTML form, and any other block
    /// renders as it does elsewhere.
    fn html_item_block(&mut self, block: &Block) -> String {
        match block {
            Block::Para(inlines) => format!("<p>{}</p>", self.inlines(inlines)),
            Block::Plain(inlines) => self.inlines(inlines),
            Block::BulletList(_) | Block::OrderedList(..) => self.html_list(block),
            other => self.block(other),
        }
    }

    fn table(&mut self, table: &Table) -> String {
        let aligns: Vec<Alignment> = table
            .col_specs
            .iter()
            .map(|spec| spec.align.clone())
            .collect();
        let mut out = String::from("{| class=\"wikitable\"");
        if !table.caption.long.is_empty() {
            let caption = self.blocks(&table.caption.long);
            let _ = write!(out, "\n|+ {}", caption.trim_end_matches('\n'));
        }
        let mut rows: Vec<String> = Vec::new();
        for row in &table.head.rows {
            rows.push(self.table_row(row, &aligns, true));
        }
        for body in &table.bodies {
            rows.extend(self.body_rows(body, &aligns));
        }
        for row in &table.foot.rows {
            rows.push(self.table_row(row, &aligns, true));
        }
        for row in rows {
            let _ = write!(out, "\n{row}");
        }
        out.push_str("\n|}");
        out
    }

    fn body_rows(&mut self, body: &TableBody, aligns: &[Alignment]) -> Vec<String> {
        let mut rows: Vec<String> = body
            .head
            .iter()
            .map(|row| self.table_row(row, aligns, true))
            .collect();
        rows.extend(
            body.body
                .iter()
                .map(|row| self.table_row(row, aligns, false)),
        );
        rows
    }

    fn table_row(&mut self, row: &Row, aligns: &[Alignment], header: bool) -> String {
        let cells: Vec<String> = row
            .cells
            .iter()
            .enumerate()
            .map(|(index, cell)| self.cell(cell, aligns.get(index), header))
            .collect();
        let mut out = String::from("|-");
        for cell in cells {
            let _ = write!(out, "\n{cell}");
        }
        out
    }

    fn cell(&mut self, cell: &Cell, col_align: Option<&Alignment>, header: bool) -> String {
        let marker = if header { "! " } else { "| " };
        let effective = match &cell.align {
            Alignment::AlignDefault => col_align.unwrap_or(&Alignment::AlignDefault),
            explicit => explicit,
        };
        let mut attrs = Vec::new();
        if cell.row_span != 1 {
            attrs.push(format!("rowspan=\"{}\"", cell.row_span));
        }
        if cell.col_span != 1 {
            attrs.push(format!("colspan=\"{}\"", cell.col_span));
        }
        if let Some(style) = alignment_style(effective) {
            attrs.push(format!("style=\"{style}\""));
        }
        let body = self.blocks(&cell.content);
        let content = body.trim_end_matches('\n');
        if attrs.is_empty() {
            format!("{marker}{content}")
        } else {
            format!("{marker}{}| {content}", attrs.join(" "))
        }
    }

    fn inlines(&mut self, inlines: &[Inline]) -> String {
        inlines.iter().map(|inline| self.inline(inline)).collect()
    }

    fn inline(&mut self, inline: &Inline) -> String {
        match inline {
            Inline::Str(text) => escape_text(text),
            Inline::Emph(inlines) => format!("''{}''", self.inlines(inlines)),
            Inline::Strong(inlines) => format!("'''{}'''", self.inlines(inlines)),
            Inline::Strikeout(inlines) => format!("<s>{}</s>", self.inlines(inlines)),
            Inline::Superscript(inlines) => format!("<sup>{}</sup>", self.inlines(inlines)),
            Inline::Subscript(inlines) => format!("<sub>{}</sub>", self.inlines(inlines)),
            Inline::Underline(inlines) => format!("<u>{}</u>", self.inlines(inlines)),
            Inline::SmallCaps(inlines) | Inline::Cite(_, inlines) => self.inlines(inlines),
            Inline::Quoted(kind, inlines) => {
                let (open, close) = quote_marks(kind);
                format!("{open}{}{close}", self.inlines(inlines))
            }
            Inline::Code(_, text) => format!("<code>{}</code>", escape_text(text)),
            Inline::Space | Inline::SoftBreak => " ".to_owned(),
            Inline::LineBreak => "<br />\n".to_owned(),
            Inline::Math(kind, text) => {
                let display = match kind {
                    MathType::InlineMath => "inline",
                    MathType::DisplayMath => "block",
                };
                format!("<math display=\"{display}\">{text}</math>")
            }
            Inline::RawInline(format, text) => raw_passthrough(format, text),
            Inline::Link(_, inlines, target) => self.link(inlines, target),
            Inline::Image(attr, inlines, target) => self.image(attr, inlines, target),
            Inline::Span(attr, inlines) => {
                format!(
                    "<span{}>{}</span>",
                    render_attr(attr),
                    self.inlines(inlines)
                )
            }
            Inline::Note(blocks) => self.note(blocks),
        }
    }

    fn link(&mut self, inlines: &[Inline], target: &Target) -> String {
        let label = self.inlines(inlines);
        let plain = to_plain_text(inlines);
        if is_external_uri(&target.url) {
            if plain == target.url {
                target.url.clone()
            } else {
                format!("[{} {label}]", target.url)
            }
        } else {
            let destination = target.url.strip_prefix('/').unwrap_or(&target.url);
            if plain == target.url {
                format!("[[{destination}]]")
            } else {
                format!("[[{destination}|{label}]]")
            }
        }
    }

    fn image(&mut self, attr: &Attr, inlines: &[Inline], target: &Target) -> String {
        let mut parts = vec![format!("File:{}", target.url)];
        if let Some(size) = image_size(attr) {
            parts.push(size);
        }
        let alt = self.inlines(inlines);
        if !alt.is_empty() {
            parts.push(alt);
        }
        format!("[[{}]]", parts.join("|"))
    }

    fn note(&mut self, blocks: &[Block]) -> String {
        self.has_notes = true;
        let body = self.blocks(blocks);
        format!("<ref>{}</ref>", body.trim_end_matches('\n'))
    }
}

/// The anchor `MediaWiki` derives for a section heading: its plain-text content with spaces turned
/// into underscores. A heading whose explicit id already equals this needs no separate anchor.
fn section_anchor(inlines: &[Inline]) -> String {
    to_plain_text(inlines).replace(' ', "_")
}

fn alignment_style(align: &Alignment) -> Option<&'static str> {
    match align {
        Alignment::AlignLeft => Some("text-align: left;"),
        Alignment::AlignRight => Some("text-align: right;"),
        Alignment::AlignCenter => Some("text-align: center;"),
        Alignment::AlignDefault => None,
    }
}

/// The `<ol …>` opening tag for an ordered list that must use HTML form: a `start` attribute when
/// the first number is not one, followed by the numeral style as a `list-style-type`.
fn ordered_open_tag(attrs: &ListAttributes) -> String {
    let mut tag = String::from("<ol");
    if attrs.start != 1 {
        let _ = write!(tag, " start=\"{}\"", attrs.start);
    }
    let _ = write!(
        tag,
        " style=\"list-style-type: {};\">",
        list_style_type(&attrs.style)
    );
    tag
}

fn list_style_type(style: &ListNumberStyle) -> &'static str {
    match style {
        ListNumberStyle::DefaultStyle | ListNumberStyle::Decimal => "decimal",
        ListNumberStyle::LowerAlpha => "lower-alpha",
        ListNumberStyle::UpperAlpha => "upper-alpha",
        ListNumberStyle::LowerRoman => "lower-roman",
        ListNumberStyle::UpperRoman => "upper-roman",
        ListNumberStyle::Example => "example",
    }
}

/// Whether a list can be rendered in the compact prefix notation rather than HTML tags. An ordered
/// list qualifies only with a default numeral style starting at one; every item must be simple.
fn is_simple_list(block: &Block) -> bool {
    match block {
        Block::BulletList(items) => items.iter().all(|item| is_simple_item(item)),
        Block::OrderedList(attrs, items) => {
            attrs.start == 1
                && matches!(
                    attrs.style,
                    ListNumberStyle::DefaultStyle | ListNumberStyle::Decimal
                )
                && items.iter().all(|item| is_simple_item(item))
        }
        _ => false,
    }
}

/// Whether a list item fits the compact notation: empty, a single text block, or a single text block
/// followed by one sublist that is itself simple.
fn is_simple_item(item: &[Block]) -> bool {
    match item {
        [] | [Block::Plain(_) | Block::Para(_)] => true,
        [Block::Plain(_) | Block::Para(_), sublist] => is_simple_list(sublist),
        _ => false,
    }
}

/// Render a code block: a `<syntaxhighlight>` element when the first class names a known highlighting
/// language (with a `line` flag for line numbering), else an escaped `<pre>` carrying any classes.
fn code_block(attr: &Attr, text: &str) -> String {
    if let Some(language) = attr.classes.first()
        && is_highlight_language(language)
    {
        let numbered = if attr.classes.iter().any(|class| is_number_lines(class)) {
            " line"
        } else {
            ""
        };
        format!("<syntaxhighlight lang=\"{language}\"{numbered}>{text}</syntaxhighlight>")
    } else if attr.classes.is_empty() {
        format!("<pre>{}</pre>", escape_text(text))
    } else {
        format!(
            "<pre class=\"{}\">{}</pre>",
            escape_attr(&attr.classes.join(" ")),
            escape_text(text)
        )
    }
}

fn is_number_lines(class: &str) -> bool {
    matches!(class, "numberLines" | "number-lines" | "numberlines")
}

/// The `WxHpx` size descriptor for an image, derived from its `width`/`height` attributes; `None`
/// when neither is present.
fn image_size(attr: &Attr) -> Option<String> {
    let width = attribute_value(attr, "width");
    let height = attribute_value(attr, "height");
    match (width, height) {
        (Some(w), Some(h)) => Some(format!("{w}x{h}px")),
        (Some(w), None) => Some(format!("{w}px")),
        (None, Some(h)) => Some(format!("x{h}px")),
        (None, None) => None,
    }
}

/// Emit a raw-passthrough payload verbatim when its format is one `MediaWiki` carries directly
/// (`MediaWiki` markup or HTML); otherwise drop it.
fn raw_passthrough(format: &Format, text: &str) -> String {
    if matches!(format.0.as_str(), "mediawiki" | "html") {
        text.to_owned()
    } else {
        String::new()
    }
}

/// Render an [`Attr`] as an HTML attribute string with a leading space when non-empty: the id, the
/// classes, then key/value pairs (`data-`-prefixed unless the key is a recognized HTML attribute).
fn render_attr(attr: &Attr) -> String {
    let mut parts = Vec::new();
    if !attr.id.is_empty() {
        parts.push(format!("id=\"{}\"", escape_attr(&attr.id)));
    }
    if !attr.classes.is_empty() {
        parts.push(format!(
            "class=\"{}\"",
            escape_attr(&attr.classes.join(" "))
        ));
    }
    for (key, value) in &attr.attributes {
        let name = if is_known_attribute(key) {
            key.clone()
        } else {
            format!("data-{key}")
        };
        parts.push(format!("{name}=\"{}\"", escape_attr(value)));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" {}", parts.join(" "))
    }
}

/// Whether a URL is an absolute reference to an external resource: it carries a scheme drawn from the
/// known set. Internal references (page names, anchors, relative paths) use wiki-link syntax instead.
fn is_external_uri(url: &str) -> bool {
    if url.contains(char::is_whitespace) {
        return false;
    }
    let Some(colon) = url.find(':') else {
        return false;
    };
    let Some(scheme) = url.get(..colon) else {
        return false;
    };
    if scheme.is_empty() || !is_uri_scheme(scheme) {
        return false;
    }
    URI_SCHEMES
        .iter()
        .any(|known| scheme.eq_ignore_ascii_case(known))
}

fn escape_text(text: &str) -> String {
    escape_xml(text, false)
}

fn escape_attr(text: &str) -> String {
    escape_xml(text, true)
}

fn is_highlight_language(name: &str) -> bool {
    HIGHLIGHT_LANGUAGES.contains(&name)
}

/// Source languages recognized for `<syntaxhighlight>`. The match is case-sensitive: only these
/// lowercase canonical names and common short aliases select syntax highlighting; any other class
/// falls back to `<pre>`.
const HIGHLIGHT_LANGUAGES: &[&str] = &[
    "abc",
    "actionscript",
    "ada",
    "agda",
    "apache",
    "asn1",
    "asp",
    "ats",
    "awk",
    "bash",
    "bibtex",
    "boo",
    "c",
    "changelog",
    "clojure",
    "cmake",
    "coffee",
    "coldfusion",
    "comments",
    "commonlisp",
    "cpp",
    "crystal",
    "cs",
    "css",
    "curry",
    "d",
    "dart",
    "debiancontrol",
    "default",
    "diff",
    "djangotemplate",
    "dockerfile",
    "dosbat",
    "dot",
    "doxygen",
    "doxygenlua",
    "dtd",
    "eiffel",
    "elixir",
    "elm",
    "email",
    "erlang",
    "fasm",
    "fortranfixed",
    "fortranfree",
    "fsharp",
    "gap",
    "gcc",
    "gdscript",
    "gleam",
    "glsl",
    "gnuassembler",
    "go",
    "gpr",
    "graphql",
    "groovy",
    "hamlet",
    "haskell",
    "haxe",
    "html",
    "idris",
    "ini",
    "isocpp",
    "j",
    "java",
    "javadoc",
    "javascript",
    "javascriptreact",
    "json",
    "jsp",
    "julia",
    "kotlin",
    "latex",
    "lex",
    "lilypond",
    "literatecurry",
    "literatehaskell",
    "llvm",
    "lua",
    "m4",
    "makefile",
    "mandoc",
    "markdown",
    "mathematica",
    "matlab",
    "maxima",
    "mediawiki",
    "metafont",
    "mips",
    "modelines",
    "modula2",
    "modula3",
    "monobasic",
    "mustache",
    "nasm",
    "nim",
    "nix",
    "noweb",
    "objectivec",
    "objectivecpp",
    "ocaml",
    "octave",
    "odin",
    "opencl",
    "orgmode",
    "pascal",
    "perl",
    "php",
    "pike",
    "postscript",
    "povray",
    "powershell",
    "prolog",
    "protobuf",
    "pure",
    "purebasic",
    "purescript",
    "python",
    "qml",
    "r",
    "racket",
    "raku",
    "relaxng",
    "relaxngcompact",
    "rest",
    "rhtml",
    "roff",
    "ruby",
    "rust",
    "sass",
    "scala",
    "scheme",
    "sci",
    "scss",
    "sed",
    "sgml",
    "sml",
    "spdxcomments",
    "sql",
    "sqlmysql",
    "sqlpostgresql",
    "stan",
    "stata",
    "swift",
    "systemverilog",
    "tcl",
    "tcsh",
    "terraform",
    "texinfo",
    "tlaplus",
    "toml",
    "typescript",
    "typst",
    "verilog",
    "vhdl",
    "xml",
    "xorg",
    "xslt",
    "xul",
    "yacc",
    "yaml",
    "zig",
    "zsh",
    // Common short aliases that also select highlighting.
    "js",
    "ts",
    "py",
    "rb",
    "c++",
    "c#",
    "objective-c",
    "shell",
    "console",
    "pl",
    "ps1",
    "docker",
    "make",
];

/// URI schemes treated as external links. A URL whose scheme is absent from this set is rendered as
/// an internal wiki link.
const URI_SCHEMES: &[&str] = &[
    "aaa",
    "about",
    "acap",
    "acct",
    "cap",
    "cid",
    "coap",
    "coaps",
    "crid",
    "data",
    "dav",
    "dict",
    "dns",
    "example",
    "file",
    "ftp",
    "geo",
    "go",
    "gopher",
    "h323",
    "http",
    "https",
    "iax",
    "icap",
    "im",
    "imap",
    "info",
    "ipp",
    "ipps",
    "iris",
    "jabber",
    "ldap",
    "mailto",
    "mid",
    "msrp",
    "msrps",
    "mtqp",
    "mupdate",
    "news",
    "nfs",
    "nntp",
    "opaquelocktoken",
    "pkcs11",
    "pop",
    "pres",
    "reload",
    "rtsp",
    "rtsps",
    "rtspu",
    "service",
    "session",
    "shttp",
    "sieve",
    "sip",
    "sips",
    "sms",
    "snmp",
    "soap.beep",
    "soap.beeps",
    "ssh",
    "stun",
    "stuns",
    "tag",
    "tel",
    "telnet",
    "tftp",
    "thismessage",
    "tip",
    "tn3270",
    "turn",
    "turns",
    "tv",
    "urn",
    "vemmi",
    "vnc",
    "ws",
    "wss",
    "xmpp",
];
