use super::attributes::{add_inline_attr, fields, formatted_attributes};
use super::{
    ATTRIBUTE_REFERENCE, IDENTIFIER_SOURCE, MONOSPACE_ATTRIBUTE, MONOSPACE_REFERENCE,
    MONOSPACE_SCOPE, REFERENCE_LABEL,
};
use super::{Attributes, Parser, class_attr, push_text, text_inlines};
use carta_ast::{Attr, Block, BlockParts, Inline, MathType, QuoteType, Table, Target};

impl Parser {
    pub(super) fn inlines(&mut self, source: &str) -> Vec<Inline> {
        self.parse_inlines(source, false)
    }

    pub(super) fn paragraph_inlines(&mut self, source: &str, hard_breaks: bool) -> Vec<Inline> {
        self.parse_inlines(source, hard_breaks)
    }

    fn parse_inlines(&mut self, source: &str, hard_breaks: bool) -> Vec<Inline> {
        if self.depth >= 128 {
            return text_inlines(source);
        }
        self.depth += 1;
        let output = self.scan(super::trim_space(source), hard_breaks);
        self.depth -= 1;
        output
    }

    #[allow(
        clippy::too_many_lines,
        reason = "The scanner dispatches each inline construct."
    )]
    fn scan(&mut self, source: &str, hard_breaks: bool) -> Vec<Inline> {
        let mut output = Vec::new();
        let mut position = 0;
        let mut replacement_start = 0;
        let mut raw_hard_break = false;
        while position < source.len() {
            let rest = &source[position..];
            let character = rest.chars().next().unwrap_or_default();
            let previous = if position == replacement_start {
                None
            } else {
                match output.last() {
                    Some(Inline::Str(text)) => text.chars().next_back(),
                    Some(Inline::Space) => Some(' '),
                    Some(Inline::SoftBreak | Inline::LineBreak) => Some('\n'),
                    _ => None,
                }
            };
            if super::line_comment(rest) {
                position += rest.find('\n').map_or(rest.len(), |end| end + 1);
                continue;
            }

            if rest.starts_with('\\')
                && let Some(next) = rest[1..].chars().next()
                && (letter(next) || punctuation(next))
            {
                finish_text(&mut output, &mut raw_hard_break);
                push_text(&mut output, &next.to_string());
                position += 1 + next.len_utf8();
                replacement_start = position;
                continue;
            }
            if rest.starts_with("+\n") {
                finish_text(&mut output, &mut raw_hard_break);
                while matches!(output.last(), Some(Inline::Space | Inline::SoftBreak)) {
                    output.pop();
                }
                output.push(Inline::LineBreak);
                position += 1;
                while source[position..].starts_with('\n') {
                    position += 1;
                }
                replacement_start = position;
                continue;
            }
            if rest.starts_with(" +\n") {
                finish_text(&mut output, &mut raw_hard_break);
                while matches!(output.last(), Some(Inline::Space | Inline::SoftBreak)) {
                    output.pop();
                }
                output.push(Inline::LineBreak);
                position += 3;
                replacement_start = position;
                continue;
            }
            if rest.starts_with(" -- ") {
                push_text(&mut output, " — ");
                position += 4;
                replacement_start = position;
                continue;
            }
            if rest.starts_with("--")
                && position > replacement_start
                && previous.is_some_and(alpha_numeric)
                && rest[2..].chars().next().is_some_and(alpha_numeric)
            {
                push_text(&mut output, "—\u{200B}");
                position += 2;
                replacement_start = position;
                continue;
            }
            let preserved =
                if rest.starts_with("--") && position > replacement_start && rest.len() > 2 {
                    Some(2)
                } else if (rest.starts_with("---") && position == replacement_start
                    || rest.starts_with("<--")
                    || rest.starts_with("'--"))
                    && rest.len() > 3
                {
                    Some(3)
                } else {
                    None
                };
            if let Some(length) = preserved {
                push_text(&mut output, &rest[..length]);
                position += length;
                replacement_start = position;
                continue;
            }
            if matches!(character, ' ' | '\t' | '\n') {
                if character == '\n' {
                    raw_hard_break = hard_breaks;
                    if matches!(output.last(), Some(Inline::Space)) {
                        output.pop();
                    }
                    output.push(if hard_breaks {
                        Inline::LineBreak
                    } else {
                        Inline::SoftBreak
                    });
                } else if !matches!(output.last(), Some(Inline::Space | Inline::SoftBreak))
                    && !output.last().is_some_and(ends_with_line_break)
                {
                    output.push(Inline::Space);
                }
                position += character.len_utf8();
                continue;
            }
            if character == '{'
                && let Some(end) = rest.find('}')
            {
                let name = &rest[1..end];
                if let Some((counter, specification)) = name
                    .strip_prefix("counter:")
                    .filter(|value| !value.ends_with(':'))
                    .map(|s| s.split_once(':').unwrap_or((s, "")))
                    .filter(|(name, specification)| {
                        name.chars()
                            .next()
                            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
                            && name
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                            && (specification.is_empty()
                                || specification.len() == 1
                                    && specification.chars().all(|c| c.is_ascii_alphabetic())
                                || super::unsigned(specification, 10).is_some())
                    })
                {
                    let (value, alphabetic) =
                        if let Some((value, alphabetic)) = self.counters.get(counter) {
                            (value.wrapping_add(1), *alphabetic)
                        } else if let Some(value) = super::unsigned(specification, 10) {
                            (value.cast_signed(), false)
                        } else if let Some(character) = specification
                            .chars()
                            .next()
                            .filter(char::is_ascii_alphabetic)
                        {
                            (i64::from(u32::from(character.to_ascii_uppercase())), true)
                        } else {
                            (1, false)
                        };
                    self.counters
                        .insert(counter.to_owned(), (value, alphabetic));
                    let mut attr = class_attr("counter");
                    attr.attributes.push(("name".into(), counter.into()));
                    let text = if alphabetic {
                        u32::try_from(value)
                            .ok()
                            .and_then(char::from_u32)
                            .unwrap_or_default()
                            .to_string()
                    } else {
                        value.to_string()
                    };
                    finish_text(&mut output, &mut raw_hard_break);
                    output.extend(identifier_source(
                        vec![Inline::Span(Box::new(attr), vec![Inline::Str(text.into())])],
                        String::new(),
                    ));
                    position += end + 1;
                    replacement_start = position;
                    continue;
                }
                if let Some(value) = super::document::builtin_attribute(name) {
                    let inlines = text_segment(value, hard_breaks);
                    finish_text(&mut output, &mut raw_hard_break);
                    append(&mut output, inlines);
                    position += end + 1;
                    replacement_start = position;
                    continue;
                }
                if name
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                {
                    finish_text(&mut output, &mut raw_hard_break);
                    if self.in_definition_term() {
                        push_text(&mut output, &format!("{{{name}}}"));
                    } else {
                        output.push(Inline::RawInline(
                            carta_ast::Format(ATTRIBUTE_REFERENCE.into()),
                            name.into(),
                        ));
                    }
                    position += end + 1;
                    replacement_start = position;
                    continue;
                }
            }
            if character == '['
                && let Some(end) = attribute_end(rest)
                && let Some(attr) = formatted_attributes(&rest[1..end])
                && let Some((node, consumed)) =
                    self.formatted(&rest[end + 1..], previous, Some(attr), hard_breaks)
            {
                finish_text(&mut output, &mut raw_hard_break);
                append(&mut output, node);
                position += end + 1 + consumed;
                replacement_start = position;
                continue;
            }
            if rest.starts_with("[[") {
                let bibliography = rest.starts_with("[[[") && rest.contains("]]]");
                let start = if bibliography { 3 } else { 2 };
                let close = if bibliography { "]]]" } else { "]]" };
                if let Some(end) = rest[start..].find(close) {
                    let (name, label) = rest[start..start + end]
                        .split_once(',')
                        .unwrap_or((&rest[start..start + end], ""));
                    let label = label.split(',').collect::<String>();
                    let label = self.inlines(&label);
                    let identifier = super::document::identifier_text(&label);
                    if !self.in_definition_term() {
                        self.anchors.entry(name.to_owned()).or_insert(label);
                    }
                    finish_text(&mut output, &mut raw_hard_break);
                    output.extend(identifier_source(
                        vec![Inline::Span(
                            Box::new(Attr {
                                id: name.into(),
                                ..Default::default()
                            }),
                            vec![],
                        )],
                        identifier,
                    ));
                    position += start + end + close.len();
                    if bibliography {
                        while source[position..].starts_with(' ') {
                            position += 1;
                        }
                    }
                    replacement_start = position;
                    continue;
                }
            }
            if rest.starts_with("<<")
                && let Some(end) = rest.find(">>")
                && !rest[..end].contains(['\n', '\r'])
            {
                let (target, label) = rest[2..end]
                    .split_once(',')
                    .map_or((&rest[2..end], None), |(t, l)| (t, Some(l)));
                let label = match label {
                    Some(label) => self.inlines(label),
                    None if self.in_definition_term() => {
                        vec![Inline::Str(target.into())]
                    }
                    None => vec![Inline::RawInline(
                        carta_ast::Format(REFERENCE_LABEL.into()),
                        target.into(),
                    )],
                };
                finish_text(&mut output, &mut raw_hard_break);
                output.push(Inline::Link(
                    Box::new(class_attr("cross-reference")),
                    label,
                    Box::new(Target {
                        url: format!("#{target}").into(),
                        title: "".into(),
                    }),
                ));
                position += end + 2;
                replacement_start = position;
                continue;
            }
            if let Some((nodes, consumed)) = self.macro_inline(rest) {
                finish_text(&mut output, &mut raw_hard_break);
                append(&mut output, nodes);
                position += consumed;
                replacement_start = position;
                continue;
            }
            if let Some((nodes, consumed)) = self.formatted(rest, previous, None, hard_breaks) {
                finish_text(&mut output, &mut raw_hard_break);
                append(&mut output, nodes);
                position += consumed;
                replacement_start = position;
                continue;
            }
            if character == '&' {
                match entity(rest) {
                    Ok(Some((decoded, consumed))) => {
                        if matches!(decoded.as_str(), "\n" | "\r")
                            && matches!(output.last(), Some(Inline::Space))
                        {
                            output.pop();
                        }
                        finish_text(&mut output, &mut raw_hard_break);
                        append(&mut output, text_segment(&decoded, hard_breaks));
                        position += consumed;
                        replacement_start = position;
                        continue;
                    }
                    Err(()) => {
                        self.syntax_error("character reference is out of range");
                        break;
                    }
                    Ok(None) => {}
                }
            }
            let replacements = [
                (" -- ", "\u{2009}—\u{2009}"),
                ("...", "…"),
                ("(C)", "©"),
                ("(R)", "®"),
                ("(TM)", "™"),
                ("->", "→"),
                ("<-", "←"),
                ("=>", "⇒"),
                ("<=", "⇐"),
            ];
            if let Some((from, to)) = replacements.iter().find(|(from, _)| rest.starts_with(from)) {
                push_text(&mut output, to);
                position += from.len();
                replacement_start = position;
                continue;
            }
            if character == '\'' {
                push_text(&mut output, "’");
                position += 1;
                replacement_start = position;
                continue;
            }
            push_text(&mut output, &character.to_string());
            position += character.len_utf8();
        }
        output
    }

    fn quoted(
        &mut self,
        source: &str,
        attributes: Option<&Attributes>,
    ) -> Option<(Vec<Inline>, usize)> {
        for (quote, kind) in [
            ('"', QuoteType::DoubleQuote),
            ('\'', QuoteType::SingleQuote),
        ] {
            if !source.starts_with(quote) || !source[quote.len_utf8()..].starts_with('`') {
                continue;
            }
            let doubled = source[2..].starts_with('`');
            let opening = if doubled { 3 } else { 2 };
            let close = if doubled { "``" } else { "`" };
            let body = &source[opening..];
            let end = body
                .match_indices(close)
                .map(|(index, _)| index)
                .find(|index| !body[..*index].ends_with('\\'))?;
            if end == 0
                || !doubled && body.starts_with(super::is_space)
                || !body[end + close.len()..].starts_with(quote)
            {
                return None;
            }
            let content = body[..end].replace("\\`", "`");
            let mut nodes = vec![Inline::Quoted(kind, self.inlines(&content))];
            if let Some(attr) = attributes {
                nodes = add_inline_attr(nodes, &attr.attr(), !attr.named.is_empty());
            }
            return Some((nodes, opening + end + close.len() + quote.len_utf8()));
        }

        None
    }

    fn formatted(
        &mut self,
        source: &str,
        previous: Option<char>,
        attributes: Option<Attributes>,
        hard_breaks: bool,
    ) -> Option<(Vec<Inline>, usize)> {
        if source.starts_with(['"', '\'']) {
            return self.quoted(source, attributes.as_ref());
        }

        if let Some(body) = source.strip_prefix("+++")
            && let Some(end) = body.find("+++")
        {
            return Some((
                identifier_source(Self::html_inlines(&body[..end]), String::new()),
                end + 6,
            ));
        }

        for (open, close) in [
            ("**", "**"),
            ("__", "__"),
            ("``", "``"),
            ("##", "##"),
            ("++", "++"),
            ("*", "*"),
            ("_", "_"),
            ("`", "`"),
            ("#", "#"),
            ("~", "~"),
            ("^", "^"),
            ("+", "+"),
        ] {
            if !source.starts_with(open) {
                continue;
            }
            let unconstrained = open.len() > 1 || matches!(open, "~" | "^");
            if !unconstrained
                && previous.is_some_and(|c| !super::is_space(c) && !punctuation(c) && c != '+')
            {
                continue;
            }
            let body = &source[open.len()..];
            if !matches!(open, "**" | "__" | "``" | "##" | "++" | "+++")
                && body.starts_with(super::is_space)
            {
                continue;
            }
            let end = body
                .match_indices(close)
                .map(|(index, _)| index)
                .find(|index| matches!(open, "~" | "^") || !body[..*index].ends_with('\\'));
            if let Some(end) = end {
                let after = &body[end + close.len()..];
                if end == 0
                    || (!unconstrained
                        && after
                            .chars()
                            .next()
                            .is_some_and(|c| !super::is_space(c) && !punctuation(c) && c != '+'))
                {
                    return None;
                }
                let delimiter = open.chars().next().unwrap_or_default();
                let unescaped =
                    body[..end].replace(&format!("\\{delimiter}"), &delimiter.to_string());
                let body = unescaped.as_str();
                if matches!(open, "~" | "^") && body.contains(super::is_space) {
                    break;
                }
                let mut nodes = match open {
                    "*" | "**" | "***" => vec![Inline::Strong(self.inlines(body))],
                    "_" | "__" => vec![Inline::Emph(self.inlines(body))],
                    "`" | "``" => {
                        let mut nodes = self.inlines(body);
                        monospace_run(&mut nodes);
                        nodes
                    }
                    "#" | "##" => vec![Inline::Span(
                        Box::new(
                            if attributes
                                .as_ref()
                                .is_some_and(|attr| attr.get("role").is_some())
                            {
                                Attr::default()
                            } else {
                                class_attr("mark")
                            },
                        ),
                        self.inlines(body),
                    )],
                    "~" => vec![Inline::Subscript(self.inlines(body))],
                    "^" => vec![Inline::Superscript(self.inlines(body))],
                    _ => text_segment(body, hard_breaks),
                };
                if let Some(attr) = attributes {
                    nodes = add_inline_attr(nodes, &attr.attr(), !attr.named.is_empty());
                }
                return Some((nodes, open.len() + end + close.len()));
            }
        }
        None
    }

    #[allow(
        clippy::too_many_lines,
        reason = "Macro names share target and attribute parsing."
    )]
    fn macro_inline(&mut self, source: &str) -> Option<(Vec<Inline>, usize)> {
        let names = [
            "footnoteref:",
            "anchor:",
            "footnote:",
            "image:",
            "icon:",
            "link:",
            "mailto:",
            "xref:",
            "kbd:",
            "btn:",
            "menu:",
            "pass:",
            "stem:",
            "latexmath:",
            "asciimath:",
            "indexterm:",
            "indexterm2:",
        ];
        if let Some(prefix) = names.iter().copied().find(|name| source.starts_with(name))
            && let Some(parsed) = self.named_macro(prefix, &source[prefix.len()..])
        {
            return Some(parsed);
        }
        for (open, close) in [("(((", ")))"), ("((", "))")] {
            if let Some(rest) = source.strip_prefix(open)
                && let Some(end) = rest.find(close)
            {
                let mut attr = class_attr("index");
                attr.attributes.push((
                    "term".into(),
                    if open == "(((" {
                        rest[..end]
                            .split(',')
                            .map(super::trim_space)
                            .collect::<Vec<_>>()
                            .join(",")
                            .into()
                    } else {
                        rest[..end].into()
                    },
                ));
                return Some((
                    identifier_source(
                        vec![Inline::Span(
                            Box::new(attr),
                            if open == "((" {
                                text_inlines(&rest[..end])
                            } else {
                                vec![]
                            },
                        )],
                        String::new(),
                    ),
                    open.len() + end + close.len(),
                ));
            }
        }
        let angled = source.starts_with('<');
        let source_url = if angled { &source[1..] } else { source };
        if let Some(end) = autolink_target_end(source_url) {
            let target = &source_url[..end];
            let mut consumed = end;
            let mut attr = Attr::default();
            let mut label = vec![Inline::Str(target.into())];
            if source_url[end..].starts_with('[')
                && let Some((description, attributes, count)) = link_description(&source_url[end..])
            {
                attr = attributes;
                consumed += count;
                if !description.is_empty() {
                    label = self.inlines(&description);
                }
            }
            if angled {
                if !source_url[consumed..].starts_with('>') {
                    return None;
                }
                consumed += 1;
            }
            return Some((
                vec![Inline::Link(
                    Box::new(attr),
                    label,
                    Box::new(Target {
                        url: target.into(),
                        title: "".into(),
                    }),
                )],
                consumed + usize::from(angled),
            ));
        }
        if angled {
            return None;
        }
        if !source_url.chars().next().is_some_and(letter) {
            return None;
        }
        let address = email_address(source_url)?;
        let mut consumed = address.len();
        let mut attr = Attr::default();
        let mut label = vec![Inline::Str(address.into())];
        if source_url[consumed..].starts_with('[')
            && let Some((description, attributes, count)) =
                link_description(&source_url[consumed..])
        {
            attr = attributes;
            consumed += count;
            if !description.is_empty() {
                label = self.inlines(&description);
            }
        }
        Some((
            vec![Inline::Link(
                Box::new(attr),
                label,
                Box::new(Target {
                    url: address.into(),
                    title: "".into(),
                }),
            )],
            consumed,
        ))
    }

    #[allow(
        clippy::too_many_lines,
        reason = "Named macros share target and attribute parsing."
    )]
    fn named_macro(&mut self, prefix: &str, after: &str) -> Option<(Vec<Inline>, usize)> {
        let (target, start) = read_macro_target(after, true)?;
        if !after[start..].starts_with('[') {
            return None;
        }
        let raw = matches!(
            prefix,
            "footnote:"
                | "xref:"
                | "stem:"
                | "latexmath:"
                | "asciimath:"
                | "indexterm:"
                | "indexterm2:"
        );
        let end = if raw {
            bracket_end(&after[start..])? + start
        } else {
            attribute_end(&after[start..])? + start
        };
        let content = &after[start + 1..end];
        if !raw && !Attributes::valid(content) {
            return None;
        }
        let attributes = Attributes::parse(content);
        if prefix == "footnoteref:" && attributes.positional.is_empty() {
            return None;
        }
        let mut attr = attributes.attr();
        let fields = fields(content);
        let label = attributes.positional.first().map_or("", String::as_str);
        let mut origin = matches!(
            prefix,
            "btn:" | "kbd:" | "menu:" | "pass:" | "indexterm:" | "indexterm2:"
        )
        .then(String::new);
        let nodes = match prefix {
            "stem:" | "latexmath:" | "asciimath:" => {
                if prefix == "stem:" && self.in_definition_term() {
                    vec![Inline::RawInline(
                        carta_ast::Format(super::TERM_STEM_ERROR.into()),
                        content.into(),
                    )]
                } else {
                    vec![Inline::Math(MathType::InlineMath, content.into())]
                }
            }
            "anchor:" => {
                let (identifier, label) = target.split_once(',').unwrap_or((&target, ""));
                let label = label.split(',').collect::<String>();
                let label = self.inlines(&label);
                origin = Some(super::document::identifier_text(&label));
                if !self.in_definition_term() {
                    self.anchors.entry(identifier.to_owned()).or_insert(label);
                }
                vec![Inline::Span(
                    Box::new(Attr {
                        id: identifier.into(),
                        ..attr
                    }),
                    vec![],
                )]
            }
            "footnoteref:" | "footnote:" => {
                let raw_content = content;
                let (target, content) = if prefix == "footnoteref:" {
                    (
                        fields.first().cloned().unwrap_or_default(),
                        fields.get(1).map_or("", String::as_str),
                    )
                } else {
                    (target, raw_content)
                };
                let inlines = self.inlines(content);
                origin = Some(super::document::identifier_text(&inlines));
                let inlines = if target.is_empty() || self.in_definition_term() {
                    inlines
                } else {
                    self.footnotes.entry(target).or_insert(inlines).clone()
                };
                vec![Inline::Note(vec![Block::Para(inlines)])]
            }
            "image:" => vec![Self::image(&target, content)],
            "icon:" => {
                if self.in_definition_term() {
                    attr.classes.push("icon".into());
                    let label = format!("[{target}]");
                    attr.attributes.push(("name".into(), target.into()));
                    vec![Inline::Span(
                        Box::new(attr),
                        vec![Inline::Str(label.into())],
                    )]
                } else {
                    attr.attributes
                        .push((super::ICON_NAME.into(), target.into()));
                    vec![Inline::Span(Box::new(attr), vec![])]
                }
            }
            "btn:" => vec![Inline::Span(
                Box::new(class_attr("button")),
                vec![Inline::Strong(vec![Inline::Str(
                    format!("[{label}]").into(),
                )])],
            )],
            "kbd:" => {
                let mut nodes = Vec::new();
                for key in label.split('+').map(str::trim) {
                    if !nodes.is_empty() {
                        nodes.push(Inline::Str("+".into()));
                    }
                    nodes.push(Inline::Span(
                        Box::new(class_attr("kbd")),
                        vec![Inline::Strong(vec![Inline::Str(key.into())])],
                    ));
                }
                nodes
            }
            "menu:" => {
                let menu = std::iter::once(target.as_str())
                    .chain(label.split('>').map(str::trim).filter(|s| !s.is_empty()))
                    .collect::<Vec<_>>()
                    .join(" › ");
                vec![Inline::Span(
                    Box::new(class_attr("menu")),
                    vec![Inline::Strong(text_inlines(&menu))],
                )]
            }
            "pass:" => Self::html_inlines(label),
            "indexterm:" | "indexterm2:" => {
                attr = class_attr("index");
                attr.attributes.push((
                    "term".into(),
                    if prefix == "indexterm:" {
                        content
                            .split(',')
                            .map(str::trim)
                            .collect::<Vec<_>>()
                            .join(",")
                            .into()
                    } else {
                        content.into()
                    },
                ));
                vec![Inline::Span(
                    Box::new(attr),
                    if prefix == "indexterm2:" {
                        text_inlines(content)
                    } else {
                        vec![]
                    },
                )]
            }
            "xref:" => {
                let label = if super::trim_space(content).is_empty() {
                    if self.in_definition_term() {
                        vec![Inline::Str(target.as_str().into())]
                    } else {
                        vec![Inline::RawInline(
                            carta_ast::Format(REFERENCE_LABEL.into()),
                            target.as_str().into(),
                        )]
                    }
                } else {
                    self.inlines(content)
                };
                vec![Inline::Link(
                    Box::new(class_attr("cross-reference")),
                    label,
                    Box::new(Target {
                        url: format!("#{target}").into(),
                        title: "".into(),
                    }),
                )]
            }
            _ => {
                let url = match prefix {
                    "xref:" => {
                        attr.classes.push("cross-reference".into());
                        format!("#{target}")
                    }
                    _ => target.clone(),
                };
                let inlines = if label.is_empty() {
                    if prefix == "xref:" {
                        vec![Inline::RawInline(
                            carta_ast::Format(REFERENCE_LABEL.into()),
                            target.as_str().into(),
                        )]
                    } else {
                        text_inlines(&target)
                    }
                } else {
                    self.inlines(label)
                };
                vec![Inline::Link(
                    Box::new(attr),
                    inlines,
                    Box::new(Target {
                        url: url.into(),
                        title: "".into(),
                    }),
                )]
            }
        };
        let nodes = if matches!(
            prefix,
            "image:"
                | "icon:"
                | "link:"
                | "mailto:"
                | "xref:"
                | "anchor:"
                | "stem:"
                | "latexmath:"
                | "asciimath:"
                | "indexterm:"
                | "indexterm2:"
                | "footnote:"
        ) {
            nodes
        } else {
            add_inline_attr(nodes, &attributes.attr(), !attributes.named.is_empty())
        };
        let nodes = if let Some(text) = origin {
            identifier_source(nodes, text)
        } else {
            nodes
        };
        Some((nodes, prefix.len() + end + 1))
    }

    pub(super) fn image(target: &str, content: &str) -> Inline {
        let attributes = Attributes::parse(content);
        let mut attr = attributes.attr();
        let alt = attributes.positional.first().map_or("", String::as_str);
        for (name, index) in [("width", 1), ("height", 2)] {
            if let Some(value) = attributes.positional.get(index).map(String::as_str)
                && let Ok(number) = value
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse::<i64>()
            {
                attr.attributes.retain(|(key, _)| key != name);
                attr.attributes
                    .push((name.into(), format!("{number}px").into()));
            }
        }
        let target = target.to_owned();

        Inline::Image(
            Box::new(attr),
            text_inlines(alt),
            Box::new(Target {
                url: target.into(),
                title: "".into(),
            }),
        )
    }

    pub(super) fn html_inlines(source: &str) -> Vec<Inline> {
        blocks_to_inlines(Self::html_blocks(source))
    }
}

fn blocks_to_inlines(blocks: Vec<Block>) -> Vec<Inline> {
    carta_core::with_stack(|| {
        let mut output = Vec::new();
        for (index, block) in blocks.into_iter().enumerate() {
            if index > 0 {
                output.push(Inline::LineBreak);
            }
            append_inlines(&mut output, block_to_inlines(block));
        }
        output
    })
}

fn block_to_inlines(block: Block) -> Vec<Inline> {
    match block.into_parts() {
        BlockParts::Plain(inlines)
        | BlockParts::Para(inlines)
        | BlockParts::Header(_, _, inlines) => inlines,
        BlockParts::LineBlock(lines) => {
            let mut output = Vec::new();
            for (index, line) in lines.into_iter().enumerate() {
                if index > 0 {
                    output.push(Inline::LineBreak);
                }
                append_inlines(&mut output, line);
            }
            output
        }
        BlockParts::CodeBlock(attr, code) => vec![Inline::Code(attr, code)],
        BlockParts::RawBlock(format, text) => vec![Inline::RawInline(format, text)],
        BlockParts::BlockQuote(blocks)
        | BlockParts::Div(_, blocks)
        | BlockParts::Figure(_, _, blocks) => blocks_to_inlines(blocks),
        BlockParts::OrderedList(_, items) | BlockParts::BulletList(items) => {
            let mut output = Vec::new();
            for item in items {
                append_inlines(&mut output, blocks_to_inlines(item));
            }
            output
        }
        BlockParts::DefinitionList(entries) => {
            let mut output = Vec::new();
            for (term, definitions) in entries {
                append_inlines(&mut output, term);
                append_inlines(&mut output, [Inline::Str(":".into()), Inline::Space]);
                for definition in definitions {
                    append_inlines(&mut output, blocks_to_inlines(definition));
                }
            }
            output
        }
        BlockParts::HorizontalRule => Vec::new(),
        BlockParts::Table(table) => table_to_inlines(*table),
    }
}

fn table_to_inlines(table: Table) -> Vec<Inline> {
    let rows = table
        .head
        .rows
        .into_iter()
        .chain(
            table
                .bodies
                .into_iter()
                .flat_map(|body| body.head.into_iter().chain(body.body)),
        )
        .chain(table.foot.rows);
    let mut output = Vec::new();
    for (index, row) in rows.enumerate() {
        if index > 0 {
            output.push(Inline::LineBreak);
        }
        for cell in row.cells {
            append_inlines(&mut output, blocks_to_inlines(cell.content));
        }
    }
    output
}

fn append_inlines(output: &mut Vec<Inline>, inlines: impl IntoIterator<Item = Inline>) {
    for inline in inlines {
        append_builder_inline(output, inline);
    }
}

fn append_builder_inline(output: &mut Vec<Inline>, inline: Inline) {
    if matches!(output.last(), Some(Inline::Space | Inline::SoftBreak))
        && starts_with_line_break(&inline)
    {
        output.pop();
        output.push(inline);
    } else if matches!(&inline, Inline::Space | Inline::SoftBreak)
        && output.last().is_some_and(ends_with_line_break)
    {
        // Pandoc's Inlines semigroup removes whitespace beside a hard line break.
    } else if let Inline::Str(text) = &inline
        && let Some(Inline::Str(previous)) = output.last_mut()
    {
        previous.push_str(text);
    } else {
        output.push(inline);
    }
}

fn starts_with_line_break(inline: &Inline) -> bool {
    match inline {
        Inline::LineBreak => true,
        Inline::Span(attributes, children)
            if attributes
                .attributes
                .iter()
                .any(|(name, _)| name == IDENTIFIER_SOURCE) =>
        {
            matches!(children.first(), Some(Inline::LineBreak))
        }
        _ => false,
    }
}

fn ends_with_line_break(inline: &Inline) -> bool {
    match inline {
        Inline::LineBreak => true,
        Inline::Span(attributes, children)
            if attributes
                .attributes
                .iter()
                .any(|(name, _)| name == IDENTIFIER_SOURCE) =>
        {
            matches!(children.last(), Some(Inline::LineBreak))
        }
        _ => false,
    }
}

fn bracket_end(source: &str) -> Option<usize> {
    let mut depth = 0;
    let mut escaped = false;
    for (position, character) in source.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\n' || character == '\r' {
            return None;
        }
        match character {
            '\\' => escaped = true,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(position);
                }
            }
            _ => {}
        }
    }
    None
}

// Pandoc drops a text segment's final newline when it creates hard breaks.
fn finish_text(output: &mut Vec<Inline>, raw_hard_break: &mut bool) {
    if *raw_hard_break && matches!(output.last(), Some(Inline::LineBreak)) {
        output.pop();
    }
    *raw_hard_break = false;
}

fn text_segment(text: &str, hard_breaks: bool) -> Vec<Inline> {
    if !hard_breaks {
        return text_inlines(text);
    }
    let mut output = Vec::new();
    for (index, line) in text.split_terminator('\n').enumerate() {
        if index > 0 {
            if matches!(output.last(), Some(Inline::Space)) {
                output.pop();
            }
            output.push(Inline::LineBreak);
        }
        append(&mut output, text_inlines(line));
    }
    output
}

// Section IDs use source text before inline conversion.
fn identifier_source(nodes: Vec<Inline>, text: String) -> Vec<Inline> {
    vec![Inline::Span(
        Box::new(Attr {
            attributes: vec![(IDENTIFIER_SOURCE.into(), text.into())],
            ..Attr::default()
        }),
        nodes,
    )]
}

fn append(output: &mut Vec<Inline>, nodes: Vec<Inline>) {
    for node in nodes {
        if matches!(&node, Inline::Str(text) if text.is_empty()) {
            continue;
        }
        append_builder_inline(output, node);
    }
}

pub(super) fn macro_target(source: &str) -> Option<(String, usize)> {
    read_macro_target(source, false)
}

fn read_macro_target(source: &str, inline: bool) -> Option<(String, usize)> {
    let mut target = String::new();
    let mut position = 0;
    while position < source.len() {
        let rest = &source[position..];
        if rest.starts_with('[') {
            return Some((target, position));
        }
        if let Some(pass) = rest
            .strip_prefix("pass:[")
            .filter(|pass| inline && !pass.starts_with(']'))
        {
            let end = pass.find(']')?;
            target.push_str(&pass[..end]);
            position += 6 + end + 1;
            continue;
        }
        if rest.starts_with('+') {
            if !rest.starts_with("++") {
                break;
            }
            let count = 2;
            let delimiter = &rest[..count];
            let Some(end) = rest[count..].find(delimiter) else {
                break;
            };
            target.push_str(&rest[count..count + end]);
            position += count + end + count;
            continue;
        }
        let character = rest.chars().next()?;
        if super::is_space(character) {
            break;
        }
        target.push(character);
        position += character.len_utf8();
    }
    Some((target, position))
}

pub(super) fn monospace(nodes: &mut [Inline]) {
    monospace_inlines(nodes, false, false);
}

fn monospace_inlines(nodes: &mut [Inline], scope_notes: bool, defer_attributes: bool) {
    for node in nodes {
        match node {
            Inline::Str(text) => *node = Inline::Code(Box::default(), text.clone()),
            Inline::RawInline(format, name) if format.0 == ATTRIBUTE_REFERENCE => {
                if defer_attributes {
                    format.0 = MONOSPACE_ATTRIBUTE.into();
                } else {
                    *node = Inline::Code(Box::default(), format!("{{{name}}}").into());
                }
            }
            Inline::RawInline(format, _) if format.0 == REFERENCE_LABEL => {
                format.0 = MONOSPACE_REFERENCE.into();
            }
            Inline::Strong(children)
            | Inline::Emph(children)
            | Inline::Underline(children)
            | Inline::Strikeout(children)
            | Inline::Superscript(children)
            | Inline::Subscript(children)
            | Inline::SmallCaps(children)
            | Inline::Quoted(_, children)
            | Inline::Cite(_, children)
            | Inline::Span(_, children)
            | Inline::Link(_, children, _)
            | Inline::Image(_, children, _) => {
                monospace_inlines(children, scope_notes, defer_attributes);
            }
            Inline::Note(blocks) => {
                monospace_blocks(blocks, scope_notes);
            }
            _ => {}
        }
    }
}

fn monospace_blocks(blocks: &mut [Block], scope: bool) {
    carta_core::walk::for_each_block(blocks, &mut |block| match block {
        Block::Plain(nodes) | Block::Para(nodes) | Block::Header(_, _, nodes) => {
            monospace_note_block(nodes, scope);
        }
        Block::LineBlock(lines) => {
            for nodes in lines {
                monospace_note_block(nodes, scope);
            }
        }
        Block::DefinitionList(entries) => {
            for (term, _) in entries {
                monospace_note_block(term, scope);
            }
        }
        Block::Figure(_, caption, _) => {
            if let Some(short) = &mut caption.short {
                monospace_note_block(short, scope);
            }
        }
        Block::Table(table) => {
            if let Some(short) = &mut table.caption.short {
                monospace_note_block(short, scope);
            }
        }
        _ => {}
    });
}

pub(super) fn monospace_run(nodes: &mut Vec<Inline>) {
    monospace_inlines(nodes, true, true);
    add_monospace_scope(nodes);
}

fn monospace_note_block(nodes: &mut Vec<Inline>, scope: bool) {
    monospace_inlines(nodes, false, scope);
    if scope {
        add_monospace_scope(nodes);
    }
}

fn add_monospace_scope(nodes: &mut Vec<Inline>) {
    nodes.insert(
        0,
        Inline::RawInline(carta_ast::Format(MONOSPACE_SCOPE.into()), "start".into()),
    );
    nodes.push(Inline::RawInline(
        carta_ast::Format(MONOSPACE_SCOPE.into()),
        "end".into(),
    ));
}

pub(super) fn append_monospace(output: &mut Vec<Inline>, node: Inline) {
    if let Inline::Code(attr, text) = &node
        && let Some(Inline::Code(previous_attr, previous_text)) = output.last_mut()
        && previous_attr == attr
    {
        previous_text.push_str(text);
        return;
    }
    output.push(node);
}

fn entity(source: &str) -> Result<Option<(String, usize)>, ()> {
    let Some(reference) = source.strip_prefix('&') else {
        return Ok(None);
    };

    if let Some(reference) = reference.strip_prefix('#') {
        let (digits, radix, prefix_len) = if let Some(digits) = reference
            .strip_prefix('x')
            .or_else(|| reference.strip_prefix('X'))
        {
            (digits, 16, 1)
        } else {
            (reference, 10, 0)
        };
        let digits_end = digits
            .char_indices()
            .take_while(|(_, character)| {
                if radix == 16 {
                    character.is_ascii_hexdigit()
                } else {
                    character.is_ascii_digit()
                }
            })
            .last()
            .map_or(0, |(position, character)| position + character.len_utf8());
        if digits_end == 0 || !digits[digits_end..].starts_with(';') {
            return Ok(None);
        }
        let Some(value) = super::unsigned(&digits[..digits_end], radix) else {
            return Err(());
        };
        let code = u32::try_from(value)
            .ok()
            .filter(|code| *code <= 0x0010_ffff)
            .ok_or(())?;
        let decoded = char::from_u32(code).unwrap_or('\u{fffd}').to_string();
        let consumed = 1 + 1 + prefix_len + digits_end + 1;
        return Ok(Some((decoded, consumed)));
    }

    let mut terminator = None;
    for (position, character) in reference.char_indices() {
        if character == ';' || super::is_space(character) {
            terminator = Some((position, character.len_utf8()));
            break;
        }
        if !alpha_numeric(character) {
            return Ok(None);
        }
    }
    let Some((end, terminator_len)) = terminator.filter(|(end, _)| *end > 0) else {
        return Ok(None);
    };
    let Some(decoded) = crate::entities::lookup_legacy(&reference[..end]) else {
        return Ok(None);
    };
    Ok(Some((decoded.to_owned(), 1 + end + terminator_len)))
}

fn punctuation(character: char) -> bool {
    use unicode_general_category::{GeneralCategory, get_general_category};
    matches!(
        get_general_category(character),
        GeneralCategory::ClosePunctuation
            | GeneralCategory::ConnectorPunctuation
            | GeneralCategory::DashPunctuation
            | GeneralCategory::FinalPunctuation
            | GeneralCategory::InitialPunctuation
            | GeneralCategory::OpenPunctuation
            | GeneralCategory::OtherPunctuation
    )
}

fn letter(character: char) -> bool {
    use unicode_general_category::{GeneralCategory, get_general_category};
    matches!(
        get_general_category(character),
        GeneralCategory::UppercaseLetter
            | GeneralCategory::LowercaseLetter
            | GeneralCategory::TitlecaseLetter
            | GeneralCategory::ModifierLetter
            | GeneralCategory::OtherLetter
    )
}

pub(super) fn alpha_numeric(character: char) -> bool {
    use unicode_general_category::{GeneralCategory, get_general_category};
    letter(character)
        || matches!(
            get_general_category(character),
            GeneralCategory::DecimalNumber
                | GeneralCategory::LetterNumber
                | GeneralCategory::OtherNumber
        )
}

pub(super) fn attribute_end(source: &str) -> Option<usize> {
    let mut quoted = false;
    for (position, character) in source.char_indices().skip(1) {
        if character == '"' && (quoted || source[position + 1..].contains('"')) {
            quoted = !quoted;
        }
        if character == ']' && !quoted {
            return Some(position);
        }
    }
    None
}

fn link_description(source: &str) -> Option<(String, Attr, usize)> {
    let end = attribute_end(source)?;
    let content = &source[1..end];
    let (description, attributes) = if let Some(quoted) = content.strip_prefix('"') {
        let close = quoted.find('"')?;
        let rest = super::trim_space_start(&quoted[close + 1..]);
        let attributes = if rest.is_empty() {
            Attr::default()
        } else {
            Attributes::parse(super::trim_space_start(rest.strip_prefix(',')?)).attr()
        };
        (quoted[..close].to_owned(), attributes)
    } else {
        let boundary = content.char_indices().find_map(|(position, character)| {
            if character != ',' {
                return None;
            }
            let next = &content[position + 1..];
            next.split(',')
                .next()
                .filter(|field| field.contains('='))
                .map(|_| position)
        });
        let (description, rest) = boundary.map_or((content, None), |position| {
            (&content[..position], Some(&content[position + 1..]))
        });
        if description.contains('=') {
            return None;
        }
        (
            description.to_owned(),
            rest.map_or_else(Attr::default, |rest| {
                Attributes::parse(super::trim_space_start(rest)).attr()
            }),
        )
    };
    Some((description, attributes, end + 1))
}

fn autolink_target_end(source: &str) -> Option<usize> {
    // `+` is ordinary URI text, so the source parser consumes it first.
    let scheme_len = ["http:", "https:", "irc:", "ftp:", "mailto:"]
        .iter()
        .find_map(|scheme| source.starts_with(scheme).then_some(scheme.len()))?;
    let mut end = scheme_len;
    for (offset, character) in source[scheme_len..].char_indices() {
        let position = scheme_len + offset;
        let accepted = if autolink_special_punctuation(character) {
            source[position + character.len_utf8()..]
                .chars()
                .next()
                .is_some_and(|next| !super::is_space(next) && !autolink_special_punctuation(next))
        } else {
            !super::is_space(character) && character != '[' && character != '>'
        };
        if !accepted {
            break;
        }
        end = position + character.len_utf8();
    }
    (end > scheme_len).then_some(end)
}

fn autolink_special_punctuation(character: char) -> bool {
    matches!(character, ',' | '.' | '?' | '!' | ':' | ';' | ')')
}

fn email_address(source: &str) -> Option<&str> {
    let local = source
        .chars()
        .take_while(|character| email_local_character(*character))
        .map(char::len_utf8)
        .sum::<usize>();
    if local == 0 || !source[local..].starts_with('@') {
        return None;
    }
    let domain = &source[local + 1..];
    let host = domain
        .chars()
        .take_while(|character| letter(*character))
        .map(char::len_utf8)
        .sum::<usize>();
    if host == 0 || !domain[host..].starts_with('.') {
        return None;
    }
    let suffix = &domain[host + 1..];
    let count = suffix
        .chars()
        .take_while(|character| letter(*character))
        .count();
    if !(2..=5).contains(&count) {
        return None;
    }
    let length = suffix
        .chars()
        .take(count)
        .map(char::len_utf8)
        .sum::<usize>();
    Some(&source[..local + 1 + host + 1 + length])
}

fn email_local_character(character: char) -> bool {
    alpha_numeric(character) || "_.+".contains(character)
}
