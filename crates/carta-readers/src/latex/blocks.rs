//! Block-level LaTeX constructs: sectioning, environments, lists, verbatim, figures, and tables.

use carta_ast::{
    Attr, Block, Caption, Format, Inline, ListAttributes, ListNumberDelim, ListNumberStyle,
    MathType, MetaValue, slug, slug_gfm, to_plain_text,
};
use carta_core::Extension;

use crate::heading_ids::IdScheme;

use super::support::demote_image_para;
use super::tables::{build_table, parse_column_spec, parse_key_values};
use super::{InlineStop, Parser, Stop};

impl Parser {
    // --- Sectioning ------------------------------------------------------------------------------

    pub(super) fn parse_section(&mut self, intrinsic: i32) -> Block {
        self.consume_control_word();
        let starred = self.cur() == Some('*');
        if starred {
            self.bump();
            while matches!(self.cur(), Some(' ' | '\t')) {
                self.bump();
            }
        }
        // An optional `[short title]` is ignored.
        let _ = self.read_optional_raw();
        let mut label = None;
        let title = self.parse_group_inlines_capturing_label(&mut label);
        // A `\label` immediately following the heading names it too.
        self.skip_block_ws();
        if let Some(id) = self.peek_env_arg_after_label() {
            label = Some(id);
        }

        let level = (intrinsic - self.base_level + 1).max(1);
        let id = match label {
            Some(id) => {
                self.ids.reserve_native(&id);
                id
            }
            None => self.assign_id(&to_plain_text(&title)),
        };
        let mut classes = Vec::new();
        if starred {
            classes.push("unnumbered".into());
        }
        Block::Header(
            level,
            Box::new(Attr {
                id: id.into(),
                classes,
                attributes: Vec::new(),
            }),
            title,
        )
    }

    /// If the cursor is at `\label{id}`, consumes it and returns the identifier.
    pub(super) fn peek_env_arg_after_label(&mut self) -> Option<String> {
        if self.at_control_word("label") {
            self.consume_control_word();
            return self.read_group_raw();
        }
        None
    }

    /// Parses a braced inline group, capturing the identifier of any `\label` inside it into `label`.
    pub(super) fn parse_group_inlines_capturing_label(
        &mut self,
        label: &mut Option<String>,
    ) -> Vec<Inline> {
        if self.cur() != Some('{') {
            return Vec::new();
        }
        self.bump();
        let inlines = self.parse_inlines(InlineStop::Group);
        if self.cur() == Some('}') {
            self.bump();
        }
        // a `\label` renders as an empty span with a `label` attribute; pull it out as the header id
        let mut kept = Vec::new();
        for inline in inlines {
            if let Inline::Span(attr, content) = &inline
                && content.is_empty()
                && attr.attributes.iter().any(|(k, _)| k == "label")
            {
                *label = Some(attr.id.to_string());
                continue;
            }
            kept.push(inline);
        }
        kept
    }

    /// Derives a heading identifier from its title text. The slug shape follows the active extension,
    /// but a section always disambiguates natively: an empty slug becomes `section` and a repeat
    /// increments a numeric suffix until unused (also avoiding any reserved `\label`).
    pub(super) fn assign_id(&mut self, text: &str) -> String {
        let Some(scheme) = IdScheme::select(self.ext, false) else {
            return String::new();
        };
        let base = match scheme {
            IdScheme::Plain => slug(text),
            IdScheme::Gfm => slug_gfm(text),
        };
        self.ids.assign_native(base)
    }

    // --- Environments ----------------------------------------------------------------------------

    pub(super) fn parse_environment(&mut self, env: &str) -> Vec<Block> {
        self.consume_env_marker("\\begin");
        match env {
            "itemize" | "enumerate" => {
                let _ = self.read_optional_raw();
                vec![self.parse_list(env)]
            }
            "description" => {
                let _ = self.read_optional_raw();
                vec![self.parse_description()]
            }
            "quote" | "quotation" | "verse" => {
                let inner = self.parse_blocks(&Stop::Env(env));
                self.consume_env_marker("\\end");
                vec![Block::BlockQuote(inner)]
            }
            "center" | "flushleft" | "flushright" => {
                let inner = self.parse_blocks(&Stop::Env(env));
                self.consume_env_marker("\\end");
                vec![Block::Div(
                    Box::new(Attr {
                        id: carta_ast::Text::default(),
                        classes: vec![env.into()],
                        attributes: Vec::new(),
                    }),
                    inner,
                )]
            }
            "minipage" => {
                // Positional options precede the mandatory width; none affect the content.
                while self.read_optional_raw().is_some() {}
                let _ = self.read_group_raw();
                let inner = self.parse_blocks(&Stop::Env(env));
                self.consume_env_marker("\\end");
                vec![Block::Div(
                    Box::new(Attr {
                        id: carta_ast::Text::default(),
                        classes: vec!["minipage".into()],
                        attributes: Vec::new(),
                    }),
                    inner,
                )]
            }
            "verbatim" | "verbatim*" | "Verbatim" | "lstlisting" | "minted" | "alltt"
            | "lstinputlisting" => self.parse_verbatim_env(env),
            "comment" => {
                self.skip_to_end_env(env);
                Vec::new()
            }
            "figure" | "figure*" | "wrapfigure" | "SCfigure" | "marginfigure" => {
                vec![self.parse_figure(env)]
            }
            "table" | "table*" => self.parse_table_float(env),
            "tabular" | "tabular*" | "tabularx" | "array" | "longtable" | "supertabular"
            | "tabulary" => {
                vec![self.parse_tabular(env)]
            }
            "abstract" => {
                let inner = self.parse_blocks(&Stop::Env(env));
                self.consume_env_marker("\\end");
                self.meta
                    .insert("abstract".to_owned(), MetaValue::MetaBlocks(inner));
                Vec::new()
            }
            "document" => {
                let inner = self.parse_blocks(&Stop::Env(env));
                self.consume_env_marker("\\end");
                inner
            }
            _ => {
                if self.ext.contains(Extension::RawTex) {
                    self.parse_raw_env(env)
                } else {
                    let inner = self.parse_blocks(&Stop::Env(env));
                    self.consume_env_marker("\\end");
                    vec![Block::Div(
                        Box::new(Attr {
                            id: carta_ast::Text::default(),
                            classes: vec![env.into()],
                            attributes: Vec::new(),
                        }),
                        inner,
                    )]
                }
            }
        }
    }

    /// Captures an unknown environment verbatim as a raw LaTeX block (under `raw_tex`).
    pub(super) fn parse_raw_env(&mut self, env: &str) -> Vec<Block> {
        let mut raw = format!("\\begin{{{env}}}");
        while !self.eof() {
            if self.at_end_env(env) {
                break;
            }
            if let Some(c) = self.bump() {
                raw.push(c);
            }
        }
        raw.push_str("\\end{");
        raw.push_str(env);
        raw.push('}');
        self.consume_env_marker("\\end");
        vec![Block::RawBlock(Format("latex".into()), raw.into())]
    }

    /// Skips to and consumes the matching `\end{env}`.
    pub(super) fn skip_to_end_env(&mut self, env: &str) {
        while !self.eof() {
            if self.at_end_env(env) {
                break;
            }
            self.bump();
        }
        self.consume_env_marker("\\end");
    }

    // --- Lists -----------------------------------------------------------------------------------

    pub(super) fn parse_list(&mut self, env: &str) -> Block {
        let items = self.parse_items(env);
        if env == "enumerate" {
            Block::OrderedList(
                ListAttributes {
                    start: 1,
                    style: ListNumberStyle::DefaultStyle,
                    delim: ListNumberDelim::DefaultDelim,
                },
                items,
            )
        } else {
            Block::BulletList(items)
        }
    }

    /// Reads the `\item` entries of an itemize/enumerate environment.
    pub(super) fn parse_items(&mut self, env: &str) -> Vec<Vec<Block>> {
        let mut items: Vec<Vec<Block>> = Vec::new();
        loop {
            self.skip_block_ws();
            if self.eof() || self.at_end_env(env) {
                break;
            }
            if self.at_control_word("item") {
                self.consume_control_word();
                let _ = self.read_optional_raw(); // custom marker, dropped
                let blocks = self.parse_blocks(&Stop::Item(env));
                items.push(blocks);
            } else if !self.advance_over_stray() {
                break;
            }
        }
        self.consume_env_marker("\\end");
        items
    }

    pub(super) fn parse_description(&mut self) -> Block {
        let mut entries: Vec<(Vec<Inline>, Vec<Vec<Block>>)> = Vec::new();
        loop {
            self.skip_block_ws();
            if self.eof() || self.at_end_env("description") {
                break;
            }
            if self.at_control_word("item") {
                self.consume_control_word();
                let term = self.read_optional_inlines().unwrap_or_default();
                let blocks = self.parse_blocks(&Stop::Item("description"));
                entries.push((term, vec![blocks]));
            } else if !self.advance_over_stray() {
                break;
            }
        }
        self.consume_env_marker("\\end");
        Block::DefinitionList(entries)
    }

    // --- Verbatim --------------------------------------------------------------------------------

    pub(super) fn parse_verbatim_env(&mut self, env: &str) -> Vec<Block> {
        let mut classes = Vec::new();
        let mut attributes = Vec::new();
        // `lstlisting`/`Verbatim` take `[key=value,…]` options; `minted` takes `{language}`.
        if matches!(env, "lstlisting" | "Verbatim" | "lstinputlisting") {
            if let Some(opts) = self.read_optional_raw() {
                for (k, v) in parse_key_values(&opts) {
                    if k == "language" && !v.is_empty() {
                        classes.push(v.to_lowercase().into());
                    }
                    attributes.push((k, v));
                }
            }
        } else if env == "minted" {
            let _ = self.read_optional_raw();
            if let Some(lang) = self.read_group_raw()
                && !lang.is_empty()
            {
                classes.push(lang.to_lowercase().into());
            }
        }
        let content = self.read_verbatim_body(env);
        vec![Block::CodeBlock(
            Box::new(Attr {
                id: carta_ast::Text::default(),
                classes,
                attributes: attributes
                    .into_iter()
                    .map(|(k, v)| (k.into(), v.into()))
                    .collect(),
            }),
            content.into(),
        )]
    }

    /// Reads a verbatim environment body verbatim, stopping before `\end{env}`.
    pub(super) fn read_verbatim_body(&mut self, env: &str) -> String {
        let closing = format!("\\end{{{env}}}");
        let mut body = String::new();
        while !self.eof() {
            if self.looking_at(&closing) {
                break;
            }
            if let Some(c) = self.bump() {
                body.push(c);
            }
        }
        self.consume_env_marker("\\end");
        body.trim_matches('\n').to_owned()
    }

    // --- Figures & tables ------------------------------------------------------------------------

    pub(super) fn parse_figure(&mut self, env: &str) -> Block {
        let _ = self.read_optional_raw(); // float placement
        if env == "wrapfigure" {
            let _ = self.read_group_raw(); // placement
            let _ = self.read_group_raw(); // width
        }
        let was_in_figure = self.in_figure;
        self.in_figure = true;
        let (blocks, caption, id) = self.collect_float(env);
        self.in_figure = was_in_figure;
        Block::Figure(
            Box::new(Attr {
                id: id.unwrap_or_default().into(),
                classes: Vec::new(),
                attributes: Vec::new(),
            }),
            Box::new(Caption {
                short: None,
                long: caption,
            }),
            blocks.into_iter().map(demote_image_para).collect(),
        )
    }

    pub(super) fn parse_table_float(&mut self, env: &str) -> Vec<Block> {
        let _ = self.read_optional_raw();
        let (mut blocks, caption, id) = self.collect_float(env);
        if !caption.is_empty()
            && let Some(Block::Table(table)) =
                blocks.iter_mut().find(|b| matches!(b, Block::Table(_)))
        {
            table.caption = Caption {
                short: None,
                long: caption,
            };
            if let Some(id) = id {
                table.attr.id = id.into();
            }
        }
        blocks
    }

    /// Parses a float body, pulling out a `\caption` (as caption blocks) and a `\label` (as an id).
    pub(super) fn collect_float(&mut self, env: &str) -> (Vec<Block>, Vec<Block>, Option<String>) {
        let mut blocks = Vec::new();
        let mut caption = Vec::new();
        let mut id = None;
        let was_in_float = self.in_float;
        self.in_float = true;
        loop {
            self.skip_block_ws();
            if self.eof() || self.at_end_env(env) {
                break;
            }
            if self.at_control_word("caption") {
                self.consume_control_word();
                let _ = self.read_optional_raw();
                let inlines = self.parse_group_inlines_capturing_label(&mut id);
                caption = vec![Block::Plain(inlines)];
                continue;
            }
            if self.at_control_word("centering")
                || self.at_control_word("small")
                || self.at_control_word("footnotesize")
            {
                self.consume_control_word();
                continue;
            }
            if self.at_control_word("label") {
                self.consume_control_word();
                id = self.read_group_raw();
                continue;
            }
            if let Some(mut produced) = self.parse_block_construct() {
                blocks.append(&mut produced);
            } else {
                let para = self.parse_paragraph();
                if !para.is_empty() {
                    blocks.push(Block::Para(para));
                } else if !self.advance_over_stray() {
                    break;
                }
            }
        }
        self.consume_env_marker("\\end");
        self.in_float = was_in_float;
        (blocks, caption, id)
    }

    pub(super) fn parse_tabular(&mut self, env: &str) -> Block {
        if env == "tabular*" || env == "tabularx" || env == "tabulary" {
            let _ = self.read_group_raw(); // width
        }
        let _ = self.read_optional_raw(); // vertical position
        let spec = self.read_group_raw().unwrap_or_default();
        let aligns = parse_column_spec(&spec);
        let body = self.read_environment_source(env);
        self.consume_env_marker("\\end");
        build_table(self, &aligns, &body)
    }

    /// Reads a math environment as a single math inline. `math`/`displaymath` carry the body alone;
    /// the aligned and numbered environments carry their `\begin`/`\end` wrapper inside the formula.
    pub(super) fn read_math_environment(&mut self, env: &str) -> Inline {
        self.consume_env_marker("\\begin");
        let body = self.read_environment_source(env);
        self.consume_env_marker("\\end");
        match env {
            "math" => Inline::Math(MathType::InlineMath, body.trim().into()),
            "displaymath" => Inline::Math(MathType::DisplayMath, body.trim().into()),
            _ => {
                // markers re-emitted on their own lines; trailing whitespace stripped, first-line indent kept
                let content = body.trim_end().trim_start_matches(['\n', '\r']);
                Inline::Math(
                    MathType::DisplayMath,
                    format!("\\begin{{{env}}}\n{content}\n\\end{{{env}}}").into(),
                )
            }
        }
    }

    /// Reads the raw source of an environment body up to (but not consuming) its `\end{env}`.
    pub(super) fn read_environment_source(&mut self, env: &str) -> String {
        let closing = format!("\\end{{{env}}}");
        let mut out = String::new();
        while !self.eof() {
            if self.looking_at(&closing) {
                break;
            }
            if let Some(c) = self.bump() {
                out.push(c);
            }
        }
        out
    }
}
