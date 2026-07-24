//! Directive and explicit-markup construction during block parsing.

use super::definitions::{RoleDef, included_blocks};
use super::directives::{
    blank_separated, capitalize, class_div, class_list, code_attr, common_options,
    directive_content, figure_attr, image_classes, image_parts, options_div, split_directive,
    to_plain,
};
use super::markers::{Explicit, classify_explicit, explicit_body, explicit_extent};
use super::{DEFAULT_ROLE, MAX_INCLUDE_DEPTH, PENDING_CLASS, Parser, indent_of, line_at};
use carta_ast::{Attr, Block, Caption, Format, Inline, MathType, Target};

impl Parser<'_> {
    // --- explicit markup ---

    pub(super) fn explicit(
        &mut self,
        lines: &[String],
        start: usize,
        out: &mut Vec<Block>,
    ) -> usize {
        let line = line_at(lines, start);
        let indent = indent_of(line);
        let end = explicit_extent(lines, start, indent);
        if let Some(Explicit::Directive(name)) = classify_explicit(line) {
            self.directive(&name, lines, start, end, out);
        }
        end
    }

    #[allow(clippy::too_many_lines)]
    fn directive(
        &mut self,
        name: &str,
        lines: &[String],
        start: usize,
        end: usize,
        out: &mut Vec<Block>,
    ) {
        let first = line_at(lines, start).trim_start();
        let after = first
            .strip_prefix("..")
            .unwrap_or(first)
            .trim_start()
            .strip_prefix(name)
            .and_then(|r| r.strip_prefix("::"))
            .unwrap_or("");
        let prefix_len = line_at(lines, start).len() - after.len();
        let body = explicit_body(lines, start, end, prefix_len);
        let (argument, options, content) = split_directive(&body);

        match name {
            "raw" => {
                out.push(Block::RawBlock(
                    Format(argument.trim().into()),
                    content.join("\n").into(),
                ));
            }
            "code" | "code-block" | "sourcecode" => {
                let attr = code_attr(&argument, &options);
                let mut text = content.join("\n");
                while text.ends_with('\n') {
                    text.pop();
                }
                out.push(Block::CodeBlock(Box::new(attr), text.into()));
            }
            "math" => {
                let mut equations = Vec::new();
                if !argument.trim().is_empty() {
                    equations.push(argument.trim().to_string());
                }
                equations.extend(blank_separated(&content));
                let math: Vec<Inline> = equations
                    .into_iter()
                    .map(|eq| Inline::Math(MathType::DisplayMath, eq.into()))
                    .collect();
                let (id, classes, attributes) = common_options(&options);
                // Options (`:label:`, `:nowrap:`, …) attach to the equation group via a wrapping span.
                let inlines = if id.is_empty() && classes.is_empty() && attributes.is_empty() {
                    math
                } else {
                    vec![Inline::Span(
                        Box::new(Attr {
                            id: id.into(),
                            classes: classes.into_iter().map(Into::into).collect(),
                            attributes: attributes
                                .into_iter()
                                .map(|(k, v)| (k.into(), v.into()))
                                .collect(),
                        }),
                        math,
                    )]
                };
                out.push(Block::Para(inlines));
            }
            "image" => {
                let (mut attr, mut alt, url) = image_parts(&argument, &options);
                attr.classes = image_classes(&options)
                    .into_iter()
                    .map(Into::into)
                    .collect();
                if alt.is_empty() {
                    alt = vec![Inline::Str("image".into())];
                }
                let image = Inline::Image(
                    Box::new(attr),
                    alt,
                    Box::new(Target {
                        url: url.into(),
                        title: carta_ast::Text::default(),
                    }),
                );
                out.push(Block::Para(vec![Self::wrap_target(image, &options)]));
            }
            "figure" => out.push(self.figure(&argument, &options, &content)),
            "note" | "warning" | "attention" | "caution" | "danger" | "error" | "hint"
            | "important" | "tip" => {
                let title = capitalize(name);
                let mut blocks = vec![Block::Div(
                    Box::new(Attr {
                        id: carta_ast::Text::default(),
                        classes: vec!["title".into()],
                        attributes: Vec::new(),
                    }),
                    vec![Block::Para(vec![Inline::Str(title.into())])],
                )];
                blocks.extend(self.blocks(&directive_content(&body)));
                out.push(options_div(name, &options, blocks));
            }
            "admonition" => {
                let mut blocks = Vec::new();
                if !argument.trim().is_empty() {
                    blocks.push(Block::Para(self.inlines(argument.trim())));
                }
                blocks.extend(self.blocks(&content));
                out.push(class_div(vec!["admonition".to_string()], blocks));
            }
            "topic" | "sidebar" => {
                let mut blocks = Vec::new();
                if !argument.trim().is_empty() {
                    // Sidebar: subtitle joins title after a colon; topic: title alone. The subtitle
                    // is also kept as a division attribute.
                    let subtitle = options.iter().find(|(k, _)| k == "subtitle");
                    let title = match (name, subtitle) {
                        ("sidebar", Some((_, subtitle))) => {
                            format!("{}: {}", argument.trim(), subtitle.trim())
                        }
                        _ => argument.trim().to_string(),
                    };
                    blocks.push(Block::Para(vec![Inline::Strong(self.inlines(&title))]));
                }
                blocks.extend(self.blocks(&content));
                out.push(options_div(name, &options, blocks));
            }
            "rubric" => {
                out.push(Block::Para(vec![Inline::Strong(
                    self.inlines(argument.trim()),
                )]));
            }
            "container" => {
                let mut classes = vec!["container".to_string()];
                classes.extend(argument.split_whitespace().map(str::to_string));
                out.push(class_div(classes, self.blocks(&content)));
            }
            "epigraph" | "highlights" | "pull-quote" => {
                out.push(Block::BlockQuote(self.blocks(&content)));
            }
            "compound" => out.extend(self.blocks(&content)),
            "csv-table" => self.csv_table(&argument, &options, &content, out),
            "list-table" => self.list_table(&argument, &options, &content, out),
            "class" => {
                let classes: Vec<String> =
                    argument.split_whitespace().map(str::to_string).collect();
                if content.is_empty() {
                    // Apply the classes to the next sibling block via a marker the loop unwraps.
                    let mut marker = vec![PENDING_CLASS.to_string()];
                    marker.extend(classes);
                    out.push(class_div(marker, Vec::new()));
                } else {
                    out.push(class_div(classes, self.blocks(&content)));
                }
            }
            "line-block" => out.push(self.line_block_directive(&content)),
            "table" => self.table_directive(&argument, &options, &content, out),
            // A role definition configures inline interpretation; it produces no block of its own.
            "role" => self.register_role(&argument, &options),
            "default-role" => {
                let selected = argument.trim();
                self.default_role = if selected.is_empty() {
                    DEFAULT_ROLE.to_string()
                } else {
                    selected.to_string()
                };
            }
            // Splices the parsed content of an external file; an unreadable file contributes nothing.
            "include" => {
                if self.include_depth < MAX_INCLUDE_DEPTH
                    && let Some(blocks) =
                        included_blocks(argument.trim(), self.ext, self.include_depth + 1)
                {
                    out.extend(blocks);
                }
            }
            _ => {
                let mut blocks = Vec::new();
                if !argument.trim().is_empty() {
                    blocks.push(Block::Para(self.inlines(argument.trim())));
                }
                blocks.extend(self.blocks(&content));
                out.push(options_div(name, &options, blocks));
            }
        }
    }

    /// Record a `role` directive: an `name(base)` argument names the role and the base role it
    /// inherits, while options supply the classes (`:class:`), the raw output format (`:format:`),
    /// and the highlighting language (`:language:`) the role carries.
    fn register_role(&mut self, argument: &str, options: &[(String, String)]) {
        let argument = argument.trim();
        let (name, base) = match argument.split_once('(') {
            Some((name, rest)) => (
                name.trim(),
                Some(rest.trim_end_matches(')').trim().to_string()),
            ),
            None => (argument, None),
        };
        if name.is_empty() {
            return;
        }
        let base = base.filter(|b| !b.is_empty());
        let classes = class_list(options, "class");
        let option_value = |key: &str| {
            options
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        self.custom_roles.insert(
            name.to_string(),
            RoleDef {
                base,
                classes,
                format: option_value("format"),
                language: option_value("language"),
            },
        );
    }

    fn wrap_target(image: Inline, options: &[(String, String)]) -> Inline {
        if let Some((_, url)) = options.iter().find(|(k, _)| k == "target") {
            Inline::Link(
                Box::default(),
                vec![image],
                Box::new(Target {
                    url: url.clone().into(),
                    title: carta_ast::Text::default(),
                }),
            )
        } else {
            image
        }
    }

    fn figure(
        &mut self,
        argument: &str,
        options: &[(String, String)],
        content: &[String],
    ) -> Block {
        let (img_attr, alt, url) = image_parts(argument, options);
        let inner = self.blocks(content);
        let mut caption = Caption::default();
        let mut caption_inlines = Vec::new();
        let mut iter = inner.into_iter();
        if let Some(first) = iter.next() {
            let plain = to_plain(first);
            if let Block::Plain(inlines) = &plain {
                caption_inlines.clone_from(inlines);
            }
            // First body block is the caption; further blocks are the legend, joined to the caption.
            caption.long = vec![plain];
            caption.long.extend(iter);
        }
        // The image description defaults to the figure's caption when no explicit alt is given.
        let description = if alt.is_empty() { caption_inlines } else { alt };
        let image = Inline::Image(
            Box::new(img_attr),
            description,
            Box::new(Target {
                url: url.into(),
                title: carta_ast::Text::default(),
            }),
        );
        let body = vec![Block::Plain(vec![image])];
        Block::Figure(Box::new(figure_attr(options)), Box::new(caption), body)
    }

    /// A `line-block` directive: each body line becomes one line of the block, with a blank body line
    /// rendering as an empty line.
    fn line_block_directive(&mut self, content: &[String]) -> Block {
        let mut end = content.len();
        while end > 0 && content.get(end - 1).is_some_and(|l| l.trim().is_empty()) {
            end -= 1;
        }
        let lines = content
            .get(..end)
            .unwrap_or(&[])
            .iter()
            .map(|line| self.inlines(line.trim()))
            .collect();
        Block::LineBlock(lines)
    }

    /// A `table` directive: its body is an ordinary table whose caption is taken from the directive's
    /// argument.
    fn table_directive(
        &mut self,
        argument: &str,
        _options: &[(String, String)],
        content: &[String],
        out: &mut Vec<Block>,
    ) {
        let mut blocks = self.blocks(content);
        let argument = argument.trim();
        if !argument.is_empty() {
            let caption = self.inlines(argument);
            if let Some(Block::Table(table)) =
                blocks.iter_mut().find(|b| matches!(b, Block::Table(_)))
            {
                table.caption = Caption {
                    short: None,
                    long: vec![Block::Plain(caption)],
                };
            }
        }
        out.extend(blocks);
    }

    /// The trailing `citations` division gathering every citation definition, or `None` when the
    /// document defines no citations.
    pub(super) fn citation_block(&mut self) -> Option<Block> {
        if self.defs.citations.is_empty() {
            return None;
        }
        let items = self
            .defs
            .citations
            .iter()
            .map(|(label, body)| {
                let term = vec![Inline::Span(
                    Box::new(Attr {
                        id: label.clone().into(),
                        classes: vec!["citation-label".into()],
                        attributes: Vec::new(),
                    }),
                    vec![Inline::Str(label.clone().into())],
                )];
                (term, vec![self.blocks(body)])
            })
            .collect();
        Some(Block::Div(
            Box::new(Attr {
                id: "citations".into(),
                classes: Vec::new(),
                attributes: Vec::new(),
            }),
            vec![Block::DefinitionList(items)],
        ))
    }
}
