//! Inline-level rendering for the ODT writer.

use std::fmt::Write as _;

use carta_ast::{Attr, Block, Inline, MathType, QuoteType, Target};
use carta_core::container::xml::{escape_attribute, escape_text, is_xml_char};
use carta_core::media::{decode_data_uri, extension_for_mime, image_mime_for_extension};

use super::helpers::{custom_style, is_opendocument, parent_prefix_index};
use super::media::{image_metrics, image_size};
use super::{Builder, Deco, Formula, Image, STACK_RED_ZONE, STACK_SEGMENT};

impl Builder<'_> {
    pub(super) fn inlines(&mut self, inlines: &[Inline]) {
        self.walk(inlines, &[]);
    }

    /// Renders an inline sequence under a set of active decorations. Plain content accumulates into a
    /// run emitted as a single styled span; a formatting node extends the decoration set over its
    /// content; a structural node (link, span, note, …) breaks the run and is emitted in place,
    /// carrying the active decorations into any content of its own.
    fn walk<'i>(&mut self, inlines: &'i [Inline], decos: &[Deco]) {
        stacker::maybe_grow(STACK_RED_ZONE, STACK_SEGMENT, || {
            let mut run: Vec<&'i Inline> = Vec::new();
            for inline in inlines {
                match inline {
                    Inline::Str(_)
                    | Inline::Space
                    | Inline::SoftBreak
                    | Inline::LineBreak
                    | Inline::Math(..) => run.push(inline),
                    Inline::RawInline(format, _) if is_opendocument(format) => run.push(inline),
                    Inline::RawInline(..) => {}
                    Inline::Emph(inner) => self.nested(&mut run, decos, Deco::Emph, inner),
                    Inline::Strong(inner) => self.nested(&mut run, decos, Deco::Strong, inner),
                    Inline::Underline(inner) => {
                        self.nested(&mut run, decos, Deco::Underline, inner);
                    }
                    Inline::SmallCaps(inner) => {
                        self.nested(&mut run, decos, Deco::SmallCaps, inner);
                    }
                    Inline::Strikeout(inner) => {
                        self.nested(&mut run, decos, Deco::Strikeout, inner);
                    }
                    Inline::Superscript(inner) => {
                        self.nested(&mut run, decos, Deco::Superscript, inner);
                    }
                    Inline::Subscript(inner) => {
                        self.nested(&mut run, decos, Deco::Subscript, inner);
                    }
                    Inline::Quoted(kind, inner) => {
                        self.flush_run(&mut run, decos);
                        let (open, close) = match kind {
                            QuoteType::DoubleQuote => ('\u{201C}', '\u{201D}'),
                            QuoteType::SingleQuote => ('\u{2018}', '\u{2019}'),
                        };
                        self.body.push(open);
                        self.walk(inner, decos);
                        self.body.push(close);
                    }
                    Inline::Code(_, text) => {
                        self.flush_run(&mut run, decos);
                        self.body
                            .push_str("<text:span text:style-name=\"Source_20_Text\">");
                        self.push_verbatim(text);
                        self.body.push_str("</text:span>");
                    }
                    Inline::Link(_, text, target) => {
                        self.flush_run(&mut run, decos);
                        self.link(text, target, decos);
                    }
                    Inline::Image(attr, alt, target) => {
                        self.flush_run(&mut run, decos);
                        self.image(attr, alt, target);
                    }
                    Inline::Note(blocks) => {
                        self.flush_run(&mut run, decos);
                        self.note(blocks);
                    }
                    Inline::Span(attr, inner) => {
                        self.flush_run(&mut run, decos);
                        self.span(attr, inner, decos);
                    }
                    Inline::Cite(_, inner) => {
                        self.flush_run(&mut run, decos);
                        self.walk(inner, decos);
                    }
                }
            }
            self.flush_run(&mut run, decos);
        });
    }

    fn nested<'i>(
        &mut self,
        run: &mut Vec<&'i Inline>,
        decos: &[Deco],
        add: Deco,
        inner: &'i [Inline],
    ) {
        self.flush_run(run, decos);
        let mut extended = decos.to_vec();
        extended.push(add);
        self.walk(inner, &extended);
    }

    /// Emits the accumulated run wrapped in one styled span (bare, when no decoration is active) and
    /// clears it. A run that renders to nothing contributes nothing.
    fn flush_run(&mut self, run: &mut Vec<&Inline>, decos: &[Deco]) {
        if run.is_empty() {
            return;
        }
        let start = self.body.len();
        self.render_run_content(run);
        run.clear();
        if self.body.len() == start || decos.is_empty() {
            return;
        }
        let style = self.run_style(decos);
        self.body
            .insert_str(start, &format!("<text:span text:style-name=\"{style}\">"));
        self.body.push_str("</text:span>");
    }

    /// Renders the plain inlines gathered into a run, collapsing breaking spaces the way a flowing
    /// paragraph does.
    fn render_run_content(&mut self, run: &[&Inline]) {
        let mut pending_space = false;
        for inline in run {
            if matches!(inline, Inline::Space | Inline::SoftBreak) {
                pending_space = true;
                continue;
            }
            if pending_space {
                self.body.push(' ');
                pending_space = false;
            }
            match inline {
                Inline::Str(text) => self.push_verbatim(text),
                Inline::LineBreak => self.body.push_str("<text:line-break />"),
                Inline::Math(kind, tex) => self.math(kind, tex),
                Inline::RawInline(_, text) => self.body.push_str(text),
                _ => {}
            }
        }
        if pending_space {
            self.body.push(' ');
        }
    }

    /// The style name a text run carries under an active set of decorations: a fixed named style for
    /// a lone named decoration, otherwise an automatic style registered on first use.
    fn run_style(&mut self, decos: &[Deco]) -> String {
        let mut key = decos.to_vec();
        key.sort_unstable();
        key.dedup();
        if let [only] = key.as_slice()
            && let Some(named) = only.named_style()
        {
            return named.to_string();
        }
        if let Some(index) = self
            .text_styles
            .iter()
            .position(|existing| *existing == key)
        {
            return format!("T{}", index + 1);
        }
        self.text_styles.push(key);
        format!("T{}", self.text_styles.len())
    }

    /// A span: an `id` becomes a bookmark bracketing the content, and a `custom-style` wraps the
    /// content in a named text span. When both are present the bookmark encloses the styled span, so
    /// the anchor survives alongside the styling rather than being dropped for it.
    fn span(&mut self, attr: &Attr, inner: &[Inline], decos: &[Deco]) {
        let anchored = !attr.id.is_empty();
        if anchored {
            self.body.push_str("<text:bookmark-start text:name=\"");
            escape_attribute(attr.id.as_str(), &mut self.body);
            self.body.push_str("\" />");
        }
        match custom_style(attr) {
            Some(name) => {
                self.body.push_str("<text:span text:style-name=\"");
                escape_attribute(name, &mut self.body);
                self.body.push_str("\">");
                self.walk(inner, decos);
                self.body.push_str("</text:span>");
            }
            None => self.walk(inner, decos),
        }
        if anchored {
            self.body.push_str("<text:bookmark-end text:name=\"");
            escape_attribute(attr.id.as_str(), &mut self.body);
            self.body.push_str("\" />");
        }
    }

    fn link(&mut self, text: &[Inline], target: &Target, decos: &[Deco]) {
        self.body
            .push_str("<text:a xlink:type=\"simple\" xlink:href=\"");
        let url = target.url.as_str();
        match parent_prefix_index(url) {
            Some(at) => {
                escape_attribute(url.get(..at).unwrap_or_default(), &mut self.body);
                self.body.push_str("../");
                escape_attribute(url.get(at..).unwrap_or(url), &mut self.body);
            }
            None => escape_attribute(url, &mut self.body),
        }
        self.body.push_str("\" office:name=\"");
        escape_attribute(target.title.as_str(), &mut self.body);
        self.body.push_str("\">");
        self.body
            .push_str("<text:span text:style-name=\"Definition\">");
        self.walk(text, decos);
        self.body.push_str("</text:span></text:a>");
    }

    fn note(&mut self, blocks: &[Block]) {
        let id = self.note_id;
        self.note_id += 1;
        let _ = write!(
            self.body,
            "<text:note text:id=\"ftn{id}\" text:note-class=\"footnote\">\
             <text:note-citation>{}</text:note-citation><text:note-body>",
            id + 1
        );
        let checkpoint = self.body.len();
        self.render_blocks(blocks, Some("Footnote"));
        if self.body.len() == checkpoint {
            self.paragraph("Footnote", &[]);
        }
        self.body.push_str("</text:note-body></text:note>");
    }

    /// Renders inline or display math as an embedded formula object: a drawing frame that references
    /// a `Formula-N/` sub-object holding the Presentation MathML. The inline form anchors as a
    /// character in the text flow; the display form anchors to its paragraph. Math that cannot be
    /// parsed degrades to its verbatim source set as text.
    fn math(&mut self, kind: &MathType, tex: &str) {
        let display = matches!(kind, MathType::DisplayMath);
        let Some(mathml) = crate::math::to_mathml(tex, display) else {
            escape_text(tex, &mut self.body);
            return;
        };
        let index = self.object_index;
        let (style, anchor) = if display {
            ("fr2", "paragraph")
        } else {
            ("fr1", "as-char")
        };
        let _ = write!(
            self.body,
            "<draw:frame draw:style-name=\"{style}\" text:anchor-type=\"{anchor}\">\
             <draw:object xlink:href=\"Formula-{index}/\" xlink:type=\"simple\" \
             xlink:show=\"embed\" xlink:actuate=\"onLoad\" /></draw:frame>"
        );
        self.formulas.push(Formula {
            index,
            mathml,
            text_mode: !display,
        });
        self.object_index += 2;
    }

    fn image(&mut self, attr: &Attr, alt: &[Inline], target: &Target) {
        match self.resolve_image(target.url.as_str()) {
            Some((bytes, mime)) => {
                let extension = extension_for_mime(&mime).to_string();
                let index = self.object_index;
                let ordinal = self.image_ordinal;
                let file_name = format!("{index}.{extension}");
                let ((width, height), density) = image_metrics(&bytes);
                let size = image_size(attr, width, height, density);
                let _ = write!(
                    self.body,
                    "<draw:frame draw:name=\"img{}\"{size}>\
                     <draw:image xlink:href=\"Pictures/{file_name}\" xlink:type=\"simple\" \
                     xlink:show=\"embed\" xlink:actuate=\"onLoad\" /></draw:frame>",
                    ordinal + 1
                );
                self.images.push(Image {
                    file_name,
                    mime,
                    bytes,
                });
                self.object_index += 1;
                self.image_ordinal += 1;
            }
            None => {
                if !alt.is_empty() {
                    self.walk(alt, &[Deco::Emph]);
                }
            }
        }
    }

    /// Resolves an image reference to its bytes and MIME type, from the media bag or an inline data
    /// URI. Returns `None` when neither carries the resource, so the caller degrades to the alt text.
    fn resolve_image(&self, url: &str) -> Option<(Vec<u8>, String)> {
        if let Some(item) = self.options.media.get(url) {
            let mime = item
                .mime
                .clone()
                .or_else(|| image_mime_for_extension(url).map(str::to_string))
                .unwrap_or_else(|| "application/octet-stream".to_string());
            return Some((item.bytes.clone(), mime));
        }
        decode_data_uri(url)
    }

    pub(super) fn paragraph(&mut self, style: &str, inlines: &[Inline]) {
        self.body.push_str("<text:p text:style-name=\"");
        escape_attribute(style, &mut self.body);
        self.body.push_str("\">");
        self.inlines(inlines);
        self.body.push_str("</text:p>");
    }

    /// Appends verbatim text, preserving space runs (as `<text:s>`) and tabs (as `<text:tab>`), so
    /// indentation and internal spacing survive the layout engine's whitespace collapsing.
    pub(super) fn push_verbatim(&mut self, text: &str) {
        let mut chars = text.chars().peekable();
        let mut at_start = true;
        while let Some(ch) = chars.next() {
            match ch {
                ' ' => {
                    let mut run = 1usize;
                    while chars.peek() == Some(&' ') {
                        chars.next();
                        run += 1;
                    }
                    if at_start || run > 1 {
                        let _ = write!(self.body, "<text:s text:c=\"{run}\" />");
                    } else {
                        self.body.push(' ');
                    }
                }
                '\t' => self.body.push_str("<text:tab />"),
                '&' => self.body.push_str("&amp;"),
                '<' => self.body.push_str("&lt;"),
                '>' => self.body.push_str("&gt;"),
                other if is_xml_char(other) => self.body.push(other),
                _ => {}
            }
            at_start = false;
        }
    }
}
