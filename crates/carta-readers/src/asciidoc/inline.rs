use super::attributes::{add_inline_attr, formatted_attributes, shorthand};
use super::{
    ATTRIBUTE_REFERENCE, IDENTIFIER_SOURCE, MONOSPACE_ATTRIBUTE, MONOSPACE_REFERENCE,
    MONOSPACE_SCOPE, REFERENCE_LABEL,
};
use super::{Attributes, Parser, class_attr, push_text, text_inlines};
use carta_ast::{Attr, Block, BlockParts, Inline, MathType, QuoteType, Table, Target};
use std::collections::BTreeMap;

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
        let mut ahead = Lookahead::new(source);
        while position < source.len() {
            let rest = &source[position..];
            let length = ahead.plain_length(position);
            if length > 0 {
                push_text(&mut output, &rest[..length]);
                position += length;
                continue;
            }
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
                && let Some(end) = ahead.next(Marker::Brace, position)
            {
                let end = end - position;
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
                && let Some((nodes, consumed)) =
                    self.attributed(&mut ahead, position, previous, hard_breaks)
            {
                finish_text(&mut output, &mut raw_hard_break);
                append(&mut output, nodes);
                position += consumed;
                replacement_start = position;
                continue;
            }
            if rest.starts_with("[[") {
                let bibliography = rest.starts_with("[[[")
                    && ahead.next(Marker::BibliographyAnchor, position).is_some();
                let (start, marker, close) = if bibliography {
                    (3, Marker::BibliographyAnchor, "]]]")
                } else {
                    (2, Marker::Anchor, "]]")
                };
                if let Some(end) = ahead.next(marker, position + start) {
                    let end = end - position - start;
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
                && let Some(end) = ahead.next(Marker::Reference, position)
                && ahead
                    .next(Marker::LineEnd, position)
                    .is_none_or(|line_end| end < line_end)
            {
                let end = end - position;
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
            // The character tests from here on only skip lookups that cannot match.
            if (character.is_ascii_lowercase() || matches!(character, '(' | '<'))
                && let Some((nodes, consumed)) = self.macro_inline(&mut ahead, position)
            {
                finish_text(&mut output, &mut raw_hard_break);
                append(&mut output, nodes);
                position += consumed;
                replacement_start = position;
                continue;
            }
            if position >= ahead.address_free && letter(character) {
                match email_address(rest) {
                    Ok(address) => {
                        let (nodes, consumed) =
                            self.email_link(&mut ahead, address, position + address.len());
                        finish_text(&mut output, &mut raw_hard_break);
                        append(&mut output, nodes);
                        position += consumed;
                        replacement_start = position;
                        continue;
                    }
                    Err(length) => ahead.address_free = position + length,
                }
            }
            if opens_formatting(character)
                && let Some((nodes, consumed)) = self.formatted(rest, previous, None, hard_breaks)
            {
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
            if matches!(character, ' ' | '.' | '(' | '-' | '<' | '=')
                && let Some((from, to)) =
                    replacements.iter().find(|(from, _)| rest.starts_with(from))
            {
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
            push_text(&mut output, &rest[..character.len_utf8()]);
            position += character.len_utf8();
        }
        output
    }

    /// Reads formatted text that an attribute list precedes, as in `[.role]#text#`.
    fn attributed(
        &mut self,
        ahead: &mut Lookahead,
        position: usize,
        previous: Option<char>,
        hard_breaks: bool,
    ) -> Option<(Vec<Inline>, usize)> {
        let source = ahead.source;
        let end = ahead.list_end(position, ListKind::Positional)?;
        let text = source.get(end + 1..)?;
        let after_word = follows_word(previous);
        if !text.starts_with(opens_formatting) || ahead.unformatted(end, after_word) {
            return None;
        }
        let attributes = formatted_attributes(source.get(position + 1..end)?);
        let Some((nodes, consumed)) = self.formatted(text, previous, Some(attributes), hard_breaks)
        else {
            ahead.set_unformatted(end, after_word);
            return None;
        };
        Some((nodes, end + 1 - position + consumed))
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
            if !unconstrained && follows_word(previous) {
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
    fn macro_inline(
        &mut self,
        ahead: &mut Lookahead,
        position: usize,
    ) -> Option<(Vec<Inline>, usize)> {
        let source = ahead.source.get(position..)?;
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
        let first = source.chars().next()?;
        if first.is_ascii_lowercase()
            && let Some(prefix) = names.iter().copied().find(|name| source.starts_with(name))
            && let Some(parsed) = self.named_macro(ahead, prefix, position + prefix.len())
        {
            return Some(parsed);
        }
        if first == '(' {
            // An index term runs to the first closing parenthesis, which starts its closing marker.
            let (open, close) = if source.starts_with("(((") {
                ("(((", ")))")
            } else {
                ("((", "))")
            };
            let rest = source.strip_prefix(open)?;
            let start = position + open.len();
            let end = ahead.next(Marker::Parenthesis, start)? - start;
            if end == 0 || !rest[end..].starts_with(close) {
                return None;
            }
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
        let angled = source.starts_with('<');
        let source_url = if angled { &source[1..] } else { source };
        let end = autolink_target_end(source_url)?;
        let target = &source_url[..end];
        let mut consumed = end;
        let mut attr = Attr::default();
        let mut label = vec![Inline::Str(target.into())];
        if let Some((description, attributes, count)) =
            link_description(ahead, position + usize::from(angled) + end)
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
        Some((
            vec![Inline::Link(
                Box::new(attr),
                label,
                Box::new(Target {
                    url: target.into(),
                    title: "".into(),
                }),
            )],
            consumed + usize::from(angled),
        ))
    }

    /// Links `address`, taking a bracketed description at offset `after` as its label.
    fn email_link(
        &mut self,
        ahead: &mut Lookahead,
        address: &str,
        after: usize,
    ) -> (Vec<Inline>, usize) {
        let mut consumed = address.len();
        let mut attr = Attr::default();
        let mut label = vec![Inline::Str(address.into())];
        if let Some((description, attributes, count)) = link_description(ahead, after) {
            attr = attributes;
            consumed += count;
            if !description.is_empty() {
                label = self.inlines(&description);
            }
        }
        (
            vec![Inline::Link(
                Box::new(attr),
                label,
                Box::new(Target {
                    url: address.into(),
                    title: "".into(),
                }),
            )],
            consumed,
        )
    }

    #[allow(
        clippy::too_many_lines,
        reason = "Named macros share target and attribute parsing."
    )]
    fn named_macro(
        &mut self,
        ahead: &mut Lookahead,
        prefix: &str,
        from: usize,
    ) -> Option<(Vec<Inline>, usize)> {
        let after = ahead.source.get(from..)?;
        // A target that starts here is known to reach no opening bracket.
        if ahead
            .targets_without_bracket
            .as_ref()
            .is_some_and(|targets| targets.contains(&from))
            || ahead.next(Marker::OpenBracket, from).is_none()
        {
            return None;
        }
        let (target, start) = read_macro_target(after, Some((ahead, from)))?;
        if !after[start..].starts_with('[') {
            // A target read character by character ends where every later start in it ends.
            if target.len() == start {
                ahead.targets_without_bracket = Some(from..=from + start);
            }
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
            ahead.bracket_end(from + start)?
        } else {
            ahead.list_end(from + start, ListKind::Named)?
        } - from;
        let content = &after[start + 1..end];
        let attributes = Attributes::parse(content);
        if prefix == "footnoteref:" && attributes.positional.is_empty() {
            return None;
        }
        let mut attr = attributes.attr();
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
                        attributes.positional.first().cloned().unwrap_or_default(),
                        attributes.positional.get(1).map_or("", String::as_str),
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

/// A marker that the scanner searches for ahead of its position.
#[derive(Clone, Copy)]
enum Marker {
    Brace,
    Reference,
    LineEnd,
    Parenthesis,
    Anchor,
    BibliographyAnchor,
    Bracket,
    OpenBracket,
    Comma,
    Equals,
    Quote,
}

/// How an attribute list reads its fields.
#[derive(Clone, Copy)]
pub(super) enum ListKind {
    /// Positional and named values, as in a macro or on a block attribute line.
    Named,
    /// Positional values only, as before formatted text.
    Positional,
    /// A link description, which only named values may follow.
    Description,
}

/// The fields a list may hold from some field on.
#[derive(Clone, Copy)]
enum Fields {
    Named,
    Positional,
    NamedOnly,
}

/// What follows a field of an attribute list.
enum Step {
    /// The bracket that closes the list, at this offset.
    End(usize),
    /// Another field, which starts at this offset.
    Next(usize),
}

/// Where a marker next occurs: the first one at or after `from` is at `found`.
#[derive(Clone, Copy)]
struct Next {
    from: usize,
    found: Option<usize>,
}

impl Next {
    const UNKNOWN: Self = Self {
        from: usize::MAX,
        found: None,
    };
}

/// A bracket on a line whose nesting is matched.
struct Bracket {
    offset: usize,
    /// Nesting depth after this bracket.
    depth: isize,
    /// Offset of the first later bracket that brings the nesting below `depth`.
    leaves: Option<usize>,
}

/// Where a list was found to end, by the offset of each field after a comma in it.
type ListEnds = BTreeMap<usize, Option<usize>>;

/// An attribute list end after which formatted text was sought and not found.
#[derive(Default)]
struct Unformatted {
    end: usize,
    /// For a list that follows a word character, which keeps some delimiters closed.
    after_word: bool,
    /// For any other list.
    elsewhere: bool,
}

/// What the scanner has learned about the source ahead of its position.
///
/// It finds text in which no inline construct starts, so the scanner passes it through whole, and
/// it remembers the outcome of each search for a closing marker, so no stretch of the source is
/// searched again from every opening marker in it. Offsets are positions in the scanned source.
pub(super) struct Lookahead<'a> {
    source: &'a str,
    /// No email address starts before this offset.
    address_free: usize,
    /// End of the word examined last.
    word_end: usize,
    /// Offsets where a macro target starts that reaches no opening bracket.
    targets_without_bracket: Option<std::ops::RangeInclusive<usize>>,
    brace: Next,
    reference: Next,
    line_end: Next,
    parenthesis: Next,
    anchor: Next,
    bibliography_anchor: Next,
    bracket: Next,
    open_bracket: Next,
    comma: Next,
    equals: Next,
    quote: Next,
    named_lists: ListEnds,
    positional_lists: ListEnds,
    named_only_lists: ListEnds,
    /// The last comma before an equals sign: the offsets searched between, and the comma.
    last_comma: (usize, usize, Option<usize>),
    unformatted: Unformatted,
    /// Stretch of one line, from the first bracket matched in it to its end.
    matched: std::ops::Range<usize>,
    /// The brackets that nest in that stretch, in order.
    brackets: Vec<Bracket>,
}

impl<'a> Lookahead<'a> {
    pub(super) fn new(source: &'a str) -> Self {
        Self {
            source,
            address_free: 0,
            word_end: 0,
            targets_without_bracket: None,
            brace: Next::UNKNOWN,
            reference: Next::UNKNOWN,
            line_end: Next::UNKNOWN,
            parenthesis: Next::UNKNOWN,
            anchor: Next::UNKNOWN,
            bibliography_anchor: Next::UNKNOWN,
            bracket: Next::UNKNOWN,
            open_bracket: Next::UNKNOWN,
            comma: Next::UNKNOWN,
            equals: Next::UNKNOWN,
            quote: Next::UNKNOWN,
            named_lists: ListEnds::new(),
            positional_lists: ListEnds::new(),
            named_only_lists: ListEnds::new(),
            last_comma: (usize::MAX, 0, None),
            unformatted: Unformatted::default(),
            matched: 0..0,
            brackets: Vec::new(),
        }
    }

    /// Length of the plain text at offset `position`.
    ///
    /// Zero means a construct may start at `position`.
    fn plain_length(&mut self, position: usize) -> usize {
        let Some(rest) = self.source.get(position..) else {
            return 0;
        };
        let character = rest.chars().next().unwrap_or_default();
        if starts_markup(character) {
            return 0;
        }
        if !alpha_numeric(character) {
            return rest
                .find(|character| starts_markup(character) || alpha_numeric(character))
                .unwrap_or(rest.len());
        }
        if position < self.word_end {
            return 0;
        }
        let word = rest
            .find(|character| !alpha_numeric(character))
            .unwrap_or(rest.len());
        self.word_end = position + word;
        if position >= self.address_free
            && letter(character)
            && let Err(length) = email_address(rest)
        {
            self.address_free = position + length;
        }
        // Inside a word only an address, or a macro name or URL scheme before a colon, can start.
        if self.address_free >= self.word_end && !rest[word..].starts_with(':') {
            word
        } else {
            0
        }
    }

    /// Offset of the first `marker` at or after `from`.
    fn next(&mut self, marker: Marker, from: usize) -> Option<usize> {
        let rest = self.source.get(from..)?;
        let (next, search): (&mut Next, fn(&str) -> Option<usize>) = match marker {
            Marker::Brace => (&mut self.brace, |text| text.find('}')),
            Marker::Reference => (&mut self.reference, |text| text.find(">>")),
            Marker::LineEnd => (&mut self.line_end, |text| text.find(['\n', '\r'])),
            Marker::Parenthesis => (&mut self.parenthesis, |text| text.find(')')),
            Marker::Anchor => (&mut self.anchor, |text| text.find("]]")),
            Marker::BibliographyAnchor => (&mut self.bibliography_anchor, |text| text.find("]]]")),
            Marker::Bracket => (&mut self.bracket, |text| text.find(']')),
            Marker::OpenBracket => (&mut self.open_bracket, |text| text.find('[')),
            Marker::Comma => (&mut self.comma, |text| text.find(',')),
            Marker::Equals => (&mut self.equals, |text| text.find('=')),
            Marker::Quote => (&mut self.quote, |text| text.find('"')),
        };
        if !(next.from <= from && next.found.is_none_or(|found| from <= found)) {
            *next = Next {
                from,
                found: search(rest).map(|offset| from + offset),
            };
        }
        next.found
    }

    /// Offset of the bracket that closes the attribute list opening at offset `at`.
    ///
    /// Nothing when the list is malformed or never closes.
    pub(super) fn list_end(&mut self, at: usize, kind: ListKind) -> Option<usize> {
        let first = at + 1;
        let (step, fields) = match kind {
            ListKind::Named if shorthand(self.source.get(first..)?) => {
                // A first field in shorthand is bare: it names no value and quotes none.
                let bracket = self.next(Marker::Bracket, first)?;
                let step = match self.next(Marker::Comma, first) {
                    Some(comma) if comma < bracket => Step::Next(self.skip_space(comma + 1)),
                    _ => Step::End(bracket),
                };
                (step, Fields::Named)
            }
            ListKind::Named => (self.field(first, Fields::Named)?, Fields::Named),
            ListKind::Positional => {
                let first = self.skip_space(first);
                (self.field(first, Fields::Positional)?, Fields::Positional)
            }
            ListKind::Description => return self.description_end(first),
        };
        match step {
            Step::End(end) => Some(end),
            Step::Next(start) => self.fields_end(start, fields),
        }
    }

    /// Offset of the bracket that closes a link description starting at offset `start`.
    fn description_end(&mut self, start: usize) -> Option<usize> {
        let source = self.source;
        if source.get(start..)?.starts_with('"')
            && let Some(close) = self.next(Marker::Quote, start + 1)
        {
            let after = close + 1;
            return match source.get(after..)?.chars().next()? {
                ']' => Some(after),
                ',' => self.fields_end(self.skip_space(after + 1), Fields::NamedOnly),
                _ => None,
            };
        }
        let bracket = self.next(Marker::Bracket, start)?;
        let Some(equals) = self
            .next(Marker::Equals, start)
            .filter(|equals| *equals < bracket)
        else {
            return Some(bracket);
        };
        // The text runs to the comma before the first named value and is not empty.
        let (from, before, _) = self.last_comma;
        if !(before == equals && from <= start) {
            let comma = source
                .get(start..equals)?
                .rfind(',')
                .map(|comma| start + comma);
            self.last_comma = (start, equals, comma);
        }
        let comma = self.last_comma.2.filter(|comma| *comma > start)?;
        self.fields_end(self.skip_space(comma + 1), Fields::NamedOnly)
    }

    /// Offset of the bracket that closes the list whose next field starts at offset `start`.
    ///
    /// Lists that open at different brackets share the fields after a comma, so the outcome is
    /// remembered for each such field.
    fn fields_end(&mut self, mut start: usize, fields: Fields) -> Option<usize> {
        let mut walked = Vec::new();
        let end = loop {
            if let Some(end) = self.list_ends(fields).get(&start) {
                break *end;
            }
            walked.push(start);
            match self.field(start, fields) {
                Some(Step::Next(next)) => start = next,
                Some(Step::End(end)) => break Some(end),
                None => break None,
            }
        };
        let ends = self.list_ends(fields);
        for start in walked {
            ends.insert(start, end);
        }
        end
    }

    /// Reads the field that starts at offset `start`: the list ends after it or goes on.
    fn field(&mut self, start: usize, fields: Fields) -> Option<Step> {
        let source = self.source;
        // A list of named values may end in an empty field.
        if matches!(fields, Fields::NamedOnly) && source.get(start..)?.starts_with(']') {
            return Some(Step::End(start));
        }
        let (delimiter, named) = self.field_end(start, !matches!(fields, Fields::Positional))?;
        if matches!(fields, Fields::NamedOnly) && !named {
            return None;
        }
        if !source.get(delimiter..)?.starts_with(']') {
            return Some(Step::Next(self.skip_space(delimiter + 1)));
        }
        // The last positional value may be absent, but not blank or an empty quote.
        let text = source.get(start..delimiter)?;
        let blank = !text.is_empty() && super::trim_space(text).is_empty();
        if matches!(fields, Fields::Named) && !named && (blank || text == "\"\"") {
            return None;
        }
        Some(Step::End(delimiter))
    }

    fn list_ends(&mut self, fields: Fields) -> &mut ListEnds {
        match fields {
            Fields::Named => &mut self.named_lists,
            Fields::Positional => &mut self.positional_lists,
            Fields::NamedOnly => &mut self.named_only_lists,
        }
    }

    /// Reads the field of an attribute list that starts at offset `start`.
    ///
    /// Gives the offset of the comma or closing bracket after the field, and whether the field
    /// names its value. Nothing when the field is malformed or the list never closes. A field
    /// ends at the first comma or bracket, but a value that opens with a quote, positional or
    /// right after the equals sign of a named one, runs to its closing quote.
    fn field_end(&mut self, start: usize, may_name: bool) -> Option<(usize, bool)> {
        let bracket = self.next(Marker::Bracket, start)?;
        let delimiter = self
            .next(Marker::Comma, start)
            .map_or(bracket, |comma| comma.min(bracket));
        let name_end = self
            .next(Marker::Equals, start)
            .filter(|equals| may_name && start < *equals && *equals < delimiter);
        let value = name_end.map_or(start, |equals| equals + 1);
        if self.source.get(value..)?.starts_with('"')
            && let Some(close) = self.next(Marker::Quote, value + 1)
        {
            let after = close + 1;
            return self
                .source
                .get(after..)?
                .starts_with([',', ']'])
                .then_some((after, name_end.is_some()));
        }
        Some((delimiter, name_end.is_some()))
    }

    /// Offset of the first character at or after `from` that is not a space.
    fn skip_space(&self, from: usize) -> usize {
        let rest = self.source.get(from..).unwrap_or_default();
        from + rest.len() - super::trim_space_start(rest).len()
    }

    /// Whether formatted text is known not to follow the attribute list that ends at `end`.
    fn unformatted(&self, end: usize, after_word: bool) -> bool {
        self.unformatted.end == end
            && if after_word {
                self.unformatted.after_word
            } else {
                self.unformatted.elsewhere
            }
    }

    fn set_unformatted(&mut self, end: usize, after_word: bool) {
        if self.unformatted.end != end {
            self.unformatted = Unformatted {
                end,
                ..Unformatted::default()
            };
        }
        if after_word {
            self.unformatted.after_word = true;
        } else {
            self.unformatted.elsewhere = true;
        }
    }

    /// Offset of the bracket that closes the raw macro content opening at offset `at`.
    ///
    /// The content ends on its line, at the first bracket that leaves the nesting it opens.
    fn bracket_end(&mut self, at: usize) -> Option<usize> {
        if !self.matched.contains(&at) {
            self.match_brackets(at);
        }
        let index = self
            .brackets
            .binary_search_by_key(&at, |bracket| bracket.offset)
            .ok()?;
        self.brackets.get(index)?.leaves
    }

    /// Records the nesting of the brackets from offset `from` to the end of its line.
    ///
    /// Every opening bracket nests; a closing bracket after a backslash closes nothing.
    fn match_brackets(&mut self, from: usize) {
        self.brackets.clear();
        let mut inside: Vec<usize> = Vec::new();
        let mut depth = 0_isize;
        let mut previous = '\0';
        let mut line_end = self.source.len();
        for (offset, character) in self.source.get(from..).unwrap_or_default().char_indices() {
            if matches!(character, '\n' | '\r') {
                line_end = from + offset;
                break;
            }
            if character == '[' || character == ']' && previous != '\\' {
                depth += if character == '[' { 1 } else { -1 };
                while let Some(bracket) = inside
                    .last()
                    .and_then(|index| self.brackets.get_mut(*index))
                    .filter(|bracket| bracket.depth > depth)
                {
                    bracket.leaves = Some(from + offset);
                    inside.pop();
                }
                inside.push(self.brackets.len());
                self.brackets.push(Bracket {
                    offset: from + offset,
                    depth,
                    leaves: None,
                });
            }
            previous = character;
        }
        self.matched = from..line_end;
    }
}

/// Whether formatted or quoted text can open with `character`.
fn opens_formatting(character: char) -> bool {
    matches!(
        character,
        '"' | '\'' | '*' | '_' | '`' | '#' | '+' | '~' | '^'
    )
}

/// Whether `previous` keeps a constrained delimiter from opening formatted text.
fn follows_word(previous: Option<char>) -> bool {
    previous.is_some_and(|c| !super::is_space(c) && !punctuation(c) && c != '+')
}

/// Whether an inline construct can start with this punctuation or whitespace character.
///
/// Letters and digits start only macro names, URL schemes, and email addresses, which
/// [`Lookahead::plain_length`] rules out word by word.
fn starts_markup(character: char) -> bool {
    matches!(
        character,
        ' ' | '\t'
            | '\n'
            | '"'
            | '#'
            | '&'
            | '\''
            | '('
            | '*'
            | '+'
            | '-'
            | '.'
            | '/'
            | '<'
            | '='
            | '['
            | '\\'
            | '^'
            | '_'
            | '`'
            | '{'
            | '~'
    )
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
    read_macro_target(source, None)
}

/// Reads a macro target up to its opening bracket.
///
/// An inline macro passes the lookahead of the scanned source and the offset of `source` in it;
/// only its target may hold a `pass:[]` span.
fn read_macro_target(
    source: &str,
    mut inline: Option<(&mut Lookahead, usize)>,
) -> Option<(String, usize)> {
    let mut target = String::new();
    let mut position = 0;
    while position < source.len() {
        let rest = &source[position..];
        if rest.starts_with('[') {
            return Some((target, position));
        }
        if let Some((ahead, base)) = inline.as_mut()
            && let Some(pass) = rest
                .strip_prefix("pass:[")
                .filter(|pass| !pass.starts_with(']'))
        {
            let start = *base + position + 6;
            let end = ahead.next(Marker::Bracket, start)? - start;
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

/// Reads the bracketed description of a link, when one opens at offset `at`.
fn link_description(ahead: &mut Lookahead, at: usize) -> Option<(String, Attr, usize)> {
    let source = ahead.source.get(at..)?;
    if !source.starts_with('[') {
        return None;
    }
    let end = ahead.list_end(at, ListKind::Description)? - at;
    let content = &source[1..end];
    let named = |fields: &str| Attributes::parse_rest(super::trim_space_start(fields)).attr();
    let (description, attributes) = if let Some(quoted) = content.strip_prefix('"')
        && let Some(close) = quoted.find('"')
    {
        let rest = &quoted[close + 1..];
        (&quoted[..close], rest.strip_prefix(',').map(named))
    } else {
        // Named values start at the comma before the first equals sign.
        let boundary = content
            .find('=')
            .and_then(|equals| content[..equals].rfind(','));
        boundary.map_or((content, None), |comma| {
            (&content[..comma], Some(named(&content[comma + 1..])))
        })
    };
    Some((
        description.to_owned(),
        attributes.unwrap_or_default(),
        end + 1,
    ))
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

/// Reads the email address at the start of `source`.
///
/// On failure, returns the length of the leading local part: no address starts inside it either.
fn email_address(source: &str) -> Result<&str, usize> {
    let local = source
        .chars()
        .take_while(|character| email_local_character(*character))
        .map(char::len_utf8)
        .sum::<usize>();
    if local == 0 || !source[local..].starts_with('@') {
        return Err(local);
    }
    let domain = &source[local + 1..];
    let host = domain
        .chars()
        .take_while(|character| letter(*character))
        .map(char::len_utf8)
        .sum::<usize>();
    if host == 0 || !domain[host..].starts_with('.') {
        return Err(local);
    }
    let suffix = &domain[host + 1..];
    let count = suffix
        .chars()
        .take_while(|character| letter(*character))
        .count();
    if !(2..=5).contains(&count) {
        return Err(local);
    }
    let length = suffix
        .chars()
        .take(count)
        .map(char::len_utf8)
        .sum::<usize>();
    Ok(&source[..local + 1 + host + 1 + length])
}

fn email_local_character(character: char) -> bool {
    alpha_numeric(character) || "_.+".contains(character)
}
