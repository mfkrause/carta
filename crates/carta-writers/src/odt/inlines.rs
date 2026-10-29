//! Inline-level rendering for the ODT writer.

use std::fmt::Write as _;

use carta_ast::{Attr, Block, Inline, MathType, QuoteType, Target};
use carta_core::container::xml::{escape_attribute, is_xml_char};
use carta_core::media::{decode_data_uri, extension_for_mime, image_mime_for_extension};

use super::helpers::{attr_value, custom_style, is_opendocument, parent_prefix_index};
use super::media::{image_metrics, image_size};
use super::{
    Builder, Deco, Formula, Image, Language, ReferenceKind, STACK_RED_ZONE, STACK_SEGMENT,
    clone_decorations,
};

impl Builder<'_> {
    pub(super) fn inlines(&mut self, inlines: &[Inline]) {
        let mut decorations = Vec::new();
        for language in &self.languages {
            carta_core::budget::charge(
                std::mem::size_of::<Deco>().saturating_add(language.clone_size()),
            );
            if carta_core::budget::exhausted() {
                return;
            }
            decorations.push(Deco::Language(language.clone()));
        }
        self.walk(inlines, &decorations);
    }

    /// Renders an inline sequence under a set of active decorations. Plain content accumulates into a
    /// run emitted as a single styled span; a formatting node extends the decoration set over its
    /// content; a structural node (link, span, note, …) breaks the run and is emitted in place,
    /// carrying the active decorations into any content of its own.
    fn walk<'i>(&mut self, inlines: &'i [Inline], decos: &[Deco]) {
        stacker::maybe_grow(STACK_RED_ZONE, STACK_SEGMENT, || {
            let mut run: Vec<&'i Inline> = Vec::new();
            for inline in inlines {
                if carta_core::budget::exhausted() {
                    break;
                }
                match inline {
                    Inline::Str(_) | Inline::Space | Inline::SoftBreak | Inline::LineBreak => {
                        run.push(inline);
                    }
                    Inline::Math(kind, text) => {
                        self.flush_run(&mut run, decos);
                        self.math(kind, text, decos);
                    }
                    Inline::RawInline(format, text) if is_opendocument(format) => {
                        self.flush_run(&mut run, decos);
                        self.body.push_str(text);
                    }
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
        if decos.contains(&add) {
            self.walk(inner, decos);
        } else if let Some(mut extended) = clone_decorations(decos) {
            extended.push(add);
            self.walk(inner, &extended);
        }
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
        let Some(style) = self.run_style(decos) else {
            self.body.truncate(start);
            return;
        };
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
                _ => {}
            }
        }
        if pending_space {
            self.body.push(' ');
        }
    }

    /// The style name a text run carries under an active set of decorations: a fixed named style for
    /// a lone named decoration, otherwise an automatic style registered on first use.
    fn run_style(&mut self, decos: &[Deco]) -> Option<String> {
        let mut key = clone_decorations(decos)?;
        key.sort_unstable();
        key.dedup();
        if let [only] = key.as_slice()
            && let Some(named) = only.named_style()
        {
            return Some(named.to_string());
        }
        if let Some(index) = self.text_styles.get(&key) {
            return Some(format!("T{index}"));
        }
        let index = self.text_styles.len() + 1;
        self.text_styles.insert(key, index);
        Some(format!("T{index}"))
    }

    /// Render bookmarks, custom character styles, and bare `mark` spans with active decorations.
    fn span(&mut self, attr: &Attr, inner: &[Inline], decos: &[Deco]) {
        let language = attr_value(attr, "lang")
            .and_then(Language::parse)
            .map(Deco::Language);
        let extended = if let Some(language) = language.filter(|language| !decos.contains(language))
        {
            let Some(mut extended) = clone_decorations(decos) else {
                return;
            };
            extended.push(language);
            Some(extended)
        } else {
            None
        };
        let decos = extended.as_deref().unwrap_or(decos);
        let anchored = !attr.id.is_empty();
        if anchored {
            self.body.push_str("<text:bookmark-start text:name=\"");
            escape_attribute(attr.id.as_str(), &mut self.body);
            self.body.push_str("\" />");
        }
        let style = if !anchored
            && attr.attributes.is_empty()
            && matches!(attr.classes.as_slice(), [class] if class.as_str() == "mark")
        {
            Some("Highlighted")
        } else {
            custom_style(attr)
        };
        match style {
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
        if let Some((identifier, kind)) = target.url.strip_prefix('#').and_then(|identifier| {
            self.references
                .get(identifier)
                .copied()
                .map(|kind| (identifier, kind))
        }) {
            self.reference(kind, identifier, text, decos);
            return;
        }
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

    fn reference(
        &mut self,
        kind: ReferenceKind,
        identifier: &str,
        text: &[Inline],
        decos: &[Deco],
    ) {
        let (element, name_format, number_format) = match kind {
            ReferenceKind::Bookmark => ("text:bookmark-ref", "text", "number"),
            ReferenceKind::Sequence => ("text:sequence-ref", "caption", "value"),
        };
        let show_name = self
            .options
            .extensions
            .contains(carta_core::Extension::CrossReferenceName);
        let show_number = self
            .options
            .extensions
            .contains(carta_core::Extension::CrossReferenceNumber);
        for (enabled, format, show_text) in [
            (show_number, number_format, false),
            (show_name, name_format, true),
        ] {
            if !enabled {
                continue;
            }
            if show_text && show_number {
                self.body.push_str("<text:s />");
            }
            let _ = write!(
                self.body,
                "<{element} text:reference-format=\"{format}\" text:ref-name=\""
            );
            escape_attribute(identifier, &mut self.body);
            self.body.push_str("\">");
            if show_text {
                self.walk(text, decos);
            }
            let _ = write!(self.body, "</{element}>");
        }
    }

    fn note(&mut self, blocks: &[Block]) {
        let id = self.note_id;
        let _ = write!(
            self.body,
            "<text:note text:id=\"ftn{id}\" text:note-class=\"footnote\">\
             <text:note-citation>{}</text:note-citation><text:note-body>",
            id + 1
        );
        let checkpoint = self.body.len();
        let languages = std::mem::take(&mut self.languages);
        let quote_depth = std::mem::replace(&mut self.quote_depth, 0);
        self.render_blocks(blocks, Some("Footnote"));
        self.languages = languages;
        self.quote_depth = quote_depth;
        // Reserve the outer note's number before rendering nested notes.
        self.note_id += 1;
        if self.body.len() == checkpoint {
            self.body.truncate(checkpoint - "<text:note-body>".len());
            self.body.push_str("<text:note-body />");
        } else {
            self.body.push_str("</text:note-body>");
        }
        self.body.push_str("</text:note>");
    }

    /// Renders inline or display math as an embedded formula object: a drawing frame that references
    /// a `Formula-N/` sub-object holding the Presentation MathML. The inline form anchors as a
    /// character in the text flow; the display form anchors to its paragraph. Math that cannot be
    /// parsed degrades to its verbatim source set as text.
    fn math(&mut self, kind: &MathType, tex: &str, decos: &[Deco]) {
        let display = matches!(kind, MathType::DisplayMath);
        let Some(mathml) = crate::math::to_mathml(tex, display) else {
            let delimiter = if display { "$$" } else { "$" };
            self.walk(
                &[Inline::Str(format!("{delimiter}{tex}{delimiter}").into())],
                decos,
            );
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
                let index = self.object_index;
                let ordinal = self.image_ordinal;
                let file_name = if mime.is_empty() {
                    index.to_string()
                } else {
                    format!("{index}.{}", extension_for_mime(&mime))
                };
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
        let data_mime = url
            .strip_prefix("data:")
            .and_then(|header| header.split([';', ',']).next());
        let (bytes, mime) = if let Some(item) = self.options.media.get(url) {
            let mime = item
                .mime
                .clone()
                .or_else(|| data_mime.map(str::to_string))
                .or_else(|| image_mime_for_extension(url).map(str::to_string))
                .unwrap_or_else(|| "application/octet-stream".to_string());
            (item.bytes.clone(), mime)
        } else {
            let (bytes, mime) = decode_data_uri(url)?;
            (bytes, data_mime.map_or(mime, str::to_string))
        };
        Some((bytes, mime))
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
        let mut after_tab = false;
        while let Some(ch) = chars.next() {
            match ch {
                ' ' => {
                    let mut run = 1usize;
                    while chars.peek() == Some(&' ') {
                        chars.next();
                        run += 1;
                    }
                    if !at_start && !after_tab {
                        self.body.push(' ');
                        run -= 1;
                    }
                    if run > 0 {
                        let _ = write!(self.body, "<text:s text:c=\"{run}\" />");
                    }
                }
                '\t' => self.body.push_str("<text:tab />"),
                '&' => self.body.push_str("&amp;"),
                '<' => self.body.push_str("&lt;"),
                '>' => self.body.push_str("&gt;"),
                '"' => self.body.push_str("&quot;"),
                other if is_xml_char(other) => self.body.push(other),
                _ => {}
            }
            after_tab = ch == '\t' && !at_start;
            at_start = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn language_attr(tag: &str) -> Box<Attr> {
        Box::new(Attr {
            attributes: vec![("lang".into(), tag.into())],
            ..Attr::default()
        })
    }

    #[test]
    fn languages_share_run_styles_and_stop_at_note_bodies() {
        let options = carta_core::WriterOptions::default();
        let mut builder = Builder::new(&options);
        let content = vec![
            Inline::Strong(vec![Inline::Span(
                language_attr("FR_ca"),
                vec![Inline::Str("Text".into())],
            )]),
            Inline::Note(vec![Block::Para(vec![Inline::Str("Note".into())])]),
            Inline::Str("After".into()),
        ];
        builder.render_blocks(
            &[
                Block::Div(language_attr("fr-CA"), vec![Block::Para(content)]),
                Block::Para(vec![Inline::Str("Outside".into())]),
            ],
            None,
        );
        let xml = builder.finish_content();
        assert_eq!(builder.text_styles.len(), 2);
        assert!(xml.contains("fo:country=\"CA\" fo:font-weight=\"bold\" fo:language=\"fr\""));
        assert!(xml.contains("<text:p text:style-name=\"Footnote\">Note</text:p>"));
        assert!(xml.contains("<text:span text:style-name=\"T2\">After</text:span>"));
        assert!(xml.contains(">Outside</text:p>"));
    }

    #[test]
    fn raw_markup_keeps_its_own_style_inside_language_runs() {
        let options = carta_core::WriterOptions::default();
        let mut builder = Builder::new(&options);
        builder.span(
            &language_attr("fr"),
            &[
                Inline::Str("Before".into()),
                Inline::RawInline(
                    carta_ast::Format("opendocument".into()),
                    "<text:span>Raw</text:span>".into(),
                ),
                Inline::Str("After".into()),
            ],
            &[],
        );
        assert_eq!(
            builder.body,
            "<text:span text:style-name=\"T1\">Before</text:span><text:span>Raw</text:span><text:span text:style-name=\"T1\">After</text:span>"
        );
    }

    #[test]
    fn formulas_keep_their_object_style_and_failed_math_keeps_the_language() {
        let options = carta_core::WriterOptions::default();
        let mut builder = Builder::new(&options);
        builder.span(
            &language_attr("fr"),
            &[
                Inline::Math(MathType::InlineMath, "x".into()),
                Inline::Math(MathType::InlineMath, "\\frac{".into()),
            ],
            &[],
        );
        assert_eq!(builder.formulas.len(), 1);
        assert!(builder.body.starts_with("<draw:frame"));
        assert!(
            builder
                .body
                .ends_with("<text:span text:style-name=\"T1\">$\\frac{$</text:span>")
        );
    }

    #[test]
    fn deep_repeated_decoration_does_not_exhaust_the_budget() {
        let mut inline = Inline::Str("Text".into());
        for _ in 0..10_000 {
            inline = Inline::Emph(vec![inline]);
        }
        let document = carta_ast::Document {
            blocks: vec![Block::Para(vec![inline])],
            ..Default::default()
        };
        assert!(super::super::write_odt(&document, &carta_core::WriterOptions::default()).is_ok());
    }

    #[test]
    fn excessive_language_combinations_return_a_resource_error() {
        let mut inline = Inline::Str("Text".into());
        for index in 0..1_000 {
            inline = Inline::Span(language_attr(&format!("en-x-{index}")), vec![inline]);
        }
        let document = carta_ast::Document {
            blocks: vec![Block::Para(vec![inline])],
            ..Default::default()
        };
        assert!(matches!(
            super::super::write_odt(&document, &carta_core::WriterOptions::default()),
            Err(carta_core::Error::OutputTooLarge)
        ));
    }

    #[test]
    fn untyped_data_images_keep_their_bytes_and_package_target() {
        let source = "data:;base64,YQ==";
        for cached in [false, true] {
            let mut options = carta_core::WriterOptions::default();
            let bytes = if cached {
                b"cached".as_slice()
            } else {
                b"a".as_slice()
            };
            if cached {
                let mut media = carta_core::media::MediaBag::new();
                media.insert(source, None, bytes.to_vec());
                options.media = std::sync::Arc::new(media);
            }
            let mut builder = Builder::new(&options);
            builder.image(
                &Attr::default(),
                &[],
                &Target {
                    url: source.into(),
                    title: Default::default(),
                },
            );
            let image = builder.images.first().expect("image is retained");
            assert_eq!(image.file_name, "0");
            assert_eq!(image.bytes, bytes);
            assert!(builder.body.contains("xlink:href=\"Pictures/0\""));
            assert!(!crate::odt::meta::manifest_xml(&builder.images, &[]).contains("Pictures/"));
        }
    }

    #[test]
    fn marked_spans_keep_outer_decorations() {
        let options = carta_core::WriterOptions::default();
        let mut builder = Builder::new(&options);
        let attr = Attr {
            classes: vec!["mark".into()],
            ..Attr::default()
        };
        builder.span(&attr, &[Inline::Str("Text".into())], &[Deco::Emph]);
        assert_eq!(
            builder.body,
            "<text:span text:style-name=\"Highlighted\"><text:span text:style-name=\"Emphasis\">Text</text:span></text:span>"
        );
    }
}
