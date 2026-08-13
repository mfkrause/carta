//! Reader for Typst markup.
//!
//! Typst source is read in three interleaved modes. *Markup mode* is the default: text is literal
//! and a small set of characters introduce structure (`=` headings, `-`/`+`/`/` list markers,
//! `*`/`_` emphasis, backtick raw text, `<label>`, `@reference`, and the `~`/`--`/`---`/`...`
//! shorthands). *Code mode* is entered with `#` and covers literals, bindings, and the element
//! functions (`#figure`, `#table`, `#link`, `#footnote`, …) that carry the constructs markup has no
//! syntax for. *Math mode* is entered with `$` and is translated to TeX, which is what the document
//! model stores for a [`carta_ast::Inline::Math`] payload.
//!
//! Block structure is indentation-driven: a list item owns every following line indented past its
//! marker, so nesting needs no explicit terminator. Inline scanning is a single left-to-right pass
//! with a pending-text buffer; a delimiter that never closes is emitted as literal text rather than
//! failing the parse.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use fancy_regex::Regex;

use carta_ast::{
    Alignment, Attr, Block, Caption, Cell, Citation, CitationMode, ColSpec, ColWidth, Document,
    Inline, ListAttributes, ListNumberDelim, ListNumberStyle, MathType, MetaValue, QuoteType, Row,
    Table, TableBody, TableFoot, TableHead, Target, Text,
};
use carta_core::{DeepStack, Error, Extension, Reader, ReaderOptions, Result, on_deep_stack};

mod data;
mod emoji;
mod integer;
mod show;
#[cfg(test)]
mod tests;

use integer::Integer;

/// The deepest level of nesting that recursive parsing follows. Beyond it, would-be delimiters are
/// taken literally, bounding stack use on adversarial input.
const MAX_DEPTH: usize = 64;

/// Bound on the values one loop walks over, so a runaway range cannot exhaust memory. It sits far
/// above any count a document plausibly writes out.
const MAX_ITERATIONS: usize = 1 << 20;

/// Document nodes a parse may copy or repeat, per character of source. Reading a binding or
/// repeating a sequence duplicates content, so this keeps materialized output proportional to the
/// source length.
const COPIES_PER_CHAR: usize = 64;

/// The materialization allowance every parse starts with, so that reuse within a short source, and
/// the long loops a few lines can spell out, never run into the ceiling.
const BASE_COPIES: usize = 1 << 20;

/// Parses Typst markup into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct TypstReader;

impl Reader for TypstReader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document> {
        // Markup, code, and math recurse as deeply as the source nests, so the parse needs room.
        match on_deep_stack(|| parse_document(input, options)) {
            DeepStack::Completed(document) => Ok(document),
            DeepStack::Panicked => Err(Error::Container("worker thread failed".into())),
            DeepStack::NotSpawned => Ok(parse_document(input, options)),
        }
    }
}

/// Parses a whole source text into the document model.
fn parse_document(input: &str, options: &ReaderOptions) -> Document {
    let mut parser = Parser::new(normalize(input), options.source_dir.clone());
    let mut blocks = parser.blocks(0);
    walk_inlines(&mut blocks, &mut merge_text_runs);
    walk_inlines(&mut blocks, &mut collapse_separators);
    resolve_references(&mut blocks, std::mem::take(&mut parser.attached));
    walk_inlines(&mut blocks, &mut merge_citations);
    if options.extensions.contains(Extension::EastAsianLineBreaks) {
        strip_wide_line_breaks(&mut blocks);
    }
    Document {
        meta: std::mem::take(&mut parser.meta),
        blocks,
        ..Document::default()
    }
}

/// Fold CRLF and CR line endings to LF and drop a leading byte-order mark.
fn normalize(input: &str) -> Vec<char> {
    let body = input.strip_prefix('\u{feff}').unwrap_or(input);
    let mut out = Vec::with_capacity(body.len());
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            other => out.push(other),
        }
    }
    out
}

/// A value produced by a code-mode expression.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    /// No content at all: a statement, a layout primitive, or an unsupported call.
    Nothing,
    /// A boolean literal.
    Bool(bool),
    /// An integer literal, of unbounded width.
    Int(Integer),
    /// A number carrying a unit suffix (`1cm`, `50%`, `1fr`); the suffix may be empty.
    Number(f64, String),
    /// A string literal.
    Str(String),
    /// A bare identifier standing for an enum-like setting (`center`, `red`, `auto`).
    Ident(String),
    /// A label literal (`<key>`).
    Label(String),
    /// A comma-separated sequence written in parentheses.
    Array(Vec<Value>),
    /// A mapping written as `(name: value, ..)`, in the order the pairs were written.
    Dict(Vec<(String, Value)>),
    /// Block-level content: a markup block or a block-producing element function.
    Content(Vec<Block>),
    /// Inline-level content produced by an element function.
    Inlines(Vec<Inline>),
    /// A table sub-element (`table.header`, `table.cell`, …) awaiting its enclosing table.
    Group(GroupKind, Vec<Arg>),
    /// A function, together with the arguments a `.with(..)` has already fixed to it. It stands
    /// for whatever it computes once the remaining arguments arrive.
    Function(Callee, Vec<Arg>),
    /// A regular expression, as the pattern it was written with.
    Regex(String),
}

/// What a [`Value::Function`] calls.
#[derive(Debug, Clone, PartialEq)]
enum Callee {
    /// A closure, as the parameters and body it was written with.
    Closure(Function),
    /// A function referred to by name, which may be an element function or a `#let` binding.
    Named(String),
}

/// A parameter of a `#let` binding: the name it binds, the source range of the default it falls
/// back to, and whether it collects the arguments no other parameter takes.
#[derive(Debug, Clone, PartialEq)]
struct Parameter {
    name: String,
    default: Option<(usize, usize)>,
    spread: bool,
}

/// A `#let`-bound function: its parameters and the source range holding its body.
#[derive(Debug, Clone, PartialEq)]
struct Function {
    parameters: Vec<Parameter>,
    body: usize,
    limit: usize,
}

/// Where a control-flow keyword sends the evaluation under way.
#[derive(Debug, Clone, PartialEq)]
enum Flow {
    /// `break`: leave the innermost loop.
    Break,
    /// `continue`: start the innermost loop's next round.
    Continue,
    /// `return`: leave the enclosing function with this value.
    Return(Value),
}

/// The body of a control-flow construct, as the source range between its delimiters.
#[derive(Debug, Clone, Copy)]
enum Body {
    /// A `[..]` markup block.
    Markup(usize, usize),
    /// A `{..}` code block.
    Code(usize, usize),
}

/// Which table sub-element a [`Value::Group`] carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroupKind {
    /// `table.header(..)`: its cells form the header rows.
    Header,
    /// `table.footer(..)`: its cells form the footer rows.
    Footer,
    /// `table.cell(..)`: one cell with span and alignment options.
    Cell,
    /// A rule primitive (`table.hline`, `table.vline`) that contributes no cell.
    Rule,
}

/// One argument of a code-mode call: positional when unnamed.
#[derive(Debug, Clone, PartialEq)]
struct Arg {
    /// The argument's name, for `name: value` arguments.
    name: Option<String>,
    /// The argument's value.
    value: Value,
}

impl Value {
    /// Whether the value stands on its own as one or more blocks rather than folding into the
    /// surrounding paragraph.
    fn is_block(&self) -> bool {
        match self {
            // A lone paragraph still flows into the lines around it; anything else stands apart.
            Value::Content(blocks) => {
                blocks.len() > 1
                    || blocks
                        .iter()
                        .any(|block| !matches!(block, Block::Para(_) | Block::Plain(_)))
            }
            Value::Group(..) => true,
            _ => false,
        }
    }

    /// The value as inline content, flattening block content and rendering literals as text.
    fn into_inlines(self) -> Vec<Inline> {
        match self {
            Value::Inlines(inlines) => inlines,
            Value::Content(blocks) => blocks_to_inlines(blocks),
            Value::Str(text) => text_inlines(&text),
            // A label marks the place it stands in, so it carries an identifier and no text.
            Value::Label(name) => vec![Inline::Span(
                Box::new(Attr {
                    id: name.as_str().into(),
                    ..Attr::default()
                }),
                Vec::new(),
            )],
            Value::Int(n) => text_inlines(&n.to_string()),
            Value::Number(n, unit) => text_inlines(&format_number(n, &unit)),
            Value::Bool(b) => text_inlines(if b { "true" } else { "false" }),
            Value::Array(items) => text_inlines(&array_repr(&items)),
            Value::Dict(pairs) => text_inlines(&dict_repr(&pairs)),
            // A name that reached content position went unresolved, so it sets nothing.
            Value::Ident(_)
            | Value::Nothing
            | Value::Group(..)
            | Value::Function(..)
            | Value::Regex(_) => Vec::new(),
        }
    }

    /// The value as block content.
    fn into_blocks(self) -> Vec<Block> {
        match self {
            Value::Content(mut blocks) => {
                // Block edges separate nothing, so the separators a content block carried go away.
                if let Some(inlines) = first_edge_inlines(&mut blocks)
                    && matches!(inlines.first(), Some(Inline::Space | Inline::SoftBreak))
                {
                    inlines.remove(0);
                }
                if let Some(inlines) = last_edge_inlines(&mut blocks) {
                    trim_trailing_space(inlines);
                }
                blocks
                    .retain(|block| !matches!(block, Block::Plain(inlines) if inlines.is_empty()));
                blocks
            }
            Value::Nothing | Value::Group(..) => Vec::new(),
            other => {
                let inlines = other.into_inlines();
                if inlines.is_empty() {
                    Vec::new()
                } else {
                    vec![Block::Para(inlines)]
                }
            }
        }
    }

    /// The value as plain text, for arguments that name a file, a URL, or a setting.
    fn as_text(&self) -> String {
        match self {
            Value::Str(text) | Value::Ident(text) | Value::Label(text) => text.clone(),
            Value::Int(n) => n.to_string(),
            Value::Number(n, unit) => format_number(*n, unit),
            Value::Bool(b) => (if *b { "true" } else { "false" }).to_string(),
            Value::Content(blocks) => carta_ast::to_plain_text(&blocks_to_inlines(blocks.clone())),
            Value::Inlines(inlines) => carta_ast::to_plain_text(inlines),
            Value::Array(items) => items
                .iter()
                .map(Value::as_text)
                .collect::<Vec<_>>()
                .join(", "),
            Value::Dict(pairs) => dict_repr(pairs),
            Value::Regex(pattern) => format!("/{pattern}/"),
            Value::Nothing | Value::Group(..) | Value::Function(..) => String::new(),
        }
    }

    /// Whether the value counts as true where a condition is expected.
    fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Int(n) => !n.is_zero(),
            Value::Number(n, _) => *n != 0.0,
            Value::Nothing => false,
            other => !other.as_text().is_empty(),
        }
    }

    /// The value read as a number, whatever numeric shape it was written in.
    fn as_number(&self) -> Option<f64> {
        match self {
            Value::Int(n) => Some(n.to_f64()),
            Value::Number(n, _) => Some(*n),
            _ => None,
        }
    }
}

/// One node still to be counted while a value's weight is summed.
enum Weighed<'a> {
    Value(&'a Value),
    Block(&'a Block),
    Inline(&'a Inline),
    Text(usize),
}

/// How much document a value holds: one unit per node plus the length of the text it carries, so a
/// value dominated by a single long string is weighed by that string rather than by its one node.
fn value_weight(value: &Value) -> usize {
    let mut total = 0usize;
    let mut pending = vec![Weighed::Value(value)];
    while let Some(node) = pending.pop() {
        total = total.saturating_add(1);
        match node {
            Weighed::Text(length) => total = total.saturating_add(length),
            Weighed::Value(value) => push_value(value, &mut pending),
            Weighed::Block(block) => push_block(block, &mut pending),
            Weighed::Inline(inline) => push_inline(inline, &mut pending),
        }
    }
    total
}

fn push_value<'a>(value: &'a Value, pending: &mut Vec<Weighed<'a>>) {
    match value {
        Value::Content(blocks) => pending.extend(blocks.iter().map(Weighed::Block)),
        Value::Inlines(inlines) => pending.extend(inlines.iter().map(Weighed::Inline)),
        Value::Array(items) => pending.extend(items.iter().map(Weighed::Value)),
        Value::Dict(entries) => pending.extend(
            entries
                .iter()
                .flat_map(|(key, item)| [Weighed::Text(key.len()), Weighed::Value(item)]),
        ),
        Value::Group(_, args) | Value::Function(_, args) => {
            pending.extend(args.iter().map(|arg| Weighed::Value(&arg.value)));
        }
        Value::Str(text) | Value::Ident(text) | Value::Label(text) | Value::Regex(text) => {
            pending.push(Weighed::Text(text.len()));
        }
        Value::Nothing | Value::Bool(_) | Value::Int(_) | Value::Number(..) => {}
    }
}

fn push_block<'a>(block: &'a Block, pending: &mut Vec<Weighed<'a>>) {
    match block {
        Block::Plain(inlines) | Block::Para(inlines) | Block::Header(_, _, inlines) => {
            pending.extend(inlines.iter().map(Weighed::Inline));
        }
        Block::LineBlock(lines) => pending.extend(lines.iter().flatten().map(Weighed::Inline)),
        Block::CodeBlock(_, text) => pending.push(Weighed::Text(text.len())),
        Block::RawBlock(format, text) => {
            pending.push(Weighed::Text(format.0.len().saturating_add(text.len())));
        }
        Block::BlockQuote(inner) | Block::Div(_, inner) => {
            pending.extend(inner.iter().map(Weighed::Block));
        }
        Block::OrderedList(_, items) | Block::BulletList(items) => {
            pending.extend(items.iter().flatten().map(Weighed::Block));
        }
        Block::DefinitionList(entries) => {
            for (term, definitions) in entries {
                pending.extend(term.iter().map(Weighed::Inline));
                pending.extend(definitions.iter().flatten().map(Weighed::Block));
            }
        }
        Block::Figure(_, caption, inner) => {
            push_caption(caption, pending);
            pending.extend(inner.iter().map(Weighed::Block));
        }
        Block::Table(table) => push_table(table, pending),
        Block::HorizontalRule => {}
    }
}

fn push_table<'a>(table: &'a Table, pending: &mut Vec<Weighed<'a>>) {
    push_caption(&table.caption, pending);
    let sections = table
        .head
        .rows
        .iter()
        .chain(
            table
                .bodies
                .iter()
                .flat_map(|body| body.head.iter().chain(body.body.iter())),
        )
        .chain(table.foot.rows.iter());
    for row in sections {
        pending.extend(
            row.cells
                .iter()
                .flat_map(|cell| cell.content.iter())
                .map(Weighed::Block),
        );
    }
}

fn push_caption<'a>(caption: &'a Caption, pending: &mut Vec<Weighed<'a>>) {
    pending.extend(caption.short.iter().flatten().map(Weighed::Inline));
    pending.extend(caption.long.iter().map(Weighed::Block));
}

fn push_inline<'a>(inline: &'a Inline, pending: &mut Vec<Weighed<'a>>) {
    match inline {
        Inline::Emph(inner)
        | Inline::Underline(inner)
        | Inline::Strong(inner)
        | Inline::Strikeout(inner)
        | Inline::Superscript(inner)
        | Inline::Subscript(inner)
        | Inline::SmallCaps(inner)
        | Inline::Quoted(_, inner)
        | Inline::Span(_, inner) => pending.extend(inner.iter().map(Weighed::Inline)),
        Inline::Str(text) | Inline::Code(_, text) | Inline::Math(_, text) => {
            pending.push(Weighed::Text(text.len()));
        }
        Inline::RawInline(format, text) => {
            pending.push(Weighed::Text(format.0.len().saturating_add(text.len())));
        }
        Inline::Cite(citations, inner) => {
            for citation in citations {
                pending.push(Weighed::Text(citation.id.len()));
                pending.extend(citation.prefix.iter().map(Weighed::Inline));
                pending.extend(citation.suffix.iter().map(Weighed::Inline));
            }
            pending.extend(inner.iter().map(Weighed::Inline));
        }
        Inline::Link(_, inner, target) | Inline::Image(_, inner, target) => {
            pending.push(Weighed::Text(
                target.url.len().saturating_add(target.title.len()),
            ));
            pending.extend(inner.iter().map(Weighed::Inline));
        }
        Inline::Note(blocks) => pending.extend(blocks.iter().map(Weighed::Block)),
        Inline::Space | Inline::SoftBreak | Inline::LineBreak => {}
    }
}

/// Render a float carrying an optional unit. A ratio counts whole percent only; every other
/// measure keeps the decimal notation of [`show_double`], trailing `.0` included.
fn format_number(value: f64, unit: &str) -> String {
    if unit == "%" {
        return format!("{}%", value.floor());
    }
    format!("{}{unit}", show_double(value))
}

/// The value of a `calc` constant, if the name is one.
fn calc_constant(name: &str) -> Option<f64> {
    match name {
        "calc.pi" => Some(std::f64::consts::PI),
        "calc.tau" => Some(std::f64::consts::TAU),
        "calc.e" => Some(std::f64::consts::E),
        _ => None,
    }
}

/// Evaluate a `calc` function over its positional arguments; an unmodelled name yields nothing.
fn calc_call(function: &str, args: &[Arg]) -> Value {
    let values: Vec<&Value> = args
        .iter()
        .filter(|arg| arg.name.is_none())
        .map(|arg| &arg.value)
        .collect();
    if let Some(exact) = calc_exact(function, &values) {
        return exact;
    }
    let integral = values.iter().all(|value| matches!(value, Value::Int(_)));
    let numbers: Vec<f64> = values
        .iter()
        .filter_map(|value| value.as_number())
        .collect();
    let (first, second) = (numbers.first().copied(), numbers.get(1).copied());
    match (function, first, second) {
        ("abs", Some(x), _) => scalar(x.abs(), integral),
        ("sqrt", Some(x), _) => Value::Number(x.sqrt(), String::new()),
        ("floor", Some(x), _) => whole(x.floor()),
        ("ceil", Some(x), _) => whole(x.ceil()),
        ("trunc", Some(x), _) => whole(x.trunc()),
        ("fract", Some(x), _) => scalar(x.fract(), integral),
        ("round", Some(x), _) => round_to_digits(x, args),
        ("min", Some(_), _) => extreme(&values, Ordering::Less),
        ("max", Some(_), _) => extreme(&values, Ordering::Greater),
        ("pow", Some(base), Some(exponent)) => power(base, exponent, integral),
        ("rem", Some(x), Some(divisor)) if divisor != 0.0 => scalar(x % divisor, integral),
        ("quo", Some(x), Some(divisor)) if divisor != 0.0 => whole((x / divisor).trunc()),
        ("gcd", Some(x), Some(other)) => whole(greatest_common_divisor(x.abs(), other.abs())),
        ("even", Some(x), _) => Value::Bool(x % 2.0 == 0.0),
        ("odd", Some(x), _) => Value::Bool(x % 2.0 != 0.0),
        ("fact", Some(x), _) => factorial(x),
        ("clamp", Some(x), Some(low)) => {
            let high = numbers.get(2).copied().unwrap_or(f64::INFINITY);
            scalar(x.max(low).min(high), integral)
        }
        ("sin", Some(x), _) => real(x.sin()),
        ("cos", Some(x), _) => real(x.cos()),
        ("tan", Some(x), _) => real(x.tan()),
        ("sinh", Some(x), _) => real(x.sinh()),
        ("cosh", Some(x), _) => real(x.cosh()),
        ("tanh", Some(x), _) => real(x.tanh()),
        ("asin", Some(x), _) => angle(x.asin()),
        ("acos", Some(x), _) => angle(x.acos()),
        ("atan", Some(x), _) => angle(x.atan()),
        ("atan2", Some(x), Some(y)) => angle(x.atan2(y)),
        ("exp", Some(x), _) => real(x.exp()),
        ("ln", Some(x), _) => real(x.ln()),
        ("log", Some(x), _) => {
            let base = named(args, "base")
                .and_then(Value::as_number)
                .unwrap_or(10.0);
            real(x.ln() / base.ln())
        }
        ("lcm", Some(x), Some(other)) => {
            let divisor = greatest_common_divisor(x.abs(), other.abs());
            if divisor == 0.0 {
                whole(0.0)
            } else {
                whole((x * other / divisor).abs())
            }
        }
        ("binom", Some(n), Some(k)) => whole(arrangements(n, k) / factorial_of(k)),
        ("perm", Some(n), Some(k)) => whole(arrangements(n, k)),
        ("norm", Some(_), _) => real(numbers.iter().map(|x| x * x).sum::<f64>().sqrt()),
        _ => Value::Nothing,
    }
}

/// Evaluate a `calc` function over whole arguments without losing a digit, or `None` when the
/// function or its arguments are not that shape.
fn calc_exact(function: &str, values: &[&Value]) -> Option<Value> {
    let whole: Vec<&Integer> = values
        .iter()
        .filter_map(|value| match value {
            Value::Int(n) => Some(n),
            _ => None,
        })
        .collect();
    if whole.len() != values.len() {
        return None;
    }
    let first = whole.first().copied()?;
    let second = whole.get(1).copied();
    Some(match (function, second) {
        ("abs", _) => Value::Int(first.abs()),
        ("floor" | "ceil" | "trunc" | "round", _) => Value::Int(first.clone()),
        ("fract", _) => Value::Int(Integer::zero()),
        ("even", _) => Value::Bool(first.is_even()),
        ("odd", _) => Value::Bool(!first.is_even()),
        ("fact", _) => {
            let count = u32::try_from(first.to_i64().unwrap_or_default().max(0)).ok()?;
            Value::Int(Integer::checked_factorial(count)?)
        }
        ("pow", Some(exponent)) => {
            Value::Int(first.checked_pow(u32::try_from(exponent.to_i64()?).ok()?)?)
        }
        ("rem", Some(divisor)) => Value::Int(first.divide(divisor)?.1),
        ("quo", Some(divisor)) => Value::Int(first.divide(divisor)?.0),
        ("gcd", Some(other)) => Value::Int(first.greatest_common_divisor(other)),
        ("lcm", Some(other)) => {
            let divisor = first.greatest_common_divisor(other);
            match first.multiply(other).abs().divide(&divisor) {
                Some((multiple, _)) => Value::Int(multiple),
                None => Value::Int(Integer::zero()),
            }
        }
        ("perm", Some(taken)) => Value::Int(exact_arrangements(first, taken)?),
        ("binom", Some(taken)) => {
            let count = u32::try_from(taken.to_i64().unwrap_or_default().max(0)).ok()?;
            let orderings = Integer::checked_factorial(count)?;
            Value::Int(exact_arrangements(first, taken)?.divide(&orderings)?.0)
        }
        ("clamp", Some(low)) => {
            let high = whole.get(2).copied();
            let bounded = first.max(low);
            Value::Int(high.map_or(bounded, |high| bounded.min(high)).clone())
        }
        _ => return None,
    })
}

/// The number of ordered ways `taken` of `count` whole items can be drawn, or `None` once the
/// product would outgrow what evaluation will build.
fn exact_arrangements(count: &Integer, taken: &Integer) -> Option<Integer> {
    if taken.is_negative() || taken > count {
        return Some(Integer::zero());
    }
    let steps = taken.to_usize().filter(|steps| *steps <= MAX_ITERATIONS)?;
    let mut product = Integer::one();
    for drawn in 0..steps {
        product = product.checked_multiply(&count.subtract(&Integer::from(drawn)))?;
    }
    Some(product)
}

/// A real-valued result, which stays a float however whole its operands were.
fn real(value: f64) -> Value {
    Value::Number(value, String::new())
}

/// An angle, whose magnitude is the number of radians it turns through.
fn angle(radians: f64) -> Value {
    Value::Number(radians, "deg".to_string())
}

/// The number of ordered ways `k` of `n` items can be drawn, and zero once `k` outgrows `n`.
fn arrangements(n: f64, k: f64) -> f64 {
    let (n, k) = (n.trunc(), k.trunc());
    if k < 0.0 || k > n {
        return 0.0;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let count = (k as usize).min(MAX_ITERATIONS);
    let mut product = 1.0;
    for taken in 0..count {
        #[allow(clippy::cast_precision_loss)]
        let step = taken as f64;
        product *= n - step;
    }
    product
}

/// The factorial of a whole count, as a real number.
fn factorial_of(count: f64) -> f64 {
    let whole = count.trunc().max(0.0);
    arrangements(whole, whole).max(1.0)
}

/// A numeric result, kept whole when every operand was.
fn scalar(value: f64, integral: bool) -> Value {
    if integral {
        whole(value)
    } else {
        Value::Number(value, String::new())
    }
}

/// A whole number as an integer value.
fn whole(value: f64) -> Value {
    Value::Int(Integer::from_f64(value))
}

/// Round to a number of decimal places, resolving a tie towards the even digit.
fn round_to_digits(value: f64, args: &[Arg]) -> Value {
    let digits = named(args, "digits")
        .and_then(Value::as_number)
        .unwrap_or(0.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let places = digits.clamp(0.0, 15.0) as usize;
    let rounded = format!("{value:.places$}").parse::<f64>().unwrap_or(value);
    if places == 0 {
        whole(rounded)
    } else {
        Value::Number(rounded, String::new())
    }
}

/// The argument holding the smallest or largest number, keeping its own type.
fn extreme(values: &[&Value], keep: Ordering) -> Value {
    let mut best: Option<&Value> = None;
    for value in values {
        if value.as_number().is_none() {
            continue;
        }
        match best {
            Some(current) if numeric_order(value, current) != Some(keep) => {}
            _ => best = Some(value),
        }
    }
    best.map_or(Value::Nothing, |value| (*value).clone())
}

/// A power, kept whole while both operands are whole and the result still fits.
fn power(base: f64, exponent: f64, integral: bool) -> Value {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    if integral
        && (0.0..64.0).contains(&exponent)
        && let Some(result) = (base as i64).checked_pow(exponent as u32)
    {
        return Value::Int(Integer::from(result));
    }
    Value::Number(base.powf(exponent), String::new())
}

/// The greatest common divisor of two whole values, by repeated remainder.
fn greatest_common_divisor(mut value: f64, mut other: f64) -> f64 {
    /// Euclid's algorithm converges well inside this many rounds for every finite pair.
    const MAX_ROUNDS: usize = 512;

    for _ in 0..MAX_ROUNDS {
        if other < 1.0 {
            break;
        }
        let rest = value % other;
        value = other;
        other = rest;
    }
    value
}

/// A factorial, or nothing once the product outgrows the integer range.
fn factorial(value: f64) -> Value {
    #[allow(clippy::cast_possible_truncation)]
    let count = value.max(0.0).min(f64::from(u32::MAX)) as i64;
    let mut product: i64 = 1;
    for factor in 2..=count {
        match product.checked_mul(factor) {
            Some(next) => product = next,
            None => return Value::Nothing,
        }
    }
    Value::Int(Integer::from(product))
}

/// Wrap plain text as inline nodes, splitting on spaces so words stay separate `Str` runs.
fn text_inlines(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut word = String::new();
    for c in text.chars() {
        let separator = match c {
            ' ' | '\t' => Inline::Space,
            '\n' | '\r' => Inline::SoftBreak,
            _ => {
                word.push(c);
                continue;
            }
        };
        if !word.is_empty() {
            out.push(Inline::Str(word.as_str().into()));
            word.clear();
        }
        out.push(separator);
    }
    if !word.is_empty() {
        out.push(Inline::Str(word.as_str().into()));
    }
    out
}

/// Flatten block content to inline content, joining consecutive blocks with a line break. The
/// entries of a list and the cells of a table run together, since each already stands apart in the
/// text it holds.
fn blocks_to_inlines(blocks: Vec<Block>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    for block in blocks {
        let part = match block {
            Block::Para(inlines) | Block::Plain(inlines) | Block::Header(_, _, inlines) => inlines,
            Block::Div(_, children) | Block::BlockQuote(children) => blocks_to_inlines(children),
            Block::CodeBlock(attr, code) => vec![Inline::Code(attr, code)],
            Block::BulletList(items) | Block::OrderedList(_, items) => {
                items.into_iter().flat_map(blocks_to_inlines).collect()
            }
            Block::DefinitionList(entries) => entries
                .into_iter()
                .flat_map(|(term, definitions)| {
                    let mut entry = term;
                    entry.push(Inline::Str(":".into()));
                    entry.push(Inline::Space);
                    entry.extend(definitions.into_iter().flat_map(blocks_to_inlines));
                    entry
                })
                .collect(),
            Block::Table(table) => table_to_inlines(*table),
            _ => continue,
        };
        if part.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(Inline::LineBreak);
        }
        out.extend(part);
    }
    out
}

/// Flatten a table to inline content, one line break per row boundary.
fn table_to_inlines(table: Table) -> Vec<Inline> {
    let sections = table
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
    let mut out: Vec<Inline> = Vec::new();
    for row in sections {
        let cells: Vec<Inline> = row
            .cells
            .into_iter()
            .flat_map(|cell| blocks_to_inlines(cell.content))
            .collect();
        if cells.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(Inline::LineBreak);
        }
        out.extend(cells);
    }
    out
}

/// The value a member of a standard module names on its own: a constant or a glyph.
fn module_value(module: &str, name: &str) -> Option<Value> {
    if let Some(constant) = calc_constant(&format!("{module}.{name}")) {
        return Some(Value::Number(constant, String::new()));
    }
    matches!(module, "sym" | "math")
        .then(|| symbol(name))
        .flatten()
        .map(|entry| Value::Inlines(vec![Inline::Str(entry.glyph.into())]))
}

/// The name an imported binding takes locally, or nothing when the import leaves it behind.
fn adopted_name(list: &ImportList, name: &str, stem: &str) -> Option<String> {
    match list {
        ImportList::Module => Some(format!("{stem}.{name}")),
        ImportList::All => Some(name.to_string()),
        ImportList::Named(names) => names.iter().find_map(|(from, local)| {
            // An empty source name stands for the module itself, renamed by `as`.
            if from.is_empty() {
                Some(format!("{local}.{name}"))
            } else {
                (from == name).then(|| local.clone())
            }
        }),
    }
}

/// The numbering an enumeration carries when no `numbering` pattern selects another.
fn default_enumeration() -> ListAttributes {
    ListAttributes {
        start: 1,
        style: ListNumberStyle::DefaultStyle,
        delim: ListNumberDelim::DefaultDelim,
    }
}

/// The list numbering `numbering:` and `start:` arguments select, keeping whatever the enclosing
/// settings already chose for the ones they leave out.
fn enumeration_from(args: &[Arg], current: &ListAttributes) -> ListAttributes {
    let (style, delim) = named(args, "numbering").map_or((current.style, current.delim), |value| {
        numbering_markers(&value.as_text())
            .unwrap_or((ListNumberStyle::DefaultStyle, ListNumberDelim::DefaultDelim))
    });
    ListAttributes {
        start: named(args, "start")
            .and_then(Value::as_number)
            .map_or(current.start, |value| {
                #[allow(clippy::cast_possible_truncation)]
                let start = value as i64;
                start
            }),
        style,
        delim,
    }
}

/// The numeral style and delimiter a numbering pattern stands for. Only a lone counter symbol with
/// the punctuation the document model can express carries over; any richer pattern is a run of
/// literal text with no equivalent, so it selects nothing.
fn numbering_markers(pattern: &str) -> Option<(ListNumberStyle, ListNumberDelim)> {
    let (symbol, delim) = match pattern.strip_prefix('(') {
        Some(rest) => (rest.strip_suffix(')')?, ListNumberDelim::TwoParens),
        None => match pattern.get(..pattern.len().checked_sub(1)?) {
            Some(head) => match pattern.chars().last()? {
                '.' => (head, ListNumberDelim::Period),
                ')' => (head, ListNumberDelim::OneParen),
                _ => return None,
            },
            None => return None,
        },
    };
    let style = match symbol {
        "1" => ListNumberStyle::Decimal,
        "a" => ListNumberStyle::LowerAlpha,
        "A" => ListNumberStyle::UpperAlpha,
        "i" => ListNumberStyle::LowerRoman,
        "I" => ListNumberStyle::UpperRoman,
        _ => return None,
    };
    Some((style, delim))
}

/// The markup parser: a cursor over the source characters plus the bindings and metadata that
/// code-mode statements accumulate.
struct Parser {
    /// Every character the parse may read: the document, then each file `#include` or `#import`
    /// pulled in, appended so a binding can keep pointing at the range holding its body.
    source: Vec<char>,
    /// The read cursor.
    pos: usize,
    /// One past the last character the current (possibly nested) parse may read.
    limit: usize,
    /// Current recursion depth, against [`MAX_DEPTH`].
    depth: usize,
    /// Bindings introduced by `#let`.
    env: BTreeMap<String, Value>,
    /// The names the innermost code block bound, which go away again when it closes.
    declared: Vec<String>,
    /// Document metadata gathered from `#set document(..)`.
    meta: BTreeMap<Text, MetaValue>,
    /// Labels absorbed as heading identifiers, which references still resolve against.
    attached: BTreeSet<Text>,
    /// Emphasis openings already found to have no closer, keyed by position and read limit.
    unclosed: BTreeSet<(usize, usize)>,
    /// Functions bound by `#let name(..) = ..`, as parameter names and where the body starts.
    functions: BTreeMap<String, Function>,
    /// The code expression a paragraph's lookahead already read, as its start, value, and end.
    evaluated: Option<(usize, Value, usize)>,
    /// The directory the source file was named under, which a referenced file resolves against.
    /// Absent when the source came from a stream, leaving references as written.
    base: Option<PathBuf>,
    /// The files whose text is currently being parsed, so a cycle of references terminates.
    open: BTreeSet<PathBuf>,
    /// Where an included file's own trailing newline ended a line, closing the paragraph that the
    /// `#include` sits in.
    line_closed: Option<usize>,
    /// The full path each name imported from a standard module stands for.
    aliases: BTreeMap<String, String>,
    /// The standard modules imported whole, searched in turn for a name nothing else resolves.
    globs: Vec<String>,
    /// The list numbering `#set enum(..)` selects for the enumerations that follow.
    enumeration: ListAttributes,
    /// The continuation threshold of the block region being read, which is how far a show rule
    /// reaches.
    indent: usize,
    /// Whether the expression being read ends at the line end, as a statement written in markup
    /// does. Inside brackets an expression spreads over as many lines as it likes.
    line_bound: bool,
    /// The blocks a mid-line code expression set, which end the paragraph they were written in.
    interrupt: Option<Vec<Block>>,
    /// The pending exit a `return`, `break`, or `continue` requested, which the loop or call it
    /// leaves clears again.
    flow: Option<Flow>,
    /// How much copied or repeated content may still materialize. Without a ceiling, a few
    /// operators or bindings can fill memory.
    copies: usize,
}

/// A file pulled into the character arena by `#include` or `#import`.
struct Loaded {
    /// The offset its characters start at.
    start: usize,
    /// One past its last character.
    end: usize,
    /// The directory it sits in, which its own references resolve against.
    base: Option<PathBuf>,
    /// Its path, held open so a reference cycle terminates.
    path: PathBuf,
}

/// What an `#import` brings into scope.
enum ImportList {
    /// No list, so the module answers to its own name (`#import "x.typ"` then `x.name`).
    Module,
    /// `*`: every name the module binds.
    All,
    /// The listed bindings, each with the name it takes locally.
    Named(Vec<(String, String)>),
}

impl Parser {
    fn new(source: Vec<char>, base: Option<PathBuf>) -> Self {
        let limit = source.len();
        let copies = BASE_COPIES.saturating_add(limit.saturating_mul(COPIES_PER_CHAR));
        Self {
            source,
            pos: 0,
            limit,
            depth: 0,
            env: BTreeMap::new(),
            declared: Vec::new(),
            meta: BTreeMap::new(),
            attached: BTreeSet::new(),
            unclosed: BTreeSet::new(),
            functions: BTreeMap::new(),
            evaluated: None,
            base,
            open: BTreeSet::new(),
            line_closed: None,
            aliases: BTreeMap::new(),
            globs: Vec::new(),
            enumeration: default_enumeration(),
            indent: 0,
            line_bound: false,
            interrupt: None,
            flow: None,
            copies,
        }
    }

    /// The character at an absolute offset, or `None` past the current limit.
    fn at(&self, index: usize) -> Option<char> {
        if index < self.limit {
            self.source.get(index).copied()
        } else {
            None
        }
    }

    fn peek(&self) -> Option<char> {
        self.at(self.pos)
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.at(self.pos.saturating_add(offset))
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos = self.pos.saturating_add(1);
        }
        c
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos = self.pos.saturating_add(1);
            true
        } else {
            false
        }
    }

    /// Whether the source at `index` begins with the given ASCII word.
    fn matches(&self, index: usize, word: &str) -> bool {
        word.chars()
            .enumerate()
            .all(|(offset, c)| self.at(index.saturating_add(offset)) == Some(c))
    }

    /// The text between two offsets.
    fn slice(&self, start: usize, end: usize) -> String {
        self.source
            .get(start..end.min(self.limit))
            .unwrap_or_default()
            .iter()
            .collect()
    }

    /// Parse a sub-range as its own markup region, keeping bindings and metadata shared.
    fn sub_blocks(&mut self, start: usize, end: usize) -> Vec<Block> {
        if self.depth >= MAX_DEPTH {
            return vec![Block::Plain(text_inlines(&self.slice(start, end)))];
        }
        let (saved_pos, saved_limit) = (self.pos, self.limit);
        self.pos = start;
        self.limit = end.min(saved_limit);
        self.depth = self.depth.saturating_add(1);
        let blocks = self.blocks(0);
        self.depth = self.depth.saturating_sub(1);
        self.pos = saved_pos;
        self.limit = saved_limit;
        blocks
    }

    /// Parse a sub-range as inline content.
    fn sub_inlines(&mut self, start: usize, end: usize) -> Vec<Inline> {
        blocks_to_inlines(self.sub_blocks(start, end))
    }

    // Block level

    /// Parse blocks whose lines are indented by at least `min_indent` columns. The cursor must sit
    /// at the start of a line and is left at the first line that falls outside the region.
    fn blocks(&mut self, min_indent: usize) -> Vec<Block> {
        let mut out: Vec<Block> = Vec::new();
        let outer = std::mem::replace(&mut self.indent, min_indent);
        let mut heading: Option<usize> = None;
        loop {
            self.skip_blank_lines();
            if self.peek().is_none() {
                break;
            }
            let (indent, content) = self.measure_indent();
            if indent < min_indent && !out.is_empty() {
                break;
            }
            self.pos = content;
            let mut produced = match self.block_construct(indent, min_indent) {
                Some(blocks) => blocks,
                None => self.paragraph(min_indent),
            };
            if let Some(index) = heading {
                self.attach_heading_label(&mut out, index, &mut produced);
            }
            heading = match produced.last() {
                // Reading the last block passes over a heading whose own line already took a label.
                Some(Block::Header(..)) => {
                    Some(out.len().saturating_add(produced.len()).saturating_sub(1))
                }
                // An emptied label block leaves the heading open to the label after it.
                _ => heading.filter(|_| produced.is_empty()),
            };
            out.append(&mut produced);
        }
        self.indent = outer;
        out
    }

    /// Move a label standing alone after a heading into the heading's identifier, dropping the
    /// block it leaves empty.
    fn attach_heading_label(
        &mut self,
        out: &mut [Block],
        heading: usize,
        produced: &mut Vec<Block>,
    ) {
        let Some(Block::Para(inlines)) = produced.first_mut() else {
            return;
        };
        let Some(id) = take_leading_label(inlines) else {
            return;
        };
        if inlines.is_empty() {
            produced.remove(0);
        }
        self.attached.insert(id.clone());
        if let Some(Block::Header(_, attr, _)) = out.get_mut(heading)
            && attr.id.is_empty()
        {
            attr.id = id;
        }
    }

    /// Skip over runs of whitespace-only lines.
    fn skip_blank_lines(&mut self) {
        loop {
            let (_, content) = self.measure_indent();
            match self.at(content) {
                Some('\n') => self.pos = content.saturating_add(1),
                None if content > self.pos => {
                    self.pos = content;
                    break;
                }
                _ => break,
            }
        }
    }

    /// The indentation width of the current line and the offset of its first non-space character.
    fn measure_indent(&self) -> (usize, usize) {
        let mut index = self.pos;
        let mut width = 0usize;
        while let Some(c) = self.at(index) {
            match c {
                ' ' => width = width.saturating_add(1),
                '\t' => width = width.saturating_add(2),
                _ => break,
            }
            index = index.saturating_add(1);
        }
        (width, index)
    }

    /// Whether the cursor sits where a line begins.
    fn at_line_start(&self) -> bool {
        self.pos == 0 || self.at(self.pos.saturating_sub(1)) == Some('\n')
    }

    /// Whether the current line is blank or past the end of the region.
    fn at_blank_line(&self) -> bool {
        let (_, content) = self.measure_indent();
        matches!(self.at(content), None | Some('\n'))
    }

    /// Recognise and parse a block opened by the line's first characters. The cursor sits just past
    /// the line's indentation. `min_indent` is the continuation threshold of the enclosing region,
    /// for the paragraph a code expression opening the line may turn out to start.
    fn block_construct(&mut self, indent: usize, min_indent: usize) -> Option<Vec<Block>> {
        match self.peek()? {
            '=' => self.heading(),
            '-' if self.is_marker(1) => Some(vec![self.bullet_list(indent)]),
            '+' if self.is_marker(1) => Some(vec![self.enum_list(indent)]),
            '/' if self.is_marker(1) => Some(vec![self.term_list(indent)]),
            '0'..='9' => self.numbered_list(indent),
            '`' if self.matches(self.pos, "```") => Some(vec![self.raw_block()]),
            '#' => Some(self.code_block_line(min_indent)),
            _ => None,
        }
    }

    /// Whether a list marker at the cursor is followed by the space (or line end) that makes it one.
    fn is_marker(&self, offset: usize) -> bool {
        matches!(self.peek_at(offset), None | Some(' ' | '\t' | '\n'))
    }

    fn heading(&mut self) -> Option<Vec<Block>> {
        let start = self.pos;
        let mut level = 0usize;
        while self.eat('=') {
            level = level.saturating_add(1);
        }
        if !matches!(self.peek(), Some(' ' | '\t')) {
            self.pos = start;
            return None;
        }
        self.skip_spaces();
        let mut inlines = self.inline_run(None).unwrap_or_default();
        self.eat('\n');
        // A label closes the heading, so it stands after it rather than inside it.
        let mut trailing = Vec::new();
        while let Some(Inline::Span(attr, children)) = inlines.last()
            && children.is_empty()
            && !attr.id.is_empty()
        {
            if let Some(label) = inlines.pop() {
                trailing.insert(0, label);
            }
            while matches!(inlines.last(), Some(Inline::Space | Inline::SoftBreak)) {
                inlines.pop();
            }
        }
        let mut out = vec![Block::Header(
            i64::try_from(level).unwrap_or(i64::MAX),
            Box::default(),
            inlines,
        )];
        if !trailing.is_empty() {
            out.push(Block::Para(trailing));
        }
        Some(out)
    }

    fn bullet_list(&mut self, indent: usize) -> Block {
        let items = self.list_items(indent, ListMarker::Bullet);
        Block::BulletList(items.into_iter().map(|(_, blocks)| blocks).collect())
    }

    fn enum_list(&mut self, indent: usize) -> Block {
        let items = self.list_items(indent, ListMarker::Enum);
        Block::OrderedList(
            self.enumeration.clone(),
            items.into_iter().map(|(_, blocks)| blocks).collect(),
        )
    }

    fn numbered_list(&mut self, indent: usize) -> Option<Vec<Block>> {
        let mut index = self.pos;
        while matches!(self.at(index), Some('0'..='9')) {
            index = index.saturating_add(1);
        }
        if index == self.pos || self.at(index) != Some('.') {
            return None;
        }
        if !matches!(
            self.at(index.saturating_add(1)),
            None | Some(' ' | '\t' | '\n')
        ) {
            return None;
        }
        let start = self.slice(self.pos, index).parse::<i64>().unwrap_or(1);
        let items = self.list_items(indent, ListMarker::Enum);
        Some(vec![Block::OrderedList(
            ListAttributes {
                start,
                ..self.enumeration.clone()
            },
            items.into_iter().map(|(_, blocks)| blocks).collect(),
        )])
    }

    fn term_list(&mut self, indent: usize) -> Block {
        let items = self.list_items(indent, ListMarker::Term);
        Block::DefinitionList(
            items
                .into_iter()
                .map(|(term, blocks)| (term, vec![blocks]))
                .collect(),
        )
    }

    /// Collect the consecutive items of one list: each item's term (term lists only) and body.
    fn list_items(&mut self, indent: usize, marker: ListMarker) -> Vec<(Vec<Inline>, Vec<Block>)> {
        let mut items = Vec::new();
        loop {
            let Some(marker_width) = self.item_marker(marker) else {
                break;
            };
            self.pos = self.pos.saturating_add(marker_width);
            let (spaces, content) = self.measure_indent();
            let content_indent = indent.saturating_add(marker_width).saturating_add(spaces);
            self.pos = content;
            let term = if marker == ListMarker::Term {
                self.item_term()
            } else {
                Vec::new()
            };
            let blocks = self.item_body(content_indent, indent.saturating_add(1));
            items.push((term, blocks));
            self.skip_blank_lines();
            let (next_indent, next_content) = self.measure_indent();
            if next_indent != indent {
                break;
            }
            self.pos = next_content;
            if self.item_marker(marker).is_none() {
                self.pos = self.pos.saturating_sub(next_indent.min(self.pos));
                break;
            }
        }
        items
    }

    /// The width of the item marker at the cursor, when the cursor opens an item of this list kind.
    fn item_marker(&self, marker: ListMarker) -> Option<usize> {
        match marker {
            ListMarker::Bullet => (self.peek() == Some('-') && self.is_marker(1)).then_some(1),
            ListMarker::Term => (self.peek() == Some('/') && self.is_marker(1)).then_some(1),
            ListMarker::Enum => {
                if self.peek() == Some('+') && self.is_marker(1) {
                    return Some(1);
                }
                let mut index = self.pos;
                while matches!(self.at(index), Some('0'..='9')) {
                    index = index.saturating_add(1);
                }
                if index == self.pos || self.at(index) != Some('.') {
                    return None;
                }
                let width = index.saturating_add(1).saturating_sub(self.pos);
                self.is_marker(width).then_some(width)
            }
        }
    }

    /// Read a term-list item's term, up to the `:` that separates it from the description.
    fn item_term(&mut self) -> Vec<Inline> {
        let start = self.pos;
        let mut index = self.pos;
        let mut nesting = 0usize;
        while let Some(c) = self.at(index) {
            match c {
                '\n' => break,
                ':' if nesting == 0 => {
                    let term = self.sub_inlines(start, index);
                    self.pos = index.saturating_add(1);
                    self.skip_spaces();
                    return term;
                }
                '[' | '(' => nesting = nesting.saturating_add(1),
                ']' | ')' => nesting = nesting.saturating_sub(1),
                '\\' => index = index.saturating_add(1),
                _ => {}
            }
            index = index.saturating_add(1);
        }
        Vec::new()
    }

    /// Read a list item's body, starting at content sitting `content_indent` columns in.
    ///
    /// The body runs on over every following line indented at least `continuation` columns, which
    /// is one past the marker's own column, so a line need not reach the content column to belong.
    fn item_body(&mut self, content_indent: usize, continuation: usize) -> Vec<Block> {
        let mut out = Vec::new();
        let outer = std::mem::replace(&mut self.indent, continuation);
        let mut indent = content_indent;
        loop {
            if let Some(mut blocks) = self.block_construct(indent, continuation) {
                out.append(&mut blocks);
            } else if self.at_blank_line() {
                self.eat('\n');
            } else {
                out.append(&mut self.paragraph(continuation));
            }
            self.skip_blank_lines();
            if self.peek().is_none() {
                break;
            }
            let (next, content) = self.measure_indent();
            if next < continuation {
                break;
            }
            indent = next;
            self.pos = content;
        }
        self.indent = outer;
        out
    }

    fn raw_block(&mut self) -> Block {
        let (language, body) = self.raw_span();
        code_block(&language, &body)
    }

    /// A code-mode expression opening a line: the blocks it sets, or the paragraph the rest of the
    /// line reads as once the expression turns out to stand inline.
    fn code_block_line(&mut self, min_indent: usize) -> Vec<Block> {
        let value = self.code_expression();
        if !value.is_block() && self.line_closed != Some(self.pos) && !self.at_line_start() {
            let mut opening = value.into_inlines();
            if matches!(opening.first(), Some(Inline::Space | Inline::SoftBreak)) {
                opening.remove(0);
            }
            return self.paragraph_from(opening, min_indent);
        }
        let mut out = value.into_blocks();
        // An expression that read as far as a line start took the rest of its own line with it.
        if self.at_line_start() {
            return out;
        }
        self.skip_spaces();
        if matches!(self.peek(), None | Some('\n')) {
            self.eat('\n');
        } else {
            let rest = self.paragraph(0);
            out.extend(rest);
        }
        out
    }

    /// Read a paragraph: inline content up to a blank line, a line that opens another block, or a
    /// line that falls out of the region.
    fn paragraph(&mut self, min_indent: usize) -> Vec<Block> {
        self.paragraph_from(Vec::new(), min_indent)
    }

    /// Read a paragraph whose first line already carries `opening` inline content.
    fn paragraph_from(&mut self, opening: Vec<Inline>, min_indent: usize) -> Vec<Block> {
        let mut out = Vec::new();
        let mut inlines = Vec::new();
        let mut opening = Some(opening);
        loop {
            let started = opening.take().unwrap_or_default();
            let mut line = self
                .inline_run_from(started, None, true)
                .unwrap_or_default();
            if !line.is_empty() {
                // A hard break already separated the lines it stands between.
                if !inlines.is_empty() && !matches!(inlines.last(), Some(Inline::LineBreak)) {
                    inlines.push(Inline::SoftBreak);
                }
                inlines.append(&mut line);
            }
            // Blocks written mid-line close the paragraph, and the rest of the line opens a new one.
            if let Some(blocks) = self.interrupt.take() {
                trim_trailing_space(&mut inlines);
                if !inlines.is_empty() {
                    out.push(Block::Para(std::mem::take(&mut inlines)));
                }
                out.extend(blocks);
                self.skip_spaces();
                continue;
            }
            if !self.eat('\n') {
                break;
            }
            if self.at_blank_line() {
                break;
            }
            let (indent, content) = self.measure_indent();
            if indent < min_indent {
                break;
            }
            let saved = self.pos;
            self.pos = content;
            if self.opens_block() {
                self.pos = saved;
                break;
            }
        }
        if !inlines.is_empty() {
            out.push(Block::Para(inlines));
        }
        out
    }

    /// Whether the cursor, sitting past a line's indentation, opens a block that interrupts an open
    /// paragraph.
    fn opens_block(&mut self) -> bool {
        match self.peek() {
            Some('#') => self.starts_block_code(),
            Some('=') => {
                let mut index = self.pos;
                while self.at(index) == Some('=') {
                    index = index.saturating_add(1);
                }
                index > self.pos && matches!(self.at(index), Some(' ' | '\t'))
            }
            Some('-' | '+' | '/') => self.is_marker(1),
            Some('`') => self.matches(self.pos, "```"),
            Some('0'..='9') => {
                let mut index = self.pos;
                while matches!(self.at(index), Some('0'..='9')) {
                    index = index.saturating_add(1);
                }
                self.at(index) == Some('.')
                    && matches!(self.at(index.saturating_add(1)), Some(' ' | '\t'))
            }
            _ => false,
        }
    }

    /// Whether the code expression at the cursor sets a block rather than inline content. The value
    /// is kept for the parse that follows, which reads the same expression at the same position.
    fn starts_block_code(&mut self) -> bool {
        let start = self.pos;
        let value = self.code_expression();
        let end = self.pos;
        self.pos = start;
        let block = value.is_block() || self.line_closed == Some(end);
        self.evaluated = Some((start, value, end));
        block
    }

    // Inline level

    fn skip_spaces(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.pos = self.pos.saturating_add(1);
        }
    }

    /// Step over what separates a keyword from its operand, stopping at the line end that would
    /// close the statement instead.
    fn skip_operand_space(&mut self) {
        loop {
            match self.peek() {
                Some(' ' | '\t') => self.pos = self.pos.saturating_add(1),
                Some('/') if self.matches(self.pos, "//") => self.skip_line_comment(),
                Some('/') if self.matches(self.pos, "/*") => self.skip_block_comment(),
                _ => break,
            }
        }
    }

    /// Whether the cursor sits where the current statement already ends, so no operand follows.
    fn ends_statement(&self) -> bool {
        matches!(self.peek(), None | Some('\n' | ';' | '}' | ']' | ')'))
    }

    /// Scan inline content to the end of the line, or to the given emphasis closer.
    ///
    /// Returns `None` when a closer was expected but the region ended first, so the caller can
    /// rewind and take the opening delimiter literally.
    fn inline_run(&mut self, closer: Option<char>) -> Option<Vec<Inline>> {
        self.inline_run_from(Vec::new(), closer, false)
    }

    /// Take the rest of the line as literal text, the fallback once nesting reaches [`MAX_DEPTH`].
    /// A run that owes its caller a closer still reports failure, but one that does not must
    /// consume something so the block scan keeps advancing.
    fn literal_line(&mut self, mut out: Vec<Inline>, closer: Option<char>) -> Option<Vec<Inline>> {
        closer.is_none().then(|| {
            let start = self.pos;
            while !matches!(self.peek(), None | Some('\n')) {
                self.bump();
            }
            let text = self.slice(start, self.pos);
            out.extend(text_inlines(text.trim_end()));
            trim_trailing_space(&mut out);
            out
        })
    }

    /// Scan inline content onto an already-started run, as [`inline_run`](Self::inline_run) does.
    ///
    /// `interruptible` says whether the caller can place blocks beside the run, which lets a code
    /// expression that sets blocks end the run instead of folding into it.
    fn inline_run_from(
        &mut self,
        out: Vec<Inline>,
        closer: Option<char>,
        interruptible: bool,
    ) -> Option<Vec<Inline>> {
        if self.depth >= MAX_DEPTH {
            return self.literal_line(out, closer);
        }
        let mut out: Vec<Inline> = out;
        let mut text = String::new();
        let mut shorthand = false;
        loop {
            let Some(c) = self.peek() else {
                return if closer.is_none() {
                    flush(&mut text, &mut out);
                    trim_trailing_space(&mut out);
                    Some(out)
                } else {
                    None
                };
            };
            if Some(c) == closer && self.toggles_emphasis() {
                self.bump();
                flush(&mut text, &mut out);
                return Some(out);
            }
            let after_shorthand = std::mem::replace(&mut shorthand, false);
            match c {
                '\n' => {
                    if closer.is_none() {
                        flush(&mut text, &mut out);
                        trim_trailing_space(&mut out);
                        return Some(out);
                    }
                    self.bump();
                    if self.at_blank_line() {
                        return None;
                    }
                    flush(&mut text, &mut out);
                    trim_trailing_space(&mut out);
                    self.skip_spaces();
                    // A hard break already separated the lines it stands between.
                    if !matches!(out.last(), Some(Inline::LineBreak)) {
                        out.push(Inline::SoftBreak);
                    }
                }
                ' ' | '\t' => {
                    self.skip_spaces();
                    flush(&mut text, &mut out);
                    let opening = out.is_empty() && closer.is_none();
                    if !opening && !matches!(out.last(), Some(Inline::Space)) {
                        out.push(Inline::Space);
                    }
                }
                '\\' => self.escape(&mut text, &mut out),
                '*' | '_' => {
                    if let Some(node) = self.emphasis(c) {
                        flush(&mut text, &mut out);
                        out.push(node);
                    } else {
                        self.bump();
                        text.push(c);
                    }
                }
                '`' => {
                    flush(&mut text, &mut out);
                    let (_, body) = self.raw_span();
                    out.push(Inline::Code(Box::default(), body.into()));
                }
                '$' => {
                    flush(&mut text, &mut out);
                    out.push(self.math());
                }
                '<' => {
                    if let Some(name) = self.label() {
                        flush(&mut text, &mut out);
                        out.push(Inline::Span(
                            Box::new(Attr {
                                id: name.into(),
                                ..Attr::default()
                            }),
                            Vec::new(),
                        ));
                        self.skip_spaces();
                    } else {
                        self.bump();
                        text.push('<');
                    }
                }
                '@' => {
                    if let Some(node) = self.reference() {
                        flush(&mut text, &mut out);
                        out.push(node);
                    } else {
                        self.bump();
                        text.push('@');
                    }
                }
                '#' => {
                    flush(&mut text, &mut out);
                    if self.splice_ends_run(&mut out, closer, interruptible) {
                        return Some(out);
                    }
                }
                '/' if self.matches(self.pos, "//") => self.skip_line_comment(),
                '/' if self.matches(self.pos, "/*") => self.skip_block_comment(),
                '~' => {
                    self.bump();
                    text.push('\u{a0}');
                    shorthand = true;
                }
                '-' => shorthand = self.dash(&mut text),
                '.' if self.matches(self.pos, "...") => {
                    self.pos = self.pos.saturating_add(3);
                    text.push('\u{2026}');
                    shorthand = true;
                }
                '"' | '\'' => {
                    self.bump();
                    let opening = self.quote_opens(c, &text, &out, after_shorthand);
                    text.push(smart_quote(c, opening));
                    shorthand = true;
                }
                'h' if self.at_url_start(&text, &out) => {
                    flush(&mut text, &mut out);
                    out.push(self.autolink());
                }
                _ => {
                    self.bump();
                    text.push(c);
                }
            }
        }
    }

    /// Splice the code expression at the cursor into an inline run, reporting whether it ended the
    /// run: either it set blocks, or it read as far as a line start and took the rest of the line.
    fn splice_ends_run(
        &mut self,
        out: &mut Vec<Inline>,
        closer: Option<char>,
        interruptible: bool,
    ) -> bool {
        self.splice_code_value(out, closer, interruptible);
        let ended = self.interrupt.is_some() || (closer.is_none() && self.at_line_start());
        if ended {
            trim_trailing_space(out);
        }
        ended
    }

    /// Splice the value of the code expression at the cursor into an inline run, dropping a
    /// leading separator where the run already ends in one. Block content is set aside instead,
    /// where the caller can place it beside the run.
    fn splice_code_value(&mut self, out: &mut Vec<Inline>, closer: Option<char>, blocks: bool) {
        let value = self.code_expression();
        if blocks && matches!(value, Value::Content(_)) && value.is_block() {
            self.interrupt = Some(value.into_blocks());
            return;
        }
        let mut inlines = value.into_inlines();
        let separated = match out.last() {
            Some(last) => matches!(last, Inline::Space | Inline::SoftBreak),
            None => closer.is_none(),
        };
        if separated && matches!(inlines.first(), Some(Inline::Space | Inline::SoftBreak)) {
            let leading = inlines.remove(0);
            // Where two separators meet, the one that spans a line outweighs the one that does not.
            if matches!(leading, Inline::SoftBreak) && matches!(out.last(), Some(Inline::Space)) {
                out.pop();
                out.push(leading);
            }
        }
        out.extend(inlines);
    }

    /// Whether the `*` or `_` at the cursor toggles emphasis: one inside a word is plain text.
    fn toggles_emphasis(&self) -> bool {
        let before = self
            .pos
            .checked_sub(1)
            .and_then(|index| self.source.get(index).copied());
        let after = self.peek_at(1);
        !(before.is_some_and(is_word_char) && after.is_some_and(is_word_char))
    }

    /// Parse `*strong*` or `_emphasis_`, or `None` when the run never closes.
    fn emphasis(&mut self, delimiter: char) -> Option<Inline> {
        if !self.toggles_emphasis() {
            return None;
        }
        let start = self.pos;
        // Re-searching a known-unclosed opening would cost exponential time on nested delimiters.
        if self.unclosed.contains(&(start, self.limit)) {
            return None;
        }
        self.bump();
        self.depth = self.depth.saturating_add(1);
        let inner = self.inline_run(Some(delimiter));
        self.depth = self.depth.saturating_sub(1);
        match inner {
            Some(children) if delimiter == '*' => Some(Inline::Strong(children)),
            Some(children) => Some(Inline::Emph(children)),
            None => {
                self.unclosed.insert((start, self.limit));
                self.pos = start;
                None
            }
        }
    }

    /// Read a backtick-delimited raw span, returning its language tag and body.
    fn raw_span(&mut self) -> (String, String) {
        let mut ticks = 0usize;
        while self.eat('`') {
            ticks = ticks.saturating_add(1);
        }
        let mut language = String::new();
        // Only an identifier tags the language; anything else is the first characters of the body.
        if ticks >= 3 && self.peek().is_some_and(|c| c.is_alphabetic() || c == '_') {
            while let Some(c) = self.peek() {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    language.push(c);
                    self.bump();
                } else {
                    break;
                }
            }
        }
        // Room left between a fence and what follows it belongs to the fence, not to the body.
        if ticks >= 3 {
            while self.peek().is_some_and(|c| c == ' ' || c == '\t') {
                self.bump();
            }
        }
        let start = self.pos;
        let mut end = None;
        let mut index = self.pos;
        while index < self.limit {
            if self.at(index) == Some('`') {
                let mut run = 0usize;
                while self.at(index.saturating_add(run)) == Some('`') {
                    run = run.saturating_add(1);
                }
                if run >= ticks {
                    end = Some((index, index.saturating_add(run)));
                    break;
                }
                index = index.saturating_add(run);
                continue;
            }
            index = index.saturating_add(1);
        }
        let (body_end, after) = end.unwrap_or((self.limit, self.limit));
        self.pos = after;
        if ticks < 3 {
            return (language, self.slice(start, body_end));
        }
        // A fenced body drops the line break that opens it, and hangs off the indentation of the
        // line its closing fence sits on.
        let (indent, body_end) = self.closing_indent(start, body_end);
        let body = self.slice(start, body_end);
        let body = body.strip_prefix('\n').unwrap_or(&body);
        (language, dedent(body, indent))
    }

    /// The indentation the closing fence at `body_end` hangs off, paired with the body end that
    /// leaves that fence's own line out. A fence sharing a line with content indents nothing.
    fn closing_indent(&self, start: usize, body_end: usize) -> (usize, usize) {
        let mut index = body_end;
        let mut indent = 0usize;
        while index > start {
            let width = match self.at(index.saturating_sub(1)) {
                Some(' ') => 1,
                Some('\t') => 2,
                _ => break,
            };
            indent = indent.saturating_add(width);
            index = index.saturating_sub(1);
        }
        match self.at(index.saturating_sub(1)) {
            Some('\n') if index > start => (indent, index.saturating_sub(1)),
            _ => (0, body_end),
        }
    }

    /// Read `$…$`, translating the Typst math body to TeX.
    fn math(&mut self) -> Inline {
        let open = self.pos;
        self.bump();
        let start = self.pos;
        let mut index = self.pos;
        let mut end = None;
        while index < self.limit {
            match self.at(index) {
                Some('\\') => index = index.saturating_add(2),
                Some('$') => {
                    end = Some(index);
                    break;
                }
                Some(_) => index = index.saturating_add(1),
                None => break,
            }
        }
        let Some(close) = end else {
            self.pos = open.saturating_add(1);
            return Inline::Str("$".into());
        };
        self.pos = close.saturating_add(1);
        let body = self.slice(start, close);
        let padded = body.starts_with([' ', '\n']) && body.ends_with([' ', '\n']);
        let kind = if padded && !body.trim().is_empty() {
            MathType::DisplayMath
        } else {
            MathType::InlineMath
        };
        Inline::Math(kind, math_to_tex(&body).into())
    }

    /// Read a `<label>`, or `None` when the angle brackets do not enclose an identifier.
    fn label(&mut self) -> Option<String> {
        let end = self.label_end(self.pos)?;
        let name = self.slice(self.pos.saturating_add(1), end.saturating_sub(1));
        self.pos = end;
        Some(name)
    }

    /// Where a `<label>` written at `from` ends, if one is written there.
    fn label_end(&self, from: usize) -> Option<usize> {
        let start = from.saturating_add(1);
        let mut index = start;
        while let Some(c) = self.at(index) {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ':') {
                index = index.saturating_add(1);
            } else {
                break;
            }
        }
        (index > start && self.at(index) == Some('>')).then(|| index.saturating_add(1))
    }

    /// Read an `@key` reference, or `None` when no identifier follows.
    fn reference(&mut self) -> Option<Inline> {
        let mut index = self.pos.saturating_add(1);
        let start = index;
        while let Some(c) = self.at(index) {
            if c.is_alphanumeric() || matches!(c, '-' | '_') {
                index = index.saturating_add(1);
            } else {
                break;
            }
        }
        if index == start {
            return None;
        }
        let key = self.slice(start, index);
        self.pos = index;
        if self.peek() == Some('[')
            && let Some(close) = self.balanced('[', ']')
        {
            // The bracketed supplement stands in for the key wherever the reference resolves.
            let body = self.sub_inlines(self.pos.saturating_add(1), close);
            self.pos = close.saturating_add(1);
            self.skip_spaces();
            return Some(reference(&key, body));
        }
        Some(reference(
            &key,
            vec![Inline::Str(format!("[{key}]").as_str().into())],
        ))
    }

    /// Handle a backslash escape: a Unicode codepoint, a hard line break, or a literal character.
    fn escape(&mut self, text: &mut String, out: &mut Vec<Inline>) {
        self.bump();
        match self.peek() {
            None | Some('\n' | ' ' | '\t') => {
                if self.peek() == Some('\n') {
                    self.bump();
                }
                self.skip_spaces();
                flush(text, out);
                if matches!(out.last(), Some(Inline::Space)) {
                    out.pop();
                }
                out.push(Inline::LineBreak);
            }
            Some('u') if self.peek_at(1) == Some('{') => {
                let start = self.pos.saturating_add(2);
                let mut index = start;
                while self.at(index).is_some_and(|c| c.is_ascii_hexdigit()) {
                    index = index.saturating_add(1);
                }
                if self.at(index) == Some('}') && index > start {
                    let digits = self.slice(start, index);
                    if let Ok(code) = u32::from_str_radix(&digits, 16)
                        && let Some(c) = char::from_u32(code)
                    {
                        text.push(c);
                        self.pos = index.saturating_add(1);
                        return;
                    }
                }
                self.bump();
                text.push('u');
            }
            Some(c) => {
                self.bump();
                text.push(c);
            }
        }
    }

    /// Expand the `--`, `---` and `-?` shorthands, reporting whether one was written; a lone
    /// hyphen stays a literal character.
    fn dash(&mut self, text: &mut String) -> bool {
        if self.matches(self.pos, "---") {
            self.pos = self.pos.saturating_add(3);
            text.push('\u{2014}');
        } else if self.matches(self.pos, "--") {
            self.pos = self.pos.saturating_add(2);
            text.push('\u{2013}');
        } else if self.matches(self.pos, "-?") {
            self.pos = self.pos.saturating_add(2);
            text.push('\u{ad}');
        } else {
            self.bump();
            text.push('-');
            return false;
        }
        true
    }

    /// Whether the quote just consumed opens rather than closes a phrase.
    ///
    /// Whitespace before it opens; otherwise an apostrophe closes, while a double quote opens
    /// whenever running text continues straight after it. A shorthand written just before the
    /// quote sets it apart from the text it follows, so the quote begins a phrase of its own.
    fn quote_opens(&self, quote: char, text: &str, out: &[Inline], after_shorthand: bool) -> bool {
        if after_shorthand {
            return self.opens_phrase();
        }
        match preceding_kind(text, out) {
            Preceding::Space => true,
            Preceding::Other if quote == '\'' => false,
            Preceding::Other | Preceding::Nothing => self.opens_phrase(),
        }
    }

    /// Whether running text, rather than a gap or another element, continues at the cursor.
    fn opens_phrase(&self) -> bool {
        let Some(next) = self.peek() else {
            return false;
        };
        if next.is_whitespace() {
            return false;
        }
        match next {
            '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '"' | '\'' | '*' | '_' | '$' | '`'
            | '~' | '@' | '#' => false,
            '-' => !matches!(self.peek_at(1), Some('-' | '?')),
            '<' => self.label_end(self.pos).is_none(),
            _ => true,
        }
    }

    /// Whether a bare URL starts at the cursor, at a position where one may begin.
    fn at_url_start(&self, text: &str, out: &[Inline]) -> bool {
        let preceded = match text.chars().last() {
            Some(c) => c.is_alphanumeric(),
            None => matches!(out.last(), Some(Inline::Str(_))),
        };
        !preceded && (self.matches(self.pos, "http://") || self.matches(self.pos, "https://"))
    }

    /// Read a bare URL as a link, trimming the sentence punctuation that trails it.
    fn autolink(&mut self) -> Inline {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_whitespace() || matches!(c, '<' | '>' | '"' | '`' | '[' | ']' | '$' | '#') {
                break;
            }
            self.bump();
        }
        let mut end = self.pos;
        while let Some(c) = end
            .checked_sub(1)
            .and_then(|index| self.source.get(index).copied())
        {
            let unbalanced = c == ')' && !self.slice(start, end).contains('(');
            if !matches!(c, '.' | ',' | ';' | ':' | '!' | '?') && !unbalanced {
                break;
            }
            end = end.saturating_sub(1);
        }
        self.pos = end;
        let url = self.slice(start, end);
        Inline::Link(
            Box::default(),
            vec![Inline::Str(url.as_str().into())],
            Box::new(Target {
                url: url.into(),
                title: Text::default(),
            }),
        )
    }

    fn skip_line_comment(&mut self) {
        while !matches!(self.peek(), None | Some('\n')) {
            self.bump();
        }
    }

    fn skip_block_comment(&mut self) {
        self.pos = self.pos.saturating_add(2);
        let mut nesting = 1usize;
        while nesting > 0 && self.pos < self.limit {
            if self.matches(self.pos, "/*") {
                nesting = nesting.saturating_add(1);
                self.pos = self.pos.saturating_add(2);
            } else if self.matches(self.pos, "*/") {
                nesting = nesting.saturating_sub(1);
                self.pos = self.pos.saturating_add(2);
            } else {
                self.pos = self.pos.saturating_add(1);
            }
        }
    }

    // Code mode

    /// Find the offset of the delimiter closing the one at the cursor, skipping strings, raw spans,
    /// comments, and escapes.
    fn balanced(&self, open: char, close: char) -> Option<usize> {
        let mut index = self.pos.saturating_add(1);
        let mut nesting = 1usize;
        while index < self.limit {
            let Some(c) = self.at(index) else { break };
            match c {
                '\\' => index = index.saturating_add(1),
                '"' => {
                    index = index.saturating_add(1);
                    while let Some(inner) = self.at(index) {
                        if inner == '\\' {
                            index = index.saturating_add(1);
                        } else if inner == '"' {
                            break;
                        }
                        index = index.saturating_add(1);
                    }
                }
                '`' => {
                    let mut run = 0usize;
                    while self.at(index.saturating_add(run)) == Some('`') {
                        run = run.saturating_add(1);
                    }
                    let mut scan = index.saturating_add(run);
                    loop {
                        if scan >= self.limit {
                            index = scan;
                            break;
                        }
                        if self.at(scan) == Some('`') {
                            let mut closing = 0usize;
                            while self.at(scan.saturating_add(closing)) == Some('`') {
                                closing = closing.saturating_add(1);
                            }
                            if closing >= run {
                                index = scan.saturating_add(closing).saturating_sub(1);
                                break;
                            }
                            scan = scan.saturating_add(closing);
                            continue;
                        }
                        scan = scan.saturating_add(1);
                    }
                }
                '/' if self.matches(index, "//") => {
                    while !matches!(self.at(index), None | Some('\n')) {
                        index = index.saturating_add(1);
                    }
                    continue;
                }
                '/' if self.matches(index, "/*") => {
                    let mut nested = 1usize;
                    index = index.saturating_add(2);
                    while nested > 0 && index < self.limit {
                        if self.matches(index, "/*") {
                            nested = nested.saturating_add(1);
                            index = index.saturating_add(2);
                        } else if self.matches(index, "*/") {
                            nested = nested.saturating_sub(1);
                            index = index.saturating_add(2);
                        } else {
                            index = index.saturating_add(1);
                        }
                    }
                    continue;
                }
                _ if c == open => nesting = nesting.saturating_add(1),
                _ if c == close => {
                    nesting = nesting.saturating_sub(1);
                    if nesting == 0 {
                        return Some(index);
                    }
                }
                _ => {}
            }
            index = index.saturating_add(1);
        }
        None
    }

    /// Read a `#`-introduced code expression.
    fn code_expression(&mut self) -> Value {
        // A paragraph's lookahead evaluates the expression that ends it; reuse that result rather
        // than evaluate it again, which would cost exponential time on nested expressions.
        if let Some((start, value, end)) = self.evaluated.take()
            && start == self.pos
        {
            self.pos = end;
            return value;
        }
        self.bump();
        self.line_closed = None;
        // Past the nesting bound the `#` is all that is consumed, so what follows reads as markup.
        if self.depth >= MAX_DEPTH {
            return Value::Nothing;
        }
        self.depth = self.depth.saturating_add(1);
        let value = self.expression();
        self.depth = self.depth.saturating_sub(1);
        // Nothing outside the expression catches an exit, so it stops here rather than swallowing
        // the rest of the document.
        self.flow = None;
        value
    }

    /// Read a full operator expression that the line end closes, as a `#let` value does.
    fn line_expression(&mut self) -> Value {
        let bound = std::mem::replace(&mut self.line_bound, true);
        let value = self.argument_value();
        self.line_bound = bound;
        value
    }

    /// Read one code-mode expression, including any trailing content-block arguments.
    fn expression(&mut self) -> Value {
        let value = self.primary_expression();
        self.method_chain(value, None)
    }

    /// Apply the `.name` field reads and `.name(..)` method calls that follow an expression.
    ///
    /// `target` names the binding the value came from, if any, so a mutating method writes the
    /// changed value back where the rest of the document will see it.
    fn method_chain(&mut self, mut value: Value, target: Option<&str>) -> Value {
        while self.peek() == Some('.')
            && self
                .peek_at(1)
                .is_some_and(|c| c.is_alphabetic() || c == '_')
        {
            let saved = self.pos;
            self.bump();
            let name = self.read_identifier();
            if self.peek() == Some('(')
                && let Some(close) = self.balanced('(', ')')
            {
                let args = self.arguments(self.pos.saturating_add(1), close);
                self.pos = close.saturating_add(1);
                value = match mutating_method(&mut value, &name, &args) {
                    Some(result) => {
                        if let Some(target) = target {
                            self.env.insert(target.to_string(), value);
                        }
                        result
                    }
                    None => self.call_method(value, &name, &args),
                };
                continue;
            }
            let Some(field) = field_value(&value, &name) else {
                self.pos = saved;
                break;
            };
            value = field;
        }
        self.trailing_content(value)
    }

    /// Apply the content blocks written after an expression, which call the function it stands for.
    fn trailing_content(&mut self, mut value: Value) -> Value {
        while self.peek() == Some('[') {
            let callee = match &value {
                Value::Function(callee, _) => callee.clone(),
                Value::Ident(name) => Callee::Named(name.clone()),
                _ => break,
            };
            let fixed = match &value {
                Value::Function(_, fixed) => fixed.clone(),
                _ => Vec::new(),
            };
            let argument = Arg {
                name: None,
                value: Value::Content(self.content_block()),
            };
            value = self.invoke(&callee, &fixed, vec![argument]);
        }
        value
    }

    /// Apply a method that needs the parser: one that calls a function it was handed, or that fixes
    /// arguments to one. Everything else is a pure transformation of the receiver.
    fn call_method(&mut self, value: Value, name: &str, args: &[Arg]) -> Value {
        let function = args
            .iter()
            .find(|arg| arg.name.is_none())
            .map(|arg| &arg.value);
        match (name, &value) {
            ("with", Value::Function(callee, fixed)) => {
                let mut fixed = fixed.clone();
                fixed.extend(args.iter().cloned());
                Value::Function(callee.clone(), fixed)
            }
            ("map", Value::Array(items)) => Value::Array(
                items
                    .clone()
                    .into_iter()
                    .map(|item| self.apply(function.cloned(), vec![item]))
                    .collect(),
            ),
            ("filter", Value::Array(items)) => Value::Array(
                items
                    .clone()
                    .into_iter()
                    .filter(|item| {
                        self.apply(function.cloned(), vec![item.clone()])
                            .is_truthy()
                    })
                    .collect(),
            ),
            ("find", Value::Array(items)) => items
                .clone()
                .into_iter()
                .find(|item| {
                    self.apply(function.cloned(), vec![item.clone()])
                        .is_truthy()
                })
                .unwrap_or(Value::Nothing),
            ("any" | "all", Value::Array(items)) => {
                let decisive = name == "any";
                for item in items.clone() {
                    if self.apply(function.cloned(), vec![item]).is_truthy() == decisive {
                        return Value::Bool(decisive);
                    }
                }
                Value::Bool(!decisive)
            }
            ("position", Value::Array(items)) => {
                for (index, item) in items.clone().into_iter().enumerate() {
                    if self.apply(function.cloned(), vec![item]).is_truthy() {
                        return Value::Int(Integer::from(index));
                    }
                }
                Value::Nothing
            }
            ("fold", Value::Array(items)) => {
                let combine = args.iter().filter(|arg| arg.name.is_none()).nth(1);
                let mut total = function.cloned().unwrap_or(Value::Nothing);
                for item in items.clone() {
                    total = self.apply(combine.map(|arg| arg.value.clone()), vec![total, item]);
                }
                total
            }
            ("sorted", Value::Array(items)) => {
                let key = named(args, "key").cloned();
                let mut keyed: Vec<(Value, Value)> = items
                    .clone()
                    .into_iter()
                    .map(|item| match &key {
                        Some(key) => (self.apply(Some(key.clone()), vec![item.clone()]), item),
                        None => (item.clone(), item),
                    })
                    .collect();
                keyed.sort_by(|(left, _), (right, _)| order(left, right));
                Value::Array(keyed.into_iter().map(|(_, item)| item).collect())
            }
            _ => method_value(value, name, args),
        }
    }

    /// Call a value that is a function, or yield nothing when it is not one.
    fn apply(&mut self, function: Option<Value>, args: Vec<Value>) -> Value {
        let Some(Value::Function(callee, fixed)) = function else {
            return Value::Nothing;
        };
        let args = args
            .into_iter()
            .map(|value| Arg { name: None, value })
            .collect();
        self.invoke(&callee, &fixed, args)
    }

    fn primary_expression(&mut self) -> Value {
        let Some(c) = self.peek() else {
            return Value::Nothing;
        };
        match c {
            '(' => self.parenthesized(),
            '[' => Value::Content(self.content_block()),
            '"' => Value::Str(self.string_literal()),
            '<' => match self.label() {
                Some(name) => Value::Label(name),
                None => Value::Nothing,
            },
            '`' => self.raw_literal(),
            '0'..='9' | '.' => self.number_literal(),
            '-' => {
                self.bump();
                // Digits write a negative literal; anything else negates the value that follows.
                if matches!(self.peek(), Some('0'..='9' | '.')) {
                    return negate(self.number_literal());
                }
                let value = self.primary_expression();
                let value = self.method_chain(value, None);
                negate(value)
            }
            c if c.is_alphabetic() || c == '_' => self.identifier_expression(),
            '{' => self.code_block(),
            _ => Value::Nothing,
        }
    }

    /// Read a parenthesized group: a grouped expression, an array, or a dictionary.
    fn parenthesized(&mut self) -> Value {
        let Some(close) = self.balanced('(', ')') else {
            self.bump();
            return Value::Nothing;
        };
        let args = self.arguments(self.pos.saturating_add(1), close);
        let body = self.slice(self.pos.saturating_add(1), close);
        self.pos = close.saturating_add(1);
        if args.iter().any(|arg| arg.name.is_some()) || body.trim() == ":" {
            return Value::Dict(
                args.into_iter()
                    .filter_map(|arg| Some((arg.name?, arg.value)))
                    .collect(),
            );
        }
        let mut positional: Vec<Value> = args
            .into_iter()
            .filter(|arg| arg.name.is_none())
            .map(|arg| arg.value)
            .collect();
        // A trailing comma is what separates a one-item array from a parenthesized expression.
        let trailing_comma = self
            .source
            .get(..close)
            .and_then(|head| head.iter().rev().find(|c| !c.is_whitespace()))
            == Some(&',');
        match positional.len() {
            1 if !trailing_comma => positional.pop().unwrap_or(Value::Nothing),
            // Empty parentheses hold no expression to stand for, so they write the empty array.
            _ => Value::Array(positional),
        }
    }

    /// Read a raw literal standing where code expects a value: a fenced one sets a code block, a
    /// backtick-delimited one a code span.
    fn raw_literal(&mut self) -> Value {
        let fenced = self.matches(self.pos, "```");
        let (language, body) = self.raw_span();
        if fenced {
            return Value::Content(vec![code_block(&language, &body)]);
        }
        Value::Inlines(vec![Inline::Code(Box::default(), body.as_str().into())])
    }

    /// Read a `[..]` markup block as its own block sequence.
    fn content_block(&mut self) -> Vec<Block> {
        let Some(close) = self.balanced('[', ']') else {
            self.bump();
            return Vec::new();
        };
        let start = self.pos.saturating_add(1);
        let mut blocks = self.sub_blocks(start, close);
        self.pos = close.saturating_add(1);
        pad_content_edges(&mut blocks, &self.slice(start, close));
        blocks
    }

    fn string_literal(&mut self) -> String {
        self.bump();
        let mut out = String::new();
        while let Some(c) = self.bump() {
            match c {
                '"' => break,
                '\\' => match self.bump() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('r') => out.push('\r'),
                    Some('u') => self.string_codepoint(&mut out),
                    Some(other @ ('\\' | '"' | '\'')) => out.push(other),
                    // Only the escapes above stand for another character; the rest are literal.
                    Some(other) => {
                        out.push('\\');
                        out.push(other);
                    }
                    None => break,
                },
                other => out.push(other),
            }
        }
        out
    }

    /// Read the `{..}` hexadecimal codepoint of a `\u` escape, which stands for the character it
    /// numbers. Written any other way, the escape is literal.
    fn string_codepoint(&mut self, out: &mut String) {
        if self.peek() != Some('{') {
            out.push_str("\\u");
            return;
        }
        let start = self.pos.saturating_add(1);
        let mut end = start;
        while self.at(end).is_some_and(|c| c.is_ascii_hexdigit()) {
            end = end.saturating_add(1);
        }
        let character = u32::from_str_radix(&self.slice(start, end), 16)
            .ok()
            .and_then(char::from_u32);
        match character.filter(|_| self.at(end) == Some('}')) {
            Some(c) => {
                out.push(c);
                self.pos = end.saturating_add(1);
            }
            None => out.push_str("\\u"),
        }
    }

    fn number_literal(&mut self) -> Value {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
        }
        let mut float = false;
        // A dot belongs to the number unless a name follows it, which makes it a field access.
        if self.peek() == Some('.')
            && !self
                .peek_at(1)
                .is_some_and(|c| c.is_alphabetic() || c == '_')
        {
            float = true;
            self.bump();
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.bump();
            }
        }
        if self.exponent_ahead() {
            float = true;
            self.bump();
            if matches!(self.peek(), Some('+' | '-')) {
                self.bump();
            }
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.bump();
            }
        }
        let digits = self.slice(start, self.pos);
        let unit_start = self.pos;
        if self.peek() == Some('%') {
            self.bump();
        } else {
            while self.peek().is_some_and(char::is_alphabetic) {
                self.bump();
            }
        }
        let unit = self.slice(unit_start, self.pos);
        if unit.is_empty() && !float {
            return Value::Int(digits.parse::<Integer>().unwrap_or_default());
        }
        Value::Number(digits.parse::<f64>().unwrap_or(0.0), unit)
    }

    /// Whether an exponent marker starts here. `em` and other letter units also open with `e`, so
    /// the marker only counts when digits follow it.
    fn exponent_ahead(&self) -> bool {
        if !matches!(self.peek(), Some('e' | 'E')) {
            return false;
        }
        match self.peek_at(1) {
            Some('+' | '-') => self.peek_at(2).is_some_and(|c| c.is_ascii_digit()),
            Some(c) => c.is_ascii_digit(),
            None => false,
        }
    }

    /// Read an identifier expression: a keyword statement, a call, or a bound name.
    fn identifier_expression(&mut self) -> Value {
        let start = self.pos;
        let name = self.read_path();
        match name.as_str() {
            "none" => return Value::Nothing,
            "auto" => return Value::Ident("auto".to_string()),
            "true" => return Value::Bool(true),
            "false" => return Value::Bool(false),
            "let" => return self.let_statement(),
            "set" => return self.set_statement(),
            "include" => return self.include_statement(),
            "import" => return self.import_statement(),
            "show" => return self.show_statement(),
            "context" => return self.context_expression(),
            "return" => {
                self.skip_operand_space();
                let value = if self.ends_statement() {
                    Value::Nothing
                } else {
                    self.argument_value()
                };
                self.flow = Some(Flow::Return(value));
                return Value::Nothing;
            }
            "break" => {
                self.flow = Some(Flow::Break);
                return Value::Nothing;
            }
            "continue" => {
                self.flow = Some(Flow::Continue);
                return Value::Nothing;
            }
            "if" => return self.conditional(),
            "for" => return self.for_loop(),
            "while" => return self.while_loop(),
            _ => {}
        }
        // A bound name owns the dots after it: they read members of its value, not a longer path.
        if let Some((head, rest)) = name.split_once('.')
            && let Some(receiver) = self.receiver(head, rest)
        {
            self.pos = start.saturating_add(head.chars().count());
            return self.method_chain(receiver, Some(head));
        }
        let name = self.aliases.get(&name).cloned().unwrap_or(name);
        let mut args = Vec::new();
        let mut applied = false;
        if self.peek() == Some('(')
            && let Some(close) = self.balanced('(', ')')
        {
            args = self.arguments(self.pos.saturating_add(1), close);
            self.pos = close.saturating_add(1);
            applied = true;
        }
        while self.peek() == Some('[') {
            applied = true;
            args.push(Arg {
                name: None,
                value: Value::Content(self.content_block()),
            });
        }
        if !applied {
            if let Some(bound) = self.bound_copy(&name) {
                return bound;
            }
            if let Some(constant) = calc_constant(&name) {
                return Value::Number(constant, String::new());
            }
            // Only the symbol modules expose glyphs by name; a bare name is an ordinary value.
            if let Some(entry) = name
                .strip_prefix("sym.")
                .or_else(|| name.strip_prefix("math."))
                .and_then(symbol)
            {
                return Value::Inlines(vec![Inline::Str(entry.glyph.into())]);
            }
            if let Some(glyph) = name.strip_prefix("emoji.").and_then(emoji::glyph) {
                return Value::Inlines(vec![Inline::Str(glyph.into())]);
            }
            if let Some(value) = self
                .globs
                .iter()
                .find_map(|module| module_value(module, &name))
            {
                return value;
            }
            // Uncalled, an identifier is a plain value, so it can name a setting such as an alignment.
            return Value::Ident(name);
        }
        // A name bound to a function calls that function, so a binding can rename or specialize one.
        match self.env.get(&name).cloned() {
            Some(Value::Function(callee, fixed)) if !self.functions.contains_key(&name) => {
                return self.invoke(&callee, &fixed, args);
            }
            Some(Value::Ident(target)) if !self.functions.contains_key(&name) => {
                return self.named_call(&target, &args);
            }
            _ => {}
        }
        self.named_call(&name, &args)
    }

    /// A copy of what `name` binds, spending the copying allowance that copy takes. Yields nothing
    /// once the allowance is gone, so bindings that each splice the one before stay within what the
    /// source spelling them out can account for.
    fn bound_copy(&mut self, name: &str) -> Option<Value> {
        let bound = self.env.get(name)?;
        let Some(left) = self.copies.checked_sub(value_weight(bound)) else {
            self.copies = 0;
            return Some(Value::Nothing);
        };
        self.copies = left;
        Some(bound.clone())
    }

    /// The value a dotted path's head stands for when the dots read members of that value rather
    /// than continue a module path.
    fn receiver(&mut self, head: &str, rest: &str) -> Option<Value> {
        if let Some(bound) = self.bound_copy(head) {
            return Some(bound);
        }
        if let Some(bound) = self.functions.get(head) {
            return Some(Value::Function(Callee::Closure(bound.clone()), Vec::new()));
        }
        // Only a function takes `.with`, so a head nothing else binds names an element function.
        (rest == "with").then(|| Value::Function(Callee::Named(head.to_string()), Vec::new()))
    }

    /// Call the function a name stands for: a `#let` binding first, then an element function, then
    /// whatever a whole-module import supplies.
    fn named_call(&mut self, name: &str, args: &[Arg]) -> Value {
        if let Some(value) = self.user_call(name, args) {
            return value;
        }
        if name == "bibliography" {
            self.record_bibliography(args);
        }
        let value = self.call(name, args.to_vec());
        if matches!(value, Value::Nothing)
            && let Some(found) = self.glob_call(name, args)
        {
            return found;
        }
        value
    }

    /// Call a function value, with the arguments a partial application fixed coming first.
    fn invoke(&mut self, callee: &Callee, fixed: &[Arg], args: Vec<Arg>) -> Value {
        let mut all = fixed.to_vec();
        all.extend(args);
        match callee {
            Callee::Closure(function) => self.call_bound(function, &all),
            Callee::Named(name) => self.named_call(name, &all),
        }
    }

    /// The value a whole-module import gives a call nothing else resolves.
    fn glob_call(&mut self, name: &str, args: &[Arg]) -> Option<Value> {
        let modules = self.globs.clone();
        modules.iter().find_map(|module| {
            match self.call(&format!("{module}.{name}"), args.to_vec()) {
                Value::Nothing => None,
                value => Some(value),
            }
        })
    }

    /// Record the bibliography files a `#bibliography(..)` call names.
    fn record_bibliography(&mut self, args: &[Arg]) {
        let entry = match args
            .iter()
            .find(|arg| arg.name.is_none())
            .map(|arg| &arg.value)
        {
            Some(Value::Array(items)) => MetaValue::MetaList(
                items
                    .iter()
                    .map(|item| MetaValue::MetaString(item.as_text().as_str().into()))
                    .collect(),
            ),
            Some(other) => MetaValue::MetaString(other.as_text().as_str().into()),
            None => return,
        };
        self.meta.insert("bibliography".into(), entry);
    }

    /// Read one identifier, without the dotted segments that may follow it.
    fn read_identifier(&mut self) -> String {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            self.bump();
        }
        self.slice(start, self.pos)
    }

    /// Read a dotted identifier path (`table.header`, `sym.arrow.r`).
    fn read_path(&mut self) -> String {
        let start = self.pos;
        self.read_identifier();
        let mut end = self.pos;
        while self.at(end) == Some('.')
            && self
                .at(end.saturating_add(1))
                .is_some_and(char::is_alphabetic)
        {
            let mut scan = end.saturating_add(1);
            while self
                .at(scan)
                .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-')
            {
                scan = scan.saturating_add(1);
            }
            end = scan;
        }
        self.pos = end;
        self.slice(start, end)
    }

    /// Read `#let name = value`, binding the name for later interpolation.
    fn let_statement(&mut self) -> Value {
        self.skip_spaces();
        let name = self.read_path();
        self.skip_spaces();
        if self.peek() == Some('(') {
            self.function_definition(name);
        } else if self.eat('=') {
            self.skip_spaces();
            if !self.closure_definition(&name) {
                let value = self.line_expression();
                if !name.is_empty() {
                    self.declared.push(name.clone());
                    self.env.insert(name, value);
                }
            }
        }
        // A semicolon closes the binding rather than standing in the text after it.
        self.eat(';');
        Value::Nothing
    }

    /// Record a `#let name = (..) => ..` binding, whose body is evaluated once per call.
    /// Reports whether a closure is what the binding held.
    fn closure_definition(&mut self, name: &str) -> bool {
        let Some(signature) = self.closure_signature() else {
            return false;
        };
        if !name.is_empty() {
            self.functions.insert(name.to_string(), signature);
        }
        true
    }

    /// Read a `(..) => ..` closure, as its parameter names and the range holding its body. The
    /// cursor is left past the body, or where it started when no closure is written there.
    fn closure_signature(&mut self) -> Option<Function> {
        let parameters = self.closure_parameters()?;
        let body = self.pos;
        self.skip_body();
        Some(Function {
            parameters,
            body,
            limit: self.pos,
        })
    }

    /// Read a closure written as one argument, whose body ends where the argument does.
    fn closure_argument(&mut self) -> Option<Function> {
        let parameters = self.closure_parameters()?;
        let body = self.pos;
        self.skip_to_argument_end();
        Some(Function {
            parameters,
            body,
            limit: self.pos,
        })
    }

    /// Read a `(..) =>` closure head, leaving the cursor on its body. The cursor goes back where it
    /// started when no closure is written there.
    fn closure_parameters(&mut self) -> Option<Vec<Parameter>> {
        let saved = self.pos;
        let parameters = if self.peek() == Some('(') {
            let close = self.balanced('(', ')')?;
            let parameters = self.parameters(self.pos.saturating_add(1), close);
            self.pos = close.saturating_add(1);
            parameters
        } else {
            let single = self.read_identifier();
            if single.is_empty() {
                self.pos = saved;
                return None;
            }
            vec![Parameter {
                name: single,
                default: None,
                spread: false,
            }]
        };
        self.skip_spaces();
        if !(self.eat('=') && self.eat('>')) {
            self.pos = saved;
            return None;
        }
        self.skip_spaces();
        Some(parameters)
    }

    /// Step over the rest of the argument at the cursor, stopping on the comma that ends it.
    fn skip_to_argument_end(&mut self) {
        let mut depth = 0usize;
        while let Some(c) = self.peek() {
            match c {
                '"' => {
                    self.bump();
                    while let Some(c) = self.peek() {
                        self.bump();
                        if c == '\\' {
                            self.bump();
                        } else if c == '"' {
                            break;
                        }
                    }
                    continue;
                }
                '(' | '[' | '{' => depth = depth.saturating_add(1),
                ')' | ']' | '}' | ',' if depth == 0 => break,
                ')' | ']' | '}' => depth = depth.saturating_sub(1),
                _ => {}
            }
            self.bump();
        }
    }

    /// The parameters a signature declares, each with the default and spread marker it carries.
    fn parameters(&self, start: usize, end: usize) -> Vec<Parameter> {
        self.split_commas(start, end)
            .into_iter()
            .filter_map(|(from, to)| self.parameter(from, to))
            .collect()
    }

    /// Read one parameter out of the source range holding it.
    fn parameter(&self, start: usize, end: usize) -> Option<Parameter> {
        let mut from = start;
        while from < end && self.at(from).is_some_and(char::is_whitespace) {
            from = from.saturating_add(1);
        }
        let spread = self.matches(from, "..");
        if spread {
            from = from.saturating_add(2);
        }
        let mut index = from;
        while index < end
            && self
                .at(index)
                .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            index = index.saturating_add(1);
        }
        let name = self.slice(from, index);
        if name.is_empty() {
            return None;
        }
        let mut scan = index;
        while scan < end && self.at(scan).is_some_and(char::is_whitespace) {
            scan = scan.saturating_add(1);
        }
        let default = (self.at(scan) == Some(':')).then(|| (scan.saturating_add(1), end));
        Some(Parameter {
            name,
            default,
            spread,
        })
    }

    /// The source ranges of the comma-separated items written between `start` and `end`, split at
    /// the commas that sit outside every string and bracket pair.
    fn split_commas(&self, start: usize, end: usize) -> Vec<(usize, usize)> {
        let mut items = Vec::new();
        let mut item = start;
        let mut depth = 0usize;
        let mut quote = false;
        let mut index = start;
        while index < end {
            match self.at(index) {
                Some('\\') if quote => index = index.saturating_add(1),
                Some('"') => quote = !quote,
                Some('(' | '[' | '{') if !quote => depth = depth.saturating_add(1),
                Some(')' | ']' | '}') if !quote => depth = depth.saturating_sub(1),
                Some(',') if !quote && depth == 0 => {
                    items.push((item, index));
                    item = index.saturating_add(1);
                }
                _ => {}
            }
            index = index.saturating_add(1);
        }
        items.push((item, end));
        items.into_iter().filter(|(from, to)| from < to).collect()
    }

    /// Record a `#let name(..) = ..` binding, whose body is evaluated once per call.
    fn function_definition(&mut self, name: String) {
        let Some(close) = self.balanced('(', ')') else {
            self.skip_line_comment();
            return;
        };
        let parameters = self.parameters(self.pos.saturating_add(1), close);
        self.pos = close.saturating_add(1);
        self.skip_spaces();
        if !self.eat('=') {
            self.skip_line_comment();
            return;
        }
        self.skip_spaces();
        let body = self.pos;
        self.skip_body();
        if !name.is_empty() {
            self.functions.insert(
                name,
                Function {
                    parameters,
                    body,
                    limit: self.pos,
                },
            );
        }
    }

    /// Step over a definition body, which is a delimited block or the rest of the statement.
    fn skip_body(&mut self) {
        let delimiters = match self.peek() {
            Some('[') => (']', '['),
            Some('{') => ('}', '{'),
            _ => {
                self.skip_to_statement_end();
                return;
            }
        };
        match self.balanced(delimiters.1, delimiters.0) {
            Some(close) => self.pos = close.saturating_add(1),
            None => self.skip_line_comment(),
        }
    }

    /// Step over the rest of the statement at the cursor, which a semicolon, the line end, or the
    /// close of the group around it ends.
    fn skip_to_statement_end(&mut self) {
        let mut depth = 0usize;
        while let Some(c) = self.peek() {
            match c {
                '"' => {
                    self.bump();
                    while let Some(c) = self.peek() {
                        self.bump();
                        if c == '\\' {
                            self.bump();
                        } else if c == '"' {
                            break;
                        }
                    }
                    continue;
                }
                '\n' | ';' => break,
                '(' | '[' | '{' => depth = depth.saturating_add(1),
                ')' | ']' | '}' if depth == 0 => break,
                ')' | ']' | '}' => depth = depth.saturating_sub(1),
                _ => {}
            }
            self.bump();
        }
    }

    /// Read `#include "file"`, putting the named source's content in its place.
    fn include_statement(&mut self) -> Value {
        self.skip_spaces();
        let reference = self.expression().as_text();
        let Some(loaded) = self.open_source(&reference) else {
            return Value::Nothing;
        };
        let closes = self.source.get(loaded.end.saturating_sub(1)) == Some(&'\n');
        let blocks = self.file_blocks(&loaded);
        // The file's own final newline ends the line the statement sits on.
        if closes {
            self.line_closed = Some(self.pos);
        }
        Value::Content(blocks)
    }

    /// Read `#import`, binding what a file or a standard module exposes.
    fn import_statement(&mut self) -> Value {
        self.skip_spaces();
        let source = self.expression();
        self.skip_spaces();
        let mut list = if self.eat(':') {
            self.skip_spaces();
            if self.eat('*') {
                ImportList::All
            } else {
                ImportList::Named(self.import_names())
            }
        } else {
            ImportList::Module
        };
        if let ImportList::Module = list
            && self.eat_rename()
        {
            self.skip_spaces();
            let alias = self.read_path();
            list = ImportList::Named(vec![(String::new(), alias)]);
        }
        match source {
            Value::Str(reference) => self.import_file(&reference, &list),
            other => self.import_module(&other.as_text(), &list),
        }
        Value::Nothing
    }

    /// Read the comma-separated names of an import list, each with the name it takes locally.
    fn import_names(&mut self) -> Vec<(String, String)> {
        let mut names = Vec::new();
        loop {
            self.skip_spaces();
            let name = self.read_path();
            if name.is_empty() {
                break;
            }
            self.skip_spaces();
            let local = if self.eat_rename() {
                self.skip_spaces();
                self.read_path()
            } else {
                name.clone()
            };
            names.push((name, local));
            self.skip_spaces();
            if !self.eat(',') {
                break;
            }
        }
        names
    }

    /// Consume an `as` renaming keyword, which stands between two names on one line.
    fn eat_rename(&mut self) -> bool {
        if !self.matches(self.pos, "as")
            || !matches!(self.at(self.pos.saturating_add(2)), Some(' ' | '\t'))
        {
            return false;
        }
        self.pos = self.pos.saturating_add(2);
        self.skip_spaces();
        true
    }

    /// Evaluate a file for its bindings alone and adopt the ones the list asks for.
    fn import_file(&mut self, reference: &str, list: &ImportList) {
        let Some(loaded) = self.open_source(reference) else {
            return;
        };
        let outer_env = std::mem::take(&mut self.env);
        let outer_functions = std::mem::take(&mut self.functions);
        self.file_blocks(&loaded);
        let module_env = std::mem::replace(&mut self.env, outer_env);
        let module_functions = std::mem::replace(&mut self.functions, outer_functions);
        let stem = loaded
            .path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_string();
        for (name, value) in module_env {
            if let Some(local) = adopted_name(list, &name, &stem) {
                self.env.insert(local, value);
            }
        }
        for (name, function) in module_functions {
            if let Some(local) = adopted_name(list, &name, &stem) {
                self.functions.insert(local, function);
            }
        }
    }

    /// Record what an import of a standard module (`calc`, `sym`, …) makes reachable unqualified.
    fn import_module(&mut self, module: &str, list: &ImportList) {
        match list {
            // The module already answers to its own name, so its members need no alias.
            ImportList::Module => {}
            ImportList::All => self.globs.push(module.to_string()),
            ImportList::Named(names) => {
                for (name, local) in names {
                    self.aliases
                        .insert(local.clone(), format!("{module}.{name}"));
                }
            }
        }
    }

    /// Append a referenced file's characters to the arena. A file that cannot be read, or one
    /// already being parsed further up the chain, contributes nothing.
    fn open_source(&mut self, reference: &str) -> Option<Loaded> {
        if reference.is_empty() || self.depth >= MAX_DEPTH {
            return None;
        }
        let path = match self.base.as_deref() {
            Some(base) => base.join(reference),
            None => PathBuf::from(reference),
        };
        if !self.open.insert(path.clone()) {
            return None;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            self.open.remove(&path);
            return None;
        };
        let start = self.source.len();
        self.source.extend(normalize(&text));
        let end = self.source.len();
        let base = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf);
        Some(Loaded {
            start,
            end,
            base,
            path,
        })
    }

    /// Load the file a data call names, as the value that call's format produces.
    fn data_call(&mut self, name: &str, args: &[Arg]) -> Value {
        let reference = positional_text(args);
        if reference.is_empty() {
            return Value::Nothing;
        }
        let path = match self.base.as_deref() {
            Some(base) => base.join(&reference),
            None => PathBuf::from(&reference),
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Value::Nothing;
        };
        data::load(name, &text).unwrap_or(Value::Nothing)
    }

    /// Evaluate a string of source as code, as `#eval` does.
    fn eval_call(&mut self, args: &[Arg]) -> Value {
        let source = positional_text(args);
        if source.is_empty() || self.depth >= MAX_DEPTH {
            return Value::Nothing;
        }
        let start = self.source.len();
        self.source.extend(normalize(&source));
        let end = self.source.len();
        let (saved_pos, saved_limit) = (self.pos, self.limit);
        self.pos = start;
        self.limit = end;
        self.depth = self.depth.saturating_add(1);
        let value = self.argument_value();
        self.depth = self.depth.saturating_sub(1);
        self.pos = saved_pos;
        self.limit = saved_limit;
        value
    }

    /// Parse a loaded file's characters as their own region, with the references inside it
    /// resolving against that file's directory, and release it for a later reference.
    fn file_blocks(&mut self, loaded: &Loaded) -> Vec<Block> {
        let (saved_pos, saved_limit) = (self.pos, self.limit);
        let saved_base = std::mem::replace(&mut self.base, loaded.base.clone());
        let saved_evaluated = self.evaluated.take();
        self.pos = loaded.start;
        self.limit = loaded.end;
        self.depth = self.depth.saturating_add(1);
        let blocks = self.blocks(0);
        self.depth = self.depth.saturating_sub(1);
        self.evaluated = saved_evaluated;
        self.base = saved_base;
        self.pos = saved_pos;
        self.limit = saved_limit;
        self.open.remove(&loaded.path);
        blocks
    }

    /// Call a `#let`-defined function, binding its parameters around its body.
    fn user_call(&mut self, name: &str, args: &[Arg]) -> Option<Value> {
        let bound = self.functions.get(name).cloned()?;
        Some(self.call_bound(&bound, args))
    }

    /// Evaluate a function body with its parameters bound to the given arguments.
    fn call_bound(&mut self, bound: &Function, args: &[Arg]) -> Value {
        let Function {
            parameters,
            body,
            limit,
        } = bound.clone();
        if self.depth >= MAX_DEPTH {
            return Value::Nothing;
        }
        let outer: Vec<Option<Value>> = parameters
            .iter()
            .map(|parameter| self.env.get(&parameter.name).cloned())
            .collect();
        self.bind_arguments(&parameters, args);
        let (saved_pos, saved_limit) = (self.pos, self.limit);
        self.pos = body;
        self.limit = limit;
        self.depth = self.depth.saturating_add(1);
        let value = self.argument_value();
        self.depth = self.depth.saturating_sub(1);
        // A `return` names the call's result; `break` and `continue` reach no loop from here.
        let value = match self.flow.take() {
            Some(Flow::Return(returned)) => returned,
            _ => value,
        };
        self.pos = saved_pos;
        self.limit = saved_limit;
        for (parameter, previous) in parameters.into_iter().zip(outer) {
            match previous {
                Some(restored) => self.env.insert(parameter.name, restored),
                None => self.env.remove(&parameter.name),
            };
        }
        value
    }

    /// Bind a call's arguments to the parameters that take them: by name first, then in order, with
    /// a parameter no argument reaches falling back to its default.
    fn bind_arguments(&mut self, parameters: &[Parameter], args: &[Arg]) {
        let mut used = vec![false; args.len()];
        let mut next = 0usize;
        for parameter in parameters.iter().filter(|parameter| !parameter.spread) {
            let found = args
                .iter()
                .position(|arg| arg.name.as_deref() == Some(parameter.name.as_str()));
            let taken = found.or_else(|| {
                while let Some(arg) = args.get(next) {
                    let index = next;
                    next = next.saturating_add(1);
                    if arg.name.is_none() {
                        return Some(index);
                    }
                }
                None
            });
            if let Some(flag) = taken.and_then(|index| used.get_mut(index)) {
                *flag = true;
            }
            let value = match (taken.and_then(|index| args.get(index)), parameter.default) {
                (Some(arg), _) => arg.value.clone(),
                (None, Some((start, end))) => self.evaluate_range(start, end),
                (None, None) => Value::Nothing,
            };
            self.env.insert(parameter.name.clone(), value);
        }
        let Some(pack) = parameters.iter().find(|parameter| parameter.spread) else {
            return;
        };
        let mut rest = Vec::new();
        let mut named = Vec::new();
        for (index, arg) in args.iter().enumerate() {
            if used.get(index).copied().unwrap_or(false) {
                continue;
            }
            match arg.name.clone() {
                Some(name) => named.push((name, arg.value.clone())),
                None => rest.push(arg.value.clone()),
            }
        }
        self.env.insert(
            pack.name.clone(),
            Value::Dict(vec![
                ("pos".to_string(), Value::Array(rest)),
                ("named".to_string(), Value::Dict(named)),
            ]),
        );
    }

    /// Evaluate the expression written between `start` and `end`.
    fn evaluate_range(&mut self, start: usize, end: usize) -> Value {
        let (saved_pos, saved_limit) = (self.pos, self.limit);
        self.pos = start;
        self.limit = end.min(saved_limit);
        self.skip_argument_space();
        let value = self.argument_value();
        self.pos = saved_pos;
        self.limit = saved_limit;
        value
    }

    /// Read `#set element(..)`, keeping only the document metadata it can carry.
    fn set_statement(&mut self) -> Value {
        self.skip_spaces();
        let name = self.read_path();
        if self.peek() == Some('(')
            && let Some(close) = self.balanced('(', ')')
        {
            let args = self.arguments(self.pos.saturating_add(1), close);
            self.pos = close.saturating_add(1);
            match name.as_str() {
                "document" => self.document_metadata(&args),
                "enum" => self.enumeration = enumeration_from(&args, &self.enumeration),
                _ => {}
            }
        }
        Value::Nothing
    }

    /// Read `#context expr`, evaluated where it stands: none of the values this reader computes
    /// depend on the position in the document that asked for them.
    fn context_expression(&mut self) -> Value {
        self.skip_spaces();
        let mut value = self.expression();
        // An operator continues the expression only while it stays on the line that opened it.
        loop {
            let saved = self.pos;
            self.skip_spaces();
            let Some(operator) = self.peek().filter(|c| matches!(c, '+' | '-' | '*' | '/')) else {
                self.pos = saved;
                break;
            };
            self.bump();
            self.skip_spaces();
            let right = self.expression();
            value = combine(value, right, operator, &mut self.copies);
        }
        value
    }

    fn document_metadata(&mut self, args: &[Arg]) {
        for arg in args {
            match arg.name.as_deref() {
                Some("title") => {
                    let inlines = arg.value.clone().into_inlines();
                    if !inlines.is_empty() {
                        self.meta
                            .insert("title".into(), MetaValue::MetaInlines(inlines));
                    }
                }
                Some("date") => {
                    let inlines = arg.value.clone().into_inlines();
                    if !inlines.is_empty() {
                        self.meta
                            .insert("date".into(), MetaValue::MetaInlines(inlines));
                    }
                }
                Some(key @ ("author" | "keywords")) => {
                    let items = match &arg.value {
                        Value::Array(values) => values.clone(),
                        other => vec![other.clone()],
                    };
                    let list: Vec<MetaValue> = items
                        .into_iter()
                        .map(Value::into_inlines)
                        .filter(|inlines| !inlines.is_empty())
                        .map(MetaValue::MetaInlines)
                        .collect();
                    if !list.is_empty() {
                        self.meta.insert(key.into(), MetaValue::MetaList(list));
                    }
                }
                _ => {}
            }
        }
    }

    /// Advance to the body of a control-flow expression, stopping at the end of its line.
    fn skip_to_body(&mut self) {
        while let Some(c) = self.peek() {
            match c {
                '[' | '{' | '\n' => break,
                '(' => match self.balanced('(', ')') {
                    Some(close) => self.pos = close.saturating_add(1),
                    None => break,
                },
                _ => {
                    self.bump();
                }
            }
        }
    }

    /// Read `#if cond [..] else [..]`, keeping the branch the condition selects.
    fn conditional(&mut self) -> Value {
        let start = self.pos;
        self.skip_to_body();
        let condition = (start, self.pos);
        let Some(body) = self.take_body() else {
            return Value::Nothing;
        };
        let taken = self.truth(condition.0, condition.1);
        // Only the branch the condition selects runs, so the other one binds and sets nothing.
        let mut result = if taken {
            self.run_padded_body(body)
        } else {
            Value::Nothing
        };
        let saved = self.pos;
        self.skip_spaces();
        if !self.matches(self.pos, "else") {
            self.pos = saved;
            return result;
        }
        self.pos = self.pos.saturating_add(4);
        self.skip_spaces();
        let alternative = if self.matches(self.pos, "if") {
            self.pos = self.pos.saturating_add(2);
            if taken {
                self.skip_conditional();
                Value::Nothing
            } else {
                self.conditional()
            }
        } else {
            match self.take_body() {
                Some(body) if !taken => self.run_padded_body(body),
                _ => Value::Nothing,
            }
        };
        if !taken {
            result = alternative;
        }
        result
    }

    /// Step over an `if` chain without evaluating any of it.
    fn skip_conditional(&mut self) {
        self.skip_to_body();
        if self.take_body().is_none() {
            return;
        }
        let saved = self.pos;
        self.skip_spaces();
        if !self.matches(self.pos, "else") {
            self.pos = saved;
            return;
        }
        self.pos = self.pos.saturating_add(4);
        self.skip_spaces();
        if self.matches(self.pos, "if") {
            self.pos = self.pos.saturating_add(2);
            self.skip_conditional();
        } else {
            self.take_body();
        }
    }

    /// Take the `[..]` or `{..}` body at the cursor, leaving the cursor past it.
    fn take_body(&mut self) -> Option<Body> {
        let markup = match self.peek() {
            Some('[') => true,
            Some('{') => false,
            _ => return None,
        };
        let (open, close) = if markup { ('[', ']') } else { ('{', '}') };
        let end = self.balanced(open, close)?;
        let start = self.pos.saturating_add(1);
        self.pos = end.saturating_add(1);
        Some(if markup {
            Body::Markup(start, end)
        } else {
            Body::Code(start, end)
        })
    }

    /// Evaluate a control-flow body once.
    fn run_body(&mut self, body: Body) -> Value {
        match body {
            Body::Markup(start, end) => Value::Content(self.sub_blocks(start, end)),
            Body::Code(start, end) => self.code_range(start, end),
        }
    }

    /// Evaluate a control-flow body once, keeping the separators a markup body has at its edges.
    fn run_padded_body(&mut self, body: Body) -> Value {
        match (self.run_body(body), body) {
            (Value::Content(mut blocks), Body::Markup(start, end)) => {
                pad_content_edges(&mut blocks, &self.slice(start, end));
                Value::Content(blocks)
            }
            (value, _) => value,
        }
    }

    /// Evaluate the condition written between `start` and `end`.
    fn truth(&mut self, start: usize, end: usize) -> bool {
        self.evaluate_range(start, end).is_truthy()
    }

    /// Read the comparison operator between two condition operands.
    fn comparison_operator(&mut self) -> Option<String> {
        let saved = self.pos;
        if self.eat_keyword("not") {
            if self.eat_keyword("in") {
                return Some("not in".to_string());
            }
            self.pos = saved;
            return None;
        }
        for candidate in ["==", "!=", "<=", ">=", "<", ">", "in"] {
            if self.matches(self.pos, candidate) {
                self.pos = self.pos.saturating_add(candidate.len());
                return Some(candidate.to_string());
            }
        }
        None
    }

    /// Read `#for name in values [..]`, joining the body once per value.
    fn for_loop(&mut self) -> Value {
        self.skip_spaces();
        let names = self.loop_names();
        self.skip_spaces();
        if !self.matches(self.pos, "in") {
            self.skip_to_body();
            self.take_body();
            return Value::Nothing;
        }
        self.pos = self.pos.saturating_add(2);
        self.skip_argument_space();
        let values = self.argument_value();
        self.skip_to_body();
        let Some(body) = self.take_body() else {
            return Value::Nothing;
        };
        let mut joined = Value::Nothing;
        for item in iteration_values(&values).into_iter().take(MAX_ITERATIONS) {
            self.bind_pattern(&names, item);
            let round = self.run_padded_body(body);
            joined = join_values(joined, round);
            if self.leaves_loop() {
                break;
            }
        }
        joined
    }

    /// Settle the exit a loop round asked for, reporting whether the loop stops. A `return` is
    /// left standing so the enclosing call catches it.
    fn leaves_loop(&mut self) -> bool {
        match self.flow {
            Some(Flow::Break) => {
                self.flow = None;
                true
            }
            Some(Flow::Continue) => {
                self.flow = None;
                false
            }
            Some(Flow::Return(_)) => true,
            None => false,
        }
    }

    /// Read `#while cond [..]`, joining the body once per round the condition holds.
    fn while_loop(&mut self) -> Value {
        let start = self.pos;
        self.skip_to_body();
        let condition = (start, self.pos);
        let Some(body) = self.take_body() else {
            return Value::Nothing;
        };
        let mut joined = Value::Nothing;
        let mut rounds = 0usize;
        while rounds < MAX_ITERATIONS && self.truth(condition.0, condition.1) {
            rounds = rounds.saturating_add(1);
            let before = self.env.clone();
            let round = self.run_padded_body(body);
            joined = join_values(joined, round);
            if self.leaves_loop() {
                break;
            }
            // A round that moves no binding leaves the condition where it was, so it never ends.
            if self.env == before {
                break;
            }
        }
        joined
    }

    /// Read the name, or parenthesized names, a loop binds each value to.
    fn loop_names(&mut self) -> Vec<String> {
        if self.peek() != Some('(') {
            return vec![self.read_path()];
        }
        let Some(close) = self.balanced('(', ')') else {
            return Vec::new();
        };
        let inner = self.slice(self.pos.saturating_add(1), close);
        self.pos = close.saturating_add(1);
        inner
            .split(',')
            .map(|part| part.trim().to_string())
            .filter(|part| !part.is_empty())
            .collect()
    }

    /// Bind one iteration's value to the loop names, spreading an array over several of them.
    fn bind_pattern(&mut self, names: &[String], item: Value) {
        match (names, item) {
            ([single], value) => {
                self.env.insert(single.clone(), value);
            }
            (many, Value::Array(parts)) => {
                for (name, part) in many.iter().zip(parts) {
                    self.env.insert(name.clone(), part);
                }
            }
            (many, value) => {
                if let Some(first) = many.first() {
                    self.env.insert(first.clone(), value);
                }
            }
        }
    }

    /// Read `#{ .. }`: a sequence of statements whose values join into one.
    fn code_block(&mut self) -> Value {
        let Some(close) = self.balanced('{', '}') else {
            self.bump();
            return Value::Nothing;
        };
        let start = self.pos.saturating_add(1);
        self.pos = close.saturating_add(1);
        self.code_range(start, close)
    }

    /// Evaluate the statements written between `start` and `end`, joining their values into one.
    fn code_range(&mut self, start: usize, end: usize) -> Value {
        let (saved_pos, saved_limit) = (self.pos, self.limit);
        let bound = std::mem::take(&mut self.line_bound);
        let outer_scope = std::mem::take(&mut self.declared);
        let outer_env = self.env.clone();
        self.pos = start;
        self.limit = end.min(saved_limit);
        let mut joined = Value::Nothing;
        loop {
            self.skip_argument_space();
            while self.eat(';') {
                self.skip_argument_space();
            }
            if self.peek().is_none() {
                break;
            }
            let before = self.pos;
            let value = self.statement();
            joined = join_values(joined, value);
            if self.flow.is_some() {
                break;
            }
            if self.pos == before {
                self.bump();
            }
        }
        self.pos = saved_pos;
        self.limit = saved_limit;
        self.line_bound = bound;
        // A block opens a scope: its own bindings go away, while its assignments reach past it.
        for name in std::mem::replace(&mut self.declared, outer_scope) {
            match outer_env.get(&name) {
                Some(outer) => self.env.insert(name, outer.clone()),
                None => self.env.remove(&name),
            };
        }
        joined
    }

    /// Evaluate one statement: an assignment to a bound name, or an expression.
    fn statement(&mut self) -> Value {
        self.assignment().unwrap_or_else(|| self.argument_value())
    }

    /// Assign to a bound name with `=` or a compound operator, reporting nothing when no assignment
    /// is written at the cursor.
    fn assignment(&mut self) -> Option<Value> {
        let saved = self.pos;
        let name = self.read_path();
        if !self.env.contains_key(&name) {
            self.pos = saved;
            return None;
        }
        self.skip_argument_space();
        let operator = match self.peek() {
            Some('=') if self.peek_at(1) != Some('=') => None,
            Some(op @ ('+' | '-' | '*' | '/')) if self.peek_at(1) == Some('=') => {
                self.bump();
                Some(op)
            }
            _ => {
                self.pos = saved;
                return None;
            }
        };
        self.bump();
        self.skip_argument_space();
        let right = self.argument_value();
        let value = match operator {
            Some(op) => {
                let left = self.bound_copy(&name).unwrap_or(Value::Nothing);
                combine(left, right, op, &mut self.copies)
            }
            None => right,
        };
        self.env.insert(name, value);
        Some(Value::Nothing)
    }

    /// Split an argument list into named and positional arguments.
    fn arguments(&mut self, start: usize, end: usize) -> Vec<Arg> {
        let (saved_pos, saved_limit) = (self.pos, self.limit);
        let bound = std::mem::take(&mut self.line_bound);
        self.pos = start;
        self.limit = end.min(saved_limit);
        let mut out = Vec::new();
        loop {
            self.skip_argument_space();
            if self.peek().is_none() {
                break;
            }
            if self.matches(self.pos, "..") {
                self.pos = self.pos.saturating_add(2);
                out.append(&mut spread_arguments(self.argument_value()));
            } else {
                let name = self.argument_name();
                let value = self.argument_value();
                if !matches!(value, Value::Nothing) || name.is_some() {
                    out.push(Arg { name, value });
                }
            }
            self.skip_argument_space();
            if !self.eat(',') {
                break;
            }
        }
        self.pos = saved_pos;
        self.limit = saved_limit;
        self.line_bound = bound;
        out
    }

    fn skip_argument_space(&mut self) {
        loop {
            match self.peek() {
                Some('\n') if self.line_bound => break,
                Some(' ' | '\t' | '\n') => {
                    self.bump();
                }
                Some('/') if self.matches(self.pos, "//") => self.skip_line_comment(),
                Some('/') if self.matches(self.pos, "/*") => self.skip_block_comment(),
                _ => break,
            }
        }
    }

    /// Read a `name:` prefix, or `None` when the argument is positional.
    fn argument_name(&mut self) -> Option<String> {
        let start = self.pos;
        let mut index = self.pos;
        while self
            .at(index)
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            index = index.saturating_add(1);
        }
        if index == start || self.at(index) != Some(':') {
            return None;
        }
        let name = self.slice(start, index);
        self.pos = index.saturating_add(1);
        self.skip_argument_space();
        Some(name)
    }

    /// Read one argument value: an operator expression folded down to a single value.
    ///
    /// The operators bind in the usual order, loosest first: `or`, `and`, the comparisons, `+` and
    /// `-`, then `*` and `/`, with `not` tightest of all.
    fn argument_value(&mut self) -> Value {
        match self.closure_argument() {
            Some(closure) => Value::Function(Callee::Closure(closure), Vec::new()),
            None => self.disjunction(),
        }
    }

    fn disjunction(&mut self) -> Value {
        let mut value = self.conjunction();
        while self.eat_keyword("or") {
            let right = self.conjunction();
            value = Value::Bool(value.is_truthy() || right.is_truthy());
        }
        value
    }

    fn conjunction(&mut self) -> Value {
        let mut value = self.comparison();
        while self.eat_keyword("and") {
            let right = self.comparison();
            value = Value::Bool(value.is_truthy() && right.is_truthy());
        }
        value
    }

    fn comparison(&mut self) -> Value {
        let left = self.sum();
        let saved = self.pos;
        self.skip_argument_space();
        let Some(operator) = self.comparison_operator() else {
            self.pos = saved;
            return left;
        };
        self.skip_argument_space();
        let right = self.sum();
        Value::Bool(compare(&left, &right, &operator))
    }

    fn sum(&mut self) -> Value {
        let mut value = self.product();
        loop {
            let saved = self.pos;
            self.skip_argument_space();
            match self.peek() {
                Some('+') => {
                    self.bump();
                    self.skip_argument_space();
                    let right = self.product();
                    value = combine(value, right, '+', &mut self.copies);
                }
                Some('-') if value.as_number().is_some() => {
                    self.bump();
                    self.skip_argument_space();
                    let right = self.product();
                    value = combine(value, right, '-', &mut self.copies);
                }
                _ => {
                    self.pos = saved;
                    break;
                }
            }
        }
        value
    }

    fn product(&mut self) -> Value {
        let mut value = self.negation();
        loop {
            let saved = self.pos;
            self.skip_argument_space();
            match self.peek() {
                Some(op @ ('*' | '/')) if value.as_number().is_some() || repeatable(&value) => {
                    self.bump();
                    self.skip_argument_space();
                    let right = self.negation();
                    value = combine(value, right, op, &mut self.copies);
                }
                _ => {
                    self.pos = saved;
                    break;
                }
            }
        }
        value
    }

    fn negation(&mut self) -> Value {
        if self.eat_keyword("not") {
            return Value::Bool(!self.negation().is_truthy());
        }
        self.expression()
    }

    /// Consume `word` when it stands alone as an operator rather than opening a longer name.
    fn eat_keyword(&mut self, word: &str) -> bool {
        let saved = self.pos;
        self.skip_argument_space();
        let after = self.pos.saturating_add(word.len());
        if !self.matches(self.pos, word)
            || self
                .at(after)
                .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            self.pos = saved;
            return false;
        }
        self.pos = after;
        self.skip_argument_space();
        true
    }
}

/// The integers `range(..)` spans, from its start, end, and step arguments.
fn range_values(args: &[Arg]) -> Vec<Value> {
    let numbers: Vec<&Integer> = args
        .iter()
        .filter(|arg| arg.name.is_none())
        .filter_map(|arg| match &arg.value {
            Value::Int(n) => Some(n),
            _ => None,
        })
        .collect();
    let (start, end) = match numbers.as_slice() {
        [end] => (Integer::zero(), (*end).clone()),
        [start, end, ..] => ((*start).clone(), (*end).clone()),
        [] => return Vec::new(),
    };
    let step = match named(args, "step") {
        Some(Value::Int(n)) => n.clone(),
        _ => numbers.get(2).map_or_else(Integer::one, |n| (*n).clone()),
    };
    let ascending = !step.is_negative() && !step.is_zero();
    let mut out = Vec::new();
    let mut current = start;
    while out.len() < MAX_ITERATIONS
        && ((ascending && current < end) || (step.is_negative() && current > end))
    {
        let next = current.add(&step);
        out.push(Value::Int(current));
        current = next;
    }
    out
}

/// The values a `#for` loop walks over.
fn iteration_values(value: &Value) -> Vec<Value> {
    match value {
        Value::Array(items) => items.clone(),
        // A dictionary walks over its entries, each a key and its value.
        Value::Dict(pairs) => pairs
            .iter()
            .map(|(key, held)| Value::Array(vec![Value::Str(key.clone()), held.clone()]))
            .collect(),
        Value::Str(text) => text.chars().map(|c| Value::Str(c.to_string())).collect(),
        Value::Nothing => Vec::new(),
        other => vec![other.clone()],
    }
}

/// Join two values the way a sequence of them joins: one laid after the other as content.
fn join_values(left: Value, right: Value) -> Value {
    match (left, right) {
        (Value::Nothing, value) | (value, Value::Nothing) => value,
        (Value::Content(mut blocks), Value::Content(right)) => {
            append_content(&mut blocks, right);
            Value::Content(blocks)
        }
        (Value::Content(mut blocks), right) => {
            append_content(&mut blocks, right.into_blocks());
            Value::Content(blocks)
        }
        (left, Value::Content(right)) => {
            let mut blocks = left.into_blocks();
            append_content(&mut blocks, right);
            Value::Content(blocks)
        }
        (Value::Str(left), Value::Str(right)) => Value::Str(left + &right),
        (left, right) => {
            let mut inlines = left.into_inlines();
            inlines.extend(right.into_inlines());
            Value::Inlines(inlines)
        }
    }
}

/// Append content to content, continuing an open paragraph rather than starting a new one.
fn append_content(blocks: &mut Vec<Block>, next: Vec<Block>) {
    let mut next = next.into_iter();
    let Some(first) = next.next() else {
        return;
    };
    let closed = closes_paragraph(blocks);
    match (blocks.last_mut(), first) {
        (Some(Block::Para(tail)), Block::Para(mut head)) if !closed => tail.append(&mut head),
        (_, block) => blocks.extend(without_leading_edge(block)),
    }
    blocks.extend(next);
}

/// Drop the separator a block opens with, which separates nothing once the block starts a fresh
/// paragraph, along with the block itself when that is all it held.
fn without_leading_edge(mut block: Block) -> Option<Block> {
    if let Some(inlines) = first_edge_inlines(std::slice::from_mut(&mut block))
        && matches!(inlines.first(), Some(Inline::Space | Inline::SoftBreak))
    {
        inlines.remove(0);
        if inlines.is_empty() {
            return None;
        }
    }
    Some(block)
}

/// Whether content ends at a line break, which closes the paragraph it sits in. The break comes
/// off, having done its work.
fn closes_paragraph(blocks: &mut Vec<Block>) -> bool {
    let Some(Block::Para(tail) | Block::Plain(tail)) = blocks.last_mut() else {
        return false;
    };
    if !matches!(tail.last(), Some(Inline::SoftBreak)) {
        return false;
    }
    tail.pop();
    if tail.is_empty() {
        blocks.pop();
    }
    true
}

/// Compare two condition operands.
fn compare(left: &Value, right: &Value, operator: &str) -> bool {
    if let (Value::Int(a), Value::Int(b)) = (left, right) {
        return ordered(a.cmp(b), operator);
    }
    if let (Some(a), Some(b)) = (left.as_number(), right.as_number()) {
        return match operator {
            "==" => (a - b).abs() < f64::EPSILON,
            "!=" => (a - b).abs() >= f64::EPSILON,
            "<" => a < b,
            "<=" => a <= b,
            ">" => a > b,
            ">=" => a >= b,
            _ => false,
        };
    }
    if let "in" | "not in" = operator {
        let found = iteration_values(right)
            .iter()
            .any(|item| item.as_text() == left.as_text());
        return found == (operator == "in");
    }
    ordered(left.as_text().cmp(&right.as_text()), operator)
}

/// Whether an ordering satisfies a comparison operator.
fn ordered(ordering: Ordering, operator: &str) -> bool {
    match operator {
        "==" => ordering == Ordering::Equal,
        "!=" => ordering != Ordering::Equal,
        "<" => ordering == Ordering::Less,
        "<=" => ordering != Ordering::Greater,
        ">" => ordering == Ordering::Greater,
        ">=" => ordering != Ordering::Less,
        _ => false,
    }
}

/// Order two numeric values, exactly while both are whole.
fn numeric_order(left: &Value, right: &Value) -> Option<Ordering> {
    if let (Value::Int(a), Value::Int(b)) = (left, right) {
        return Some(a.cmp(b));
    }
    left.as_number()?.partial_cmp(&right.as_number()?)
}

/// Combine two operands of a code-mode arithmetic or concatenation operator.
fn combine(left: Value, right: Value, op: char, copies: &mut usize) -> Value {
    if let (Value::Int(a), Value::Int(b)) = (&left, &right) {
        let exact = match op {
            '-' => Some(a.subtract(b)),
            '*' => a.checked_multiply(b),
            // Only an even division stays whole; the float path takes the rest.
            '/' => a
                .divide(b)
                .filter(|(_, rest)| rest.is_zero())
                .map(|(quotient, _)| quotient),
            _ => Some(a.add(b)),
        };
        if let Some(value) = exact.filter(Integer::is_bounded) {
            return Value::Int(value);
        }
    }
    if let (Some(a), Some(b)) = (left.as_number(), right.as_number()) {
        let result = match op {
            '-' => a - b,
            '*' => a * b,
            '/' if b != 0.0 => a / b,
            _ => a + b,
        };
        return match (&left, &right) {
            (Value::Int(_), Value::Int(_)) if result.fract() == 0.0 => {
                Value::Int(Integer::from_f64(result))
            }
            (Value::Number(_, unit), _) | (_, Value::Number(_, unit)) if !unit.is_empty() => {
                Value::Number(result, unit.clone())
            }
            _ => Value::Number(result, String::new()),
        };
    }
    if op == '*'
        && let Some(count) = right.as_number()
    {
        return repeat_value(left, count, copies);
    }
    match (left, right) {
        (Value::Content(mut a), b) => {
            append_content(&mut a, b.into_blocks());
            Value::Content(a)
        }
        (a, Value::Content(b)) => {
            let mut blocks = a.into_blocks();
            append_content(&mut blocks, b);
            Value::Content(blocks)
        }
        (Value::Inlines(mut a), b) => {
            a.extend(b.into_inlines());
            Value::Inlines(a)
        }
        (a, Value::Inlines(b)) => {
            let mut inlines = a.into_inlines();
            inlines.extend(b);
            Value::Inlines(inlines)
        }
        (Value::Array(mut a), Value::Array(b)) => {
            a.extend(b);
            Value::Array(a)
        }
        (Value::Dict(mut a), Value::Dict(b)) => {
            for (key, value) in b {
                match a.iter_mut().find(|(held, _)| *held == key) {
                    Some(slot) => slot.1 = value,
                    None => a.push((key, value)),
                }
            }
            Value::Dict(a)
        }
        (a, b) => Value::Str(a.as_text() + &b.as_text()),
    }
}

/// Whether a value repeats under `*`, the operator's meaning for a sequence rather than a number.
fn repeatable(value: &Value) -> bool {
    matches!(
        value,
        Value::Str(_) | Value::Array(_) | Value::Content(_) | Value::Inlines(_)
    )
}

/// Repeat a sequence the given number of times.
fn repeat_value(value: Value, count: f64, copies: &mut usize) -> Value {
    let weight = value_weight(&value).max(1);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let count = (count.max(0.0).min(MAX_ITERATIONS as f64) as usize).min(*copies / weight);
    *copies = copies.saturating_sub(weight.saturating_mul(count));
    match value {
        Value::Str(text) => Value::Str(text.repeat(count)),
        Value::Array(items) => Value::Array(
            std::iter::repeat_n(items, count)
                .flatten()
                .collect::<Vec<_>>(),
        ),
        Value::Inlines(inlines) => Value::Inlines(
            std::iter::repeat_n(inlines, count)
                .flatten()
                .collect::<Vec<_>>(),
        ),
        Value::Content(blocks) => {
            let mut out = Vec::new();
            for _ in 0..count {
                append_content(&mut out, blocks.clone());
            }
            Value::Content(out)
        }
        other => other,
    }
}

/// Flush the pending text buffer into the inline sequence.
fn flush(text: &mut String, out: &mut Vec<Inline>) {
    if !text.is_empty() {
        out.push(Inline::Str(text.as_str().into()));
        text.clear();
    }
}

/// Drop a space left at the end of a line, where the newline carries the separation instead.
fn trim_trailing_space(out: &mut Vec<Inline>) {
    if matches!(out.last(), Some(Inline::Space | Inline::SoftBreak)) {
        out.pop();
    }
}

/// Drop the separators at both ends of an inline run, which no longer separate anything.
fn trim_edge_space(out: &mut Vec<Inline>) {
    if matches!(out.first(), Some(Inline::Space | Inline::SoftBreak)) {
        out.remove(0);
    }
    trim_trailing_space(out);
}

/// Fold neighbouring separators into one: repeated spaces read as a single gap, and a break
/// subsumes the spaces beside it.
fn collapse_separators(out: &mut Vec<Inline>) {
    let separator =
        |inline: Option<&Inline>| matches!(inline, Some(Inline::Space | Inline::SoftBreak));
    let mut index = 0usize;
    while index.saturating_add(1) < out.len() {
        if !separator(out.get(index)) || !separator(out.get(index.saturating_add(1))) {
            index = index.saturating_add(1);
            continue;
        }
        if matches!(out.get(index.saturating_add(1)), Some(Inline::SoftBreak)) {
            out.remove(index);
        } else {
            out.remove(index.saturating_add(1));
        }
    }
}

/// The separator a run of whitespace stands for: a break when it spans a line, a space otherwise.
fn edge_separator(whitespace: &str) -> Option<Inline> {
    if whitespace.is_empty() {
        None
    } else if whitespace.contains('\n') {
        Some(Inline::SoftBreak)
    } else {
        Some(Inline::Space)
    }
}

/// Restore the separators the whitespace at the ends of a content block stands for. Block parsing
/// drops them, yet they still separate the content from whatever surrounds the block inline.
fn pad_content_edges(blocks: &mut Vec<Block>, raw: &str) {
    let lead = raw
        .get(..raw.len() - raw.trim_start().len())
        .unwrap_or_default();
    let trail = raw.get(raw.trim_end().len()..).unwrap_or_default();
    // Whitespace with no content beside it is one run, so it stands for a single separator.
    if raw.trim().is_empty() {
        if let Some(separator) = edge_separator(raw) {
            blocks.push(Block::Plain(vec![separator]));
        }
        return;
    }
    // Only a lone paragraph flows into the text around it. Content that came out block-level stands
    // apart wherever it lands, and block parsing has already spent the whitespace at its edges.
    if blocks.len() != 1 {
        return;
    }
    if let Some(inlines) = first_edge_inlines(blocks) {
        if matches!(inlines.first(), Some(Inline::Space | Inline::SoftBreak)) {
            inlines.remove(0);
        }
        if let Some(separator) = edge_separator(lead) {
            inlines.insert(0, separator);
        }
    }
    if let Some(inlines) = last_edge_inlines(blocks) {
        trim_trailing_space(inlines);
        if let Some(separator) = edge_separator(trail) {
            inlines.push(separator);
        }
    }
}

/// The inline run the opening block carries, when it is one that carries one.
fn first_edge_inlines(blocks: &mut [Block]) -> Option<&mut Vec<Inline>> {
    match blocks.first_mut() {
        Some(Block::Para(inlines) | Block::Plain(inlines)) => Some(inlines),
        _ => None,
    }
}

/// The inline run the closing block carries, when it is one that carries one.
fn last_edge_inlines(blocks: &mut [Block]) -> Option<&mut Vec<Inline>> {
    match blocks.last_mut() {
        Some(Block::Para(inlines) | Block::Plain(inlines)) => Some(inlines),
        _ => None,
    }
}

/// What sits directly before a quote, as far as the choice of glyph goes.
enum Preceding {
    /// The start of the text the quote sits in.
    Nothing,
    /// Whitespace.
    Space,
    /// Any other character.
    Other,
}

/// Classify what a quote at the end of `text` within `out` follows.
fn preceding_kind(text: &str, out: &[Inline]) -> Preceding {
    match text.chars().last() {
        Some(c) if c.is_whitespace() || c == '\u{a0}' => Preceding::Space,
        Some(_) => Preceding::Other,
        // An element ends the text before it, so a quote after one begins fresh text.
        None => match out.last() {
            Some(Inline::Space | Inline::SoftBreak) => Preceding::Space,
            _ => Preceding::Nothing,
        },
    }
}

/// The curly form of a straight quote.
fn smart_quote(quote: char, opening: bool) -> char {
    match (quote, opening) {
        ('"', true) => '\u{201c}',
        ('"', false) => '\u{201d}',
        (_, true) => '\u{2018}',
        (_, false) => '\u{2019}',
    }
}

/// Build a citation carrying the key and the bracketed text it was written as.
fn citation(key: &str, mode: CitationMode) -> Inline {
    Inline::Cite(
        vec![Citation {
            id: key.into(),
            prefix: Vec::new(),
            suffix: Vec::new(),
            mode,
            note_num: 0,
            hash: 0,
        }],
        vec![Inline::Str(format!("[{key}]").as_str().into())],
    )
}

/// The citation mode a `#cite(.., form: ..)` selects: only the year-only form leaves the author out.
fn citation_mode(form: Option<&Value>) -> CitationMode {
    match form.map(Value::as_text).as_deref() {
        Some("year") => CitationMode::SuppressAuthor,
        _ => CitationMode::NormalCitation,
    }
}

/// A reference to `key`, provisionally an internal link. Whether the document defines that label is
/// only known once it has been read through, so [`resolve_references`] demotes the ones that miss.
fn reference(key: &str, body: Vec<Inline>) -> Inline {
    Inline::Link(
        Box::new(Attr {
            classes: vec!["ref".into()],
            ..Attr::default()
        }),
        body,
        Box::new(Target {
            url: format!("#{key}").as_str().into(),
            title: Text::default(),
        }),
    )
}

/// Remove up to `indent` leading columns from every line of a raw block.
/// A block of verbatim code, classed by the language its tag names.
fn code_block(language: &str, body: &str) -> Block {
    let mut attr = Attr::default();
    if !language.is_empty() {
        attr.classes.push(language.into());
    }
    Block::CodeBlock(Box::new(attr), body.into())
}

fn dedent(body: &str, indent: usize) -> String {
    if indent == 0 {
        return body.to_string();
    }
    body.split('\n')
        .map(|line| {
            let mut width = 0;
            let mut cut = 0;
            for c in line.chars() {
                if width >= indent {
                    break;
                }
                match c {
                    ' ' => width += 1,
                    '\t' => width += 2,
                    _ => break,
                }
                cut += c.len_utf8();
            }
            line.get(cut..).unwrap_or(line)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Which marker opens the items of a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListMarker {
    /// `- item`.
    Bullet,
    /// `+ item` or `2. item`.
    Enum,
    /// `/ Term: description`.
    Term,
}

// Element functions

impl Parser {
    /// Evaluate a call to a named element function.
    fn call(&mut self, name: &str, args: Vec<Arg>) -> Value {
        match name {
            "emph" => Value::Inlines(vec![Inline::Emph(first_inlines(&args))]),
            "strong" => Value::Inlines(vec![Inline::Strong(first_inlines(&args))]),
            "strike" => Value::Inlines(vec![Inline::Strikeout(first_inlines(&args))]),
            "underline" => Value::Inlines(vec![Inline::Underline(first_inlines(&args))]),
            "overline" | "highlight" => Value::Nothing,
            "smallcaps" => Value::Inlines(vec![Inline::SmallCaps(first_inlines(&args))]),
            "super" => Value::Inlines(vec![Inline::Superscript(first_inlines(&args))]),
            "sub" => Value::Inlines(vec![Inline::Subscript(first_inlines(&args))]),
            "upper" => Value::Inlines(map_text(first_inlines(&args), str::to_uppercase)),
            "lower" => Value::Inlines(map_text(first_inlines(&args), str::to_lowercase)),
            "text" => match body_blocks(&args) {
                Some(blocks) => Value::Content(blocks),
                None => text_call(&args),
            },
            "footnote" => Value::Inlines(vec![Inline::Note(first_blocks(&args))]),
            "cite" => Value::Inlines(vec![citation(
                &positional_text(&args),
                citation_mode(named(&args, "form")),
            )]),
            "ref" => Value::Inlines(vec![reference(
                &positional_text(&args),
                named(&args, "supplement")
                    .cloned()
                    .map(Value::into_inlines)
                    .unwrap_or_default(),
            )]),
            "link" => link_call(&args),
            "image" => Value::Inlines(vec![image(&args, self.base.as_deref())]),
            "raw" => raw_call(&args),
            "label" => Value::Label(positional_text(&args)),
            "regex" => Value::Regex(positional_text(&args)),
            "linebreak" => Value::Inlines(vec![Inline::LineBreak]),
            "datetime" => datetime(&args),
            "parbreak" | "colbreak" | "v" | "outline" | "counter" | "numbering" | "metadata"
            | "place" | "hide" | "repeat" | "pad" | "move" | "scale" => place_like(name, &args),
            "rotate" => wrap_body(
                &args,
                Attr {
                    classes: vec!["rotate".into()],
                    attributes: rotation_angle(&args),
                    ..Attr::default()
                },
                content_inlines(&args),
            ),
            "read" | "csv" | "json" | "toml" | "yaml" | "xml" => self.data_call(name, &args),
            "h" => Value::Inlines(vec![Inline::Str(horizontal_space(&args).as_str().into())]),
            "lorem" => Value::Inlines(text_inlines(&lorem(&args))),
            "box" => wrap_body(
                &args,
                Attr {
                    classes: vec!["box".into()],
                    ..Attr::default()
                },
                first_inlines(&args),
            ),
            "align" => wrap_body(
                &args,
                Attr {
                    attributes: vec![("align".into(), alignment_name(&args).as_str().into())],
                    ..Attr::default()
                },
                content_inlines(&args),
            ),
            _ => self.container_call(name, args),
        }
    }

    /// Evaluate a call to an element function that sets a block or a container.
    /// Replace an `align:` closure with the alignment it gives each column, which is what the
    /// closure returns for that column's index.
    fn expand_alignment(&mut self, mut args: Vec<Arg>) -> Vec<Arg> {
        let Some(Value::Function(callee, fixed)) = named(&args, "align").cloned() else {
            return args;
        };
        let count = named(&args, "columns")
            .map_or(0, |value| track_widths(value).len())
            .max(1);
        let alignments: Vec<Value> = (0..count)
            .map(|index| {
                let column = Value::Int(Integer::from(index));
                self.invoke(
                    &callee,
                    &fixed,
                    vec![
                        Arg {
                            name: None,
                            value: column,
                        },
                        Arg {
                            name: None,
                            value: Value::Int(Integer::zero()),
                        },
                    ],
                )
            })
            .collect();
        if let Some(arg) = args
            .iter_mut()
            .find(|arg| arg.name.as_deref() == Some("align"))
        {
            arg.value = Value::Array(alignments);
        }
        args
    }

    fn container_call(&mut self, name: &str, args: Vec<Arg>) -> Value {
        match name {
            "block" | "context" => Value::Content(first_blocks(&args)),
            "stack" => Value::Content(vec![Block::Div(
                Box::new(Attr {
                    attributes: vec![("stack".into(), stack_direction(&args).into())],
                    ..Attr::default()
                }),
                args.iter()
                    .filter(|arg| arg.name.is_none())
                    .map(|arg| Block::Div(Box::default(), arg.value.clone().into_blocks()))
                    .collect(),
            )]),
            "columns" => Value::Content(vec![Block::Div(
                Box::new(Attr {
                    classes: vec!["columns-flow".into()],
                    attributes: vec![("count".into(), column_count(&args).into())],
                    ..Attr::default()
                }),
                content_blocks(&args),
            )]),
            "rect" | "circle" | "ellipse" | "square" | "polygon" | "path" | "curve" => {
                Value::Content(vec![Block::Div(
                    Box::new(Attr {
                        classes: vec![name.into()],
                        ..Attr::default()
                    }),
                    first_blocks(&args),
                )])
            }
            "line" => Value::Content(vec![Block::HorizontalRule]),
            "pagebreak" => Value::Content(vec![Block::Div(
                Box::new(Attr {
                    classes: vec!["page-break".into()],
                    attributes: vec![("wrapper".into(), "1".into())],
                    ..Attr::default()
                }),
                vec![Block::HorizontalRule],
            )]),
            "heading" => Self::heading_call(&args),
            "quote" => Self::quote_call(&args),
            "list" => Value::Content(vec![Block::BulletList(item_bodies(&args))]),
            "enum" => Value::Content(vec![Block::OrderedList(
                enumeration_from(&args, &self.enumeration),
                item_bodies(&args),
            )]),
            "terms" => Value::Content(vec![Block::DefinitionList(term_entries(&args))]),
            "table" | "grid" => {
                let args = self.expand_alignment(args);
                Value::Content(vec![build_table(&args, Caption::default())])
            }
            "table.header" | "grid.header" => Value::Group(GroupKind::Header, args),
            "table.footer" | "grid.footer" => Value::Group(GroupKind::Footer, args),
            "table.cell" | "grid.cell" => Value::Group(GroupKind::Cell, args),
            "table.hline" | "table.vline" | "grid.hline" | "grid.vline" => {
                Value::Group(GroupKind::Rule, args)
            }
            "figure" => Value::Content(vec![figure(&args)]),
            "bibliography" => {
                let heading =
                    bibliography_title(&args).map(|title| Block::Header(1, Box::default(), title));
                Value::Content(
                    heading
                        .into_iter()
                        .chain([Block::Div(
                            Box::new(Attr {
                                id: "refs".into(),
                                ..Attr::default()
                            }),
                            Vec::new(),
                        )])
                        .collect(),
                )
            }
            "math" => Value::Inlines(vec![Inline::Math(MathType::InlineMath, Text::default())]),
            "str" => Value::Str(positional(&args).map(Value::as_text).unwrap_or_default()),
            "int" => positional(&args)
                .and_then(number_of)
                .map_or_else(|| Value::Int(Integer::zero()), whole),
            "float" => Value::Number(
                positional(&args).and_then(number_of).unwrap_or_default(),
                String::new(),
            ),
            "repr" => Value::Str(positional(&args).map_or_else(String::new, value_repr)),
            "type" => Value::Str(positional(&args).map_or("none", type_name).to_string()),
            "eval" => self.eval_call(&args),
            "range" => Value::Array(range_values(&args)),
            "par" | "layout" => Value::Nothing,
            _ => match name.strip_prefix("calc.") {
                Some(function) => calc_call(function, &args),
                None => Self::unknown_call(name, args),
            },
        }
    }

    /// A call this reader does not model: a symbol path resolves to its character, anything else
    /// degrades to the content it was given.
    fn unknown_call(name: &str, args: Vec<Arg>) -> Value {
        if let Some(rest) = name
            .strip_prefix("sym.")
            .or_else(|| name.strip_prefix("math."))
            && let Some(entry) = symbol(rest)
        {
            return Value::Inlines(vec![Inline::Str(entry.glyph.into())]);
        }
        if let Some(entry) = symbol(name) {
            return Value::Inlines(vec![Inline::Str(entry.glyph.into())]);
        }
        let content: Vec<Block> = args
            .into_iter()
            .filter(|arg| arg.name.is_none())
            .filter(|arg| matches!(arg.value, Value::Content(_)))
            .flat_map(|arg| arg.value.into_blocks())
            .collect();
        if content.is_empty() {
            Value::Nothing
        } else {
            Value::Content(content)
        }
    }

    fn heading_call(args: &[Arg]) -> Value {
        let level = named(args, "level")
            .and_then(Value::as_number)
            .unwrap_or(1.0);
        #[allow(clippy::cast_possible_truncation)]
        let level = level.max(1.0).min(f64::from(i32::MAX)) as i32;
        Value::Content(vec![Block::Header(
            i64::from(level),
            Box::default(),
            content_inlines(args),
        )])
    }

    fn quote_call(args: &[Arg]) -> Value {
        if !matches!(named(args, "block"), Some(Value::Bool(true))) {
            let mut body = content_inlines(args);
            trim_edge_space(&mut body);
            return Value::Inlines(vec![Inline::Quoted(QuoteType::DoubleQuote, body)]);
        }
        let mut blocks = content_blocks(args);
        let source = named(args, "attribution");
        let attributed = source.is_some_and(|value| {
            !matches!(value, Value::Nothing)
                && !matches!(value, Value::Content(blocks) if blocks.is_empty())
        });
        let attribution = source
            .map(|value| value.clone().into_inlines())
            .unwrap_or_default();
        if attributed {
            // Dash and gap form one text run, so they fuse with a name that starts with text.
            let mut line = vec![Inline::Str("\u{2014}\u{a0}".into())];
            line.extend(attribution);
            blocks.push(Block::Para(line));
        }
        Value::Content(vec![Block::BlockQuote(blocks)])
    }
}

/// `#place`-like layout primitives: keep any content they wrap, drop the placement itself.
fn place_like(name: &str, args: &[Arg]) -> Value {
    // These position or conceal what they wrap rather than setting it.
    if matches!(name, "hide" | "repeat" | "move" | "scale") {
        return Value::Nothing;
    }
    let blocks: Vec<Block> = args
        .iter()
        .filter(|arg| arg.name.is_none())
        .filter(|arg| matches!(arg.value, Value::Content(_) | Value::Inlines(_)))
        .flat_map(|arg| arg.value.clone().into_blocks())
        .collect();
    if blocks.is_empty() {
        return Value::Nothing;
    }
    Value::Content(blocks)
}

/// The rotation as a degree attribute, when the arguments give an angle.
fn rotation_angle(args: &[Arg]) -> Vec<(Text, Text)> {
    let angle = named(args, "angle").or_else(|| {
        args.iter()
            .find(|arg| arg.name.is_none())
            .map(|arg| &arg.value)
    });
    let Some(Value::Number(value, unit)) = angle else {
        return Vec::new();
    };
    let degrees = match unit.as_str() {
        "deg" => *value,
        "rad" => value.to_degrees(),
        _ => return Vec::new(),
    };
    let mut text = format!("{degrees}");
    if !text.contains(['.', 'e']) {
        text.push_str(".0");
    }
    vec![("angle".into(), text.as_str().into())]
}

/// The first positional argument as inline content.
fn first_inlines(args: &[Arg]) -> Vec<Inline> {
    args.iter()
        .find(|arg| arg.name.is_none())
        .map(|arg| arg.value.clone().into_inlines())
        .unwrap_or_default()
}

/// The first positional argument as block content.
fn first_blocks(args: &[Arg]) -> Vec<Block> {
    args.iter()
        .find(|arg| arg.name.is_none())
        .map(|arg| arg.value.clone().into_blocks())
        .unwrap_or_default()
}

/// Whether a positional argument stands as the content a call wraps rather than one of its
/// settings. A measure, a colour name, or a count configures the call; text and content are its
/// body, however the body was written.
fn is_body(value: &Value) -> bool {
    matches!(value, Value::Content(_) | Value::Inlines(_) | Value::Str(_))
}

/// The body a call like `#align(..)[..]` carries after its settings.
fn body_argument(args: &[Arg]) -> Option<&Value> {
    args.iter()
        .rev()
        .find(|arg| arg.name.is_none() && is_body(&arg.value))
        .map(|arg| &arg.value)
}

/// The body a call carries after its settings, as inline content.
fn content_inlines(args: &[Arg]) -> Vec<Inline> {
    body_argument(args)
        .map(|value| value.clone().into_inlines())
        .unwrap_or_default()
}

/// The blocks a content argument holds when what it holds is block-level: more than one block, or
/// one block that is not a paragraph. Content that stays inline reports `None`.
fn body_blocks(args: &[Arg]) -> Option<Vec<Block>> {
    let blocks = content_blocks(args);
    match blocks.as_slice() {
        [] | [Block::Para(_) | Block::Plain(_)] => None,
        _ => Some(blocks),
    }
}

/// Wrap a styled body in the attributes its call sets: a division when the body is block-level, a
/// span otherwise.
fn wrap_body(args: &[Arg], attr: Attr, inlines: Vec<Inline>) -> Value {
    match body_blocks(args) {
        Some(blocks) => Value::Content(vec![Block::Div(Box::new(attr), blocks)]),
        None => Value::Inlines(vec![Inline::Span(Box::new(attr), inlines)]),
    }
}

/// The body a call carries after its settings, as block content.
fn content_blocks(args: &[Arg]) -> Vec<Block> {
    body_argument(args)
        .map(|value| value.clone().into_blocks())
        .unwrap_or_default()
}

/// An array written out the way source code spells it, for an array that reached content position.
fn array_repr(items: &[Value]) -> String {
    let inner: Vec<String> = items.iter().map(value_repr).collect();
    format!("({})", inner.join(", "))
}

fn value_repr(value: &Value) -> String {
    match value {
        Value::Str(text) => format!("\"{text}\""),
        Value::Array(items) => array_repr(items),
        Value::Dict(pairs) => dict_repr(pairs),
        Value::Nothing => "none".to_string(),
        other => other.as_text(),
    }
}

/// Render a dictionary the way code mode writes one, its values in repr form.
fn dict_repr(pairs: &[(String, Value)]) -> String {
    let inner: Vec<String> = pairs
        .iter()
        .map(|(key, value)| format!("{key}: {}", value_repr(value)))
        .collect();
    format!("({})", inner.join(", "))
}

/// The name code mode gives a value's type.
fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Nothing => "none",
        Value::Bool(_) => "boolean",
        Value::Int(_) => "int",
        Value::Number(_, unit) => match unit.as_str() {
            "" => "float",
            "%" => "ratio",
            _ => "length",
        },
        Value::Str(_) | Value::Ident(_) => "string",
        Value::Label(_) => "label",
        Value::Array(_) => "array",
        Value::Dict(_) => "dictionary",
        Value::Content(_) | Value::Inlines(_) | Value::Group(..) => "content",
        Value::Function(..) => "function",
        Value::Regex(_) => "regex",
    }
}

/// Apply a method call to the value it was reached through, leaving unmodelled methods inert.
/// The value a dictionary holds under a name, for the `.name` reads that follow a value.
fn field_value(value: &Value, name: &str) -> Option<Value> {
    match value {
        Value::Dict(pairs) => pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, held)| held.clone()),
        Value::Str(text) if name == "text" => Some(Value::Str(text.clone())),
        Value::Inlines(inlines) => inline_field(inlines.as_slice(), name),
        Value::Content(blocks) => block_field(blocks.as_slice(), name),
        _ => None,
    }
}

/// The field an inline element carries under a name, for the element a show rule hands its
/// transform.
fn inline_field(inlines: &[Inline], name: &str) -> Option<Value> {
    if name == "text" {
        return Some(Value::Str(carta_ast::to_plain_text(inlines)));
    }
    let [inline] = inlines else { return None };
    match (name, inline) {
        ("body", _) => inline_children(inline).map(|children| Value::Inlines(children.clone())),
        ("dest" | "url", Inline::Link(_, _, target) | Inline::Image(_, _, target)) => {
            Some(Value::Str(target.url.to_string()))
        }
        ("lang", Inline::Code(attr, _)) => Some(Value::Str(attr.classes.first()?.to_string())),
        ("block", Inline::Code(..)) => Some(Value::Bool(false)),
        _ => None,
    }
}

/// The content an inline element wraps, for the elements that wrap content at all.
fn inline_children(inline: &Inline) -> Option<&Vec<Inline>> {
    match inline {
        Inline::Emph(children)
        | Inline::Strong(children)
        | Inline::Underline(children)
        | Inline::Strikeout(children)
        | Inline::Superscript(children)
        | Inline::Subscript(children)
        | Inline::SmallCaps(children)
        | Inline::Quoted(_, children)
        | Inline::Cite(_, children)
        | Inline::Link(_, children, _)
        | Inline::Image(_, children, _)
        | Inline::Span(_, children) => Some(children),
        _ => None,
    }
}

/// The field a block element carries under a name, for the element a show rule hands its transform.
fn block_field(blocks: &[Block], name: &str) -> Option<Value> {
    let [block] = blocks else { return None };
    match (name, block) {
        ("text", Block::CodeBlock(_, code)) => Some(Value::Str(code.to_string())),
        ("lang", Block::CodeBlock(attr, _)) => Some(Value::Str(attr.classes.first()?.to_string())),
        ("block", Block::CodeBlock(..)) => Some(Value::Bool(true)),
        ("text", _) => Some(Value::Str(carta_ast::to_plain_text(&blocks_to_inlines(
            blocks.to_vec(),
        )))),
        ("level" | "depth", Block::Header(level, ..)) => Some(Value::Int(Integer::from(*level))),
        ("body", Block::Header(_, _, inlines) | Block::Para(inlines) | Block::Plain(inlines)) => {
            Some(Value::Inlines(inlines.clone()))
        }
        ("body", Block::BlockQuote(children) | Block::Div(_, children)) => {
            Some(Value::Content(children.clone()))
        }
        ("caption", Block::Figure(_, caption, _)) => Some(Value::Content(caption.long.clone())),
        ("body", Block::Figure(_, _, children)) => Some(Value::Content(children.clone())),
        _ => None,
    }
}

/// The sum of an array's items, kept whole while every item is.
fn sum_values(items: &[Value]) -> Value {
    if let Some(whole) = whole_items(items) {
        return Value::Int(
            whole
                .iter()
                .fold(Integer::zero(), |total, item| total.add(item)),
        );
    }
    Value::Number(
        items.iter().filter_map(Value::as_number).sum(),
        String::new(),
    )
}

/// The items as whole numbers, or `None` once any of them is not one.
fn whole_items(items: &[Value]) -> Option<Vec<&Integer>> {
    items
        .iter()
        .map(|item| match item {
            Value::Int(n) => Some(n),
            _ => None,
        })
        .collect()
}

/// A number with its sign flipped; anything else stands as it is.
fn negate(value: Value) -> Value {
    match value {
        Value::Int(n) => Value::Int(n.negate()),
        Value::Number(n, unit) => Value::Number(-n, unit),
        other => other,
    }
}

/// The product of an array's items, kept whole while every item is.
fn product_values(items: &[Value]) -> Value {
    if let Some(product) = whole_items(items).and_then(|whole| {
        whole
            .iter()
            .try_fold(Integer::one(), |total, item| total.checked_multiply(item))
    }) {
        return Value::Int(product);
    }
    Value::Number(
        items.iter().filter_map(Value::as_number).product(),
        String::new(),
    )
}

/// The bounds a `slice` method selects, as an index range over a sequence of the given length.
fn slice_bounds(args: &[Arg], length: usize) -> (usize, usize) {
    let positional: Vec<&Value> = args
        .iter()
        .filter(|arg| arg.name.is_none())
        .map(|arg| &arg.value)
        .collect();
    let index = |value: Option<&&Value>, fallback: usize| match value.and_then(|v| v.as_number()) {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Some(number) if number >= 0.0 => (number as usize).min(length),
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Some(number) => length.saturating_sub(-number as usize),
        None => fallback,
    };
    let start = index(positional.first(), 0);
    let end = named(args, "count").map_or_else(
        || index(positional.get(1), length),
        |count| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let count = count.as_number().unwrap_or_default().max(0.0) as usize;
            start.saturating_add(count).min(length)
        },
    );
    (start, end.max(start))
}

/// What a string method searches for: literal text or a regular expression.
enum Pattern {
    /// A literal substring.
    Text(String),
    /// A compiled regular expression.
    Expression(Box<Regex>),
}

impl Pattern {
    /// The pattern an argument writes, or `None` where a regular expression does not compile.
    fn of(value: Option<&Value>) -> Option<Self> {
        match value? {
            Value::Regex(source) => Regex::new(source)
                .ok()
                .map(|regex| Pattern::Expression(Box::new(regex))),
            other => Some(Pattern::Text(other.as_text())),
        }
    }

    /// Where the pattern next matches at or after a byte offset, as a byte range.
    fn find(&self, text: &str, from: usize) -> Option<(usize, usize)> {
        let rest = text.get(from..)?;
        let (start, end) = match self {
            // An empty needle matches nowhere, so a scan over it always ends.
            Pattern::Text(needle) if needle.is_empty() => return None,
            Pattern::Text(needle) => {
                let start = rest.find(needle.as_str())?;
                (start, start.saturating_add(needle.len()))
            }
            Pattern::Expression(regex) => {
                let found = regex.find(rest).ok().flatten()?;
                (found.start(), found.end())
            }
        };
        Some((from.saturating_add(start), from.saturating_add(end)))
    }

    /// Every place the pattern matches, as byte ranges that do not overlap.
    fn find_all(&self, text: &str) -> Vec<(usize, usize)> {
        let mut spans = Vec::new();
        let mut from = 0usize;
        while let Some((start, end)) = self.find(text, from) {
            spans.push((start, end));
            from = if end > start {
                end
            } else {
                end.saturating_add(1)
            };
        }
        spans
    }

    /// The groups the pattern captured at a match, as the text of each.
    fn captures(&self, text: &str, start: usize) -> Vec<Value> {
        let (Pattern::Expression(regex), Some(rest)) = (self, text.get(start..)) else {
            return Vec::new();
        };
        let Ok(Some(found)) = regex.captures(rest) else {
            return Vec::new();
        };
        found
            .iter()
            .skip(1)
            .map(|group| match group {
                Some(group) => Value::Str(group.as_str().to_string()),
                None => Value::Nothing,
            })
            .collect()
    }
}

/// The number of characters before a byte offset, which is how a string method counts positions.
fn characters_before(text: &str, offset: usize) -> Integer {
    Integer::from(text.get(..offset).map_or(0, |head| head.chars().count()))
}

/// One match of a string pattern, as the dictionary the language describes it with.
fn match_entry(text: &str, span: (usize, usize), pattern: &Pattern) -> Value {
    let (start, end) = span;
    Value::Dict(vec![
        (
            "start".to_string(),
            Value::Int(characters_before(text, start)),
        ),
        ("end".to_string(), Value::Int(characters_before(text, end))),
        (
            "text".to_string(),
            Value::Str(text.get(start..end).unwrap_or_default().to_string()),
        ),
        (
            "captures".to_string(),
            Value::Array(pattern.captures(text, start)),
        ),
    ])
}

/// Replace the pattern's first `count` matches, or all of them when no count is given.
fn replace_matches(text: &str, pattern: &Pattern, with: &str, count: Option<usize>) -> String {
    let mut out = String::new();
    let mut cursor = 0usize;
    for (start, end) in pattern
        .find_all(text)
        .into_iter()
        .take(count.unwrap_or(usize::MAX))
    {
        out.push_str(text.get(cursor..start).unwrap_or_default());
        out.push_str(with);
        cursor = end;
    }
    out.push_str(text.get(cursor..).unwrap_or_default());
    out
}

/// Split text on every match of the pattern.
fn split_matches(text: &str, pattern: &Pattern) -> Vec<Value> {
    let mut parts = Vec::new();
    let mut cursor = 0usize;
    for (start, end) in pattern.find_all(text) {
        parts.push(Value::Str(
            text.get(cursor..start).unwrap_or_default().to_string(),
        ));
        cursor = end;
    }
    parts.push(Value::Str(
        text.get(cursor..).unwrap_or_default().to_string(),
    ));
    parts
}

/// Flatten nested arrays into one sequence of the values they hold.
fn flatten_values(items: &[Value], out: &mut Vec<Value>) {
    for item in items {
        match item {
            Value::Array(nested) => flatten_values(nested, out),
            other => out.push(other.clone()),
        }
    }
}

/// The order two values sort in: numeric where both are numbers, by text otherwise.
fn order(left: &Value, right: &Value) -> Ordering {
    match (left.as_number(), right.as_number()) {
        (Some(a), Some(b)) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
        _ => left.as_text().cmp(&right.as_text()),
    }
}

/// Apply a method that changes its receiver in place, yielding what the call itself stands for.
/// `None` where the method is not one of those, leaving the receiver untouched.
fn mutating_method(receiver: &mut Value, name: &str, args: &[Arg]) -> Option<Value> {
    let positional: Vec<Value> = args
        .iter()
        .filter(|arg| arg.name.is_none())
        .map(|arg| arg.value.clone())
        .collect();
    let first = || positional.first().cloned().unwrap_or(Value::Nothing);
    let index = |length: usize| {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        match positional.first().and_then(Value::as_number) {
            Some(number) if number >= 0.0 => (number as usize).min(length),
            Some(number) => length.saturating_sub(-number as usize),
            None => length,
        }
    };
    match (name, receiver) {
        ("push", Value::Array(items)) => {
            items.push(first());
            Some(Value::Nothing)
        }
        ("pop", Value::Array(items)) => Some(items.pop().unwrap_or(Value::Nothing)),
        ("insert", Value::Array(items)) => {
            let at = index(items.len());
            items.insert(at, positional.get(1).cloned().unwrap_or(Value::Nothing));
            Some(Value::Nothing)
        }
        ("remove", Value::Array(items)) => {
            let at = index(items.len());
            Some(if at < items.len() {
                items.remove(at)
            } else {
                Value::Nothing
            })
        }
        ("insert", Value::Dict(pairs)) => {
            let key = first().as_text();
            let held = positional.get(1).cloned().unwrap_or(Value::Nothing);
            match pairs.iter_mut().find(|(name, _)| *name == key) {
                Some(entry) => entry.1 = held,
                None => pairs.push((key, held)),
            }
            Some(Value::Nothing)
        }
        ("remove", Value::Dict(pairs)) => {
            let key = first().as_text();
            let found = pairs.iter().position(|(name, _)| *name == key);
            Some(match found {
                Some(at) => pairs.remove(at).1,
                None => named(args, "default").cloned().unwrap_or(Value::Nothing),
            })
        }
        _ => None,
    }
}

fn method_value(value: Value, name: &str, args: &[Arg]) -> Value {
    if let Some(result) = dict_method(&value, name, args) {
        return result;
    }
    if let Value::Array(items) = &value
        && let Some(result) = array_method(items, name, args)
    {
        return result;
    }
    if name == "display" {
        return match args.iter().find(|arg| arg.name.is_none()) {
            Some(format) => Value::Str(format_date(&value.as_text(), &format.value.as_text())),
            None => value,
        };
    }
    text_method(&value, name, args).unwrap_or(value)
}

/// The value a method reads out of a dictionary, or `None` when the receiver is not one or the
/// method is not a dictionary's.
fn dict_method(value: &Value, name: &str, args: &[Arg]) -> Option<Value> {
    let Value::Dict(pairs) = value else {
        return None;
    };
    Some(match name {
        "len" => Value::Int(Integer::from(pairs.len())),
        "at" => field_value(value, &positional_text(args))
            .or_else(|| named(args, "default").cloned())
            .unwrap_or(Value::Nothing),
        "pos" | "named" => field_value(value, name).unwrap_or(Value::Nothing),
        "keys" => Value::Array(
            pairs
                .iter()
                .map(|(key, _)| Value::Str(key.clone()))
                .collect(),
        ),
        "values" => Value::Array(pairs.iter().map(|(_, held)| held.clone()).collect()),
        "pairs" => Value::Array(
            pairs
                .iter()
                .map(|(key, held)| Value::Array(vec![Value::Str(key.clone()), held.clone()]))
                .collect(),
        ),
        _ => return None,
    })
}

/// The value an array method computes, or `None` when the method is not an array's.
fn array_method(items: &[Value], name: &str, args: &[Arg]) -> Option<Value> {
    Some(match name {
        "len" => Value::Int(Integer::from(items.len())),
        "sum" => sum_values(items),
        "product" => product_values(items),
        "rev" => Value::Array(items.iter().rev().cloned().collect()),
        "at" | "first" | "last" => {
            let index = match name {
                "first" => 0,
                "last" => items.len().saturating_sub(1),
                _ => positional_index(args),
            };
            items.get(index).cloned().unwrap_or(Value::Nothing)
        }
        "enumerate" => Value::Array(
            items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    Value::Array(vec![Value::Int(Integer::from(index)), item.clone()])
                })
                .collect(),
        ),
        "slice" => {
            let (start, end) = slice_bounds(args, items.len());
            Value::Array(items.get(start..end).unwrap_or_default().to_vec())
        }
        "join" => {
            let separator = positional(args).cloned().unwrap_or(Value::Nothing);
            let mut joined = Value::Nothing;
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    joined = join_values(joined, separator.clone());
                }
                joined = join_values(joined, item.clone());
            }
            joined
        }
        "zip" => {
            let others: Vec<Vec<Value>> = args
                .iter()
                .filter(|arg| arg.name.is_none())
                .map(|arg| iteration_values(&arg.value))
                .collect();
            let length = others.iter().map(Vec::len).fold(items.len(), usize::min);
            Value::Array(
                items
                    .iter()
                    .take(length)
                    .enumerate()
                    .map(|(index, item)| {
                        let mut row = vec![item.clone()];
                        row.extend(
                            others
                                .iter()
                                .map(|other| other.get(index).cloned().unwrap_or(Value::Nothing)),
                        );
                        Value::Array(row)
                    })
                    .collect(),
            )
        }
        "flatten" => {
            let mut flat = Vec::new();
            flatten_values(items, &mut flat);
            Value::Array(flat)
        }
        "contains" => {
            let wanted = positional_text(args);
            Value::Bool(items.iter().any(|item| item.as_text() == wanted))
        }
        "dedup" => {
            let mut unique: Vec<Value> = Vec::new();
            for item in items {
                if !unique.iter().any(|kept| kept.as_text() == item.as_text()) {
                    unique.push(item.clone());
                }
            }
            Value::Array(unique)
        }
        "windows" => {
            let size = positional_index(args);
            let count = items.len().saturating_add(1).saturating_sub(size);
            Value::Array(
                (0..count)
                    .map(|start| {
                        let end = start.saturating_add(size);
                        Value::Array(items.get(start..end).unwrap_or_default().to_vec())
                    })
                    .collect(),
            )
        }
        _ => return None,
    })
}

/// The value a text method computes over the receiver's text, or `None` when the method is not a
/// string's.
fn text_method(value: &Value, name: &str, args: &[Arg]) -> Option<Value> {
    let text = value.as_text();
    let pattern = || Pattern::of(positional(args));
    Some(match name {
        "len" => Value::Int(Integer::from(text.chars().count())),
        "trim" => Value::Str(text.trim().to_string()),
        "codepoints" | "clusters" => {
            Value::Array(text.chars().map(|c| Value::Str(c.to_string())).collect())
        }
        "slice" => {
            let characters: Vec<char> = text.chars().collect();
            let (start, end) = slice_bounds(args, characters.len());
            Value::Str(
                characters
                    .get(start..end)
                    .unwrap_or_default()
                    .iter()
                    .collect(),
            )
        }
        "contains" => {
            Value::Bool(pattern().is_some_and(|pattern| pattern.find(&text, 0).is_some()))
        }
        "split" => match pattern() {
            Some(pattern) => Value::Array(split_matches(&text, &pattern)),
            // Without a separator the text splits on runs of whitespace.
            None => Value::Array(
                text.split_whitespace()
                    .map(|part| Value::Str(part.to_string()))
                    .collect(),
            ),
        },
        "replace" => match pattern() {
            Some(pattern) => Value::Str(replace_matches(
                &text,
                &pattern,
                &replacement_text(args),
                replacement_count(args),
            )),
            None => Value::Str(text),
        },
        "starts-with" | "ends-with" => {
            let spans = pattern()
                .map(|pattern| pattern.find_all(&text))
                .unwrap_or_default();
            Value::Bool(match name {
                "starts-with" => spans.first().is_some_and(|(start, _)| *start == 0),
                _ => spans.iter().any(|(_, end)| *end == text.len()),
            })
        }
        "match" | "matches" => {
            let Some(pattern) = pattern() else {
                return Some(Value::Nothing);
            };
            let spans = pattern.find_all(&text);
            if name == "matches" {
                return Some(Value::Array(
                    spans
                        .into_iter()
                        .map(|span| match_entry(&text, span, &pattern))
                        .collect(),
                ));
            }
            spans
                .first()
                .map_or(Value::Nothing, |span| match_entry(&text, *span, &pattern))
        }
        "find" | "position" => {
            let found = pattern().and_then(|pattern| pattern.find(&text, 0));
            match found {
                Some((start, end)) if name == "find" => {
                    Value::Str(text.get(start..end).unwrap_or_default().to_string())
                }
                Some((start, _)) => Value::Int(characters_before(&text, start)),
                None => Value::Nothing,
            }
        }
        _ => return None,
    })
}

/// The text a `replace` puts in place of each match.
fn replacement_text(args: &[Arg]) -> String {
    args.iter()
        .filter(|arg| arg.name.is_none())
        .nth(1)
        .map(|arg| arg.value.as_text())
        .unwrap_or_default()
}

/// How many matches a `replace` is limited to, when it names a limit.
fn replacement_count(args: &[Arg]) -> Option<usize> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    named(args, "count")
        .and_then(Value::as_number)
        .map(|count| count.max(0.0) as usize)
}

/// The arguments a `..` spread stands for: an array spreads into positional arguments, a dictionary
/// into named ones, and anything else contributes a single positional argument.
fn spread_arguments(value: Value) -> Vec<Arg> {
    match value {
        Value::Array(items) => items
            .into_iter()
            .map(|value| Arg { name: None, value })
            .collect(),
        Value::Dict(pairs) => pairs
            .into_iter()
            .map(|(name, value)| Arg {
                name: Some(name),
                value,
            })
            .collect(),
        value => vec![Arg { name: None, value }],
    }
}

/// The first positional argument as a zero-based index.
fn positional_index(args: &[Arg]) -> usize {
    match args
        .iter()
        .find(|arg| arg.name.is_none())
        .map(|arg| &arg.value)
    {
        Some(Value::Int(n)) => n.to_usize().unwrap_or(0),
        _ => 0,
    }
}

/// Substitute the date components of an ISO date into a display pattern.
fn format_date(date: &str, pattern: &str) -> String {
    let mut parts = date.split('-');
    let year = parts.next().unwrap_or_default();
    let month = parts.next().unwrap_or_default();
    let day = parts.next().unwrap_or_default();
    pattern
        .replace("[year]", year)
        .replace("[month]", month)
        .replace("[day]", day)
}

/// The heading a bibliography section carries, which its `title` argument may override or, by
/// naming nothing, take away.
fn bibliography_title(args: &[Arg]) -> Option<Vec<Inline>> {
    let default = || vec![Inline::Str("References".into())];
    match named(args, "title") {
        Some(Value::Nothing) => None,
        Some(Value::Ident(name)) if name == "auto" => Some(default()),
        Some(title) => Some(title.clone().into_inlines()),
        None => Some(default()),
    }
}

/// The flow direction `#stack` lays its children out along.
fn stack_direction(args: &[Arg]) -> String {
    named(args, "dir").map_or_else(|| "ltr".to_string(), Value::as_text)
}

/// The column count `#columns` splits its body into.
fn column_count(args: &[Arg]) -> String {
    args.iter()
        .find(|arg| arg.name.is_none() && !matches!(arg.value, Value::Content(_)))
        .map_or_else(|| "2".to_string(), |arg| arg.value.as_text())
}

/// The first positional argument.
fn positional(args: &[Arg]) -> Option<&Value> {
    args.iter()
        .find(|arg| arg.name.is_none())
        .map(|arg| &arg.value)
}

/// The number a value states, reading one out of a string when that is how it was written.
fn number_of(value: &Value) -> Option<f64> {
    value
        .as_number()
        .or_else(|| value.as_text().trim().parse().ok())
}

/// The first positional argument as plain text.
fn positional_text(args: &[Arg]) -> String {
    args.iter()
        .find(|arg| arg.name.is_none())
        .map(|arg| arg.value.as_text())
        .unwrap_or_default()
}

/// The value of a named argument.
fn named<'a>(args: &'a [Arg], key: &str) -> Option<&'a Value> {
    args.iter()
        .find(|arg| arg.name.as_deref() == Some(key))
        .map(|arg| &arg.value)
}

/// Apply a text transform to every string in an inline sequence.
fn map_text(inlines: Vec<Inline>, transform: fn(&str) -> String) -> Vec<Inline> {
    inlines
        .into_iter()
        .map(|inline| match inline {
            Inline::Str(text) => Inline::Str(transform(&text).as_str().into()),
            other => other,
        })
        .collect()
}

/// `#text(..)`: only a bold weight changes the document model; other styling is presentational.
fn text_call(args: &[Arg]) -> Value {
    let body = content_inlines(args);
    let body = if body.is_empty() {
        text_inlines(&positional_text(args))
    } else {
        body
    };
    if named(args, "weight").is_some_and(|value| value.as_text() == "bold") {
        return Value::Inlines(vec![Inline::Strong(body)]);
    }
    Value::Inlines(body)
}

/// A date, and the time of day when the arguments carry one, in ISO 8601 order.
fn datetime(args: &[Arg]) -> Value {
    let field = |key: &str| named(args, key).and_then(Value::as_number);
    let (Some(year), Some(month), Some(day)) = (field("year"), field("month"), field("day")) else {
        return Value::Nothing;
    };
    #[allow(clippy::cast_possible_truncation)]
    let part = |value: f64| value.clamp(0.0, f64::from(i32::MAX)) as i64;
    let mut out = format!("{:04}-{:02}-{:02}", part(year), part(month), part(day));
    if let (Some(hour), Some(minute), Some(second)) =
        (field("hour"), field("minute"), field("second"))
    {
        let _ = write!(
            out,
            " {:02}:{:02}:{:02}",
            part(hour),
            part(minute),
            part(second)
        );
    }
    Value::Str(out)
}

/// The fixed-width spaces that stand in for a horizontal spacer.
fn horizontal_space(args: &[Arg]) -> String {
    /// A spacer of an unresolvable width sets a third of an em.
    const DEFAULT_EM: f64 = 1.0 / 3.0;
    /// The point size the em widths are relative to.
    const POINTS_PER_EM: f64 = 12.0;
    /// A ceiling on the repeated quads, so an extreme width cannot exhaust memory.
    const MAX_QUADS: u16 = 256;

    let width = match args
        .iter()
        .find(|arg| arg.name.is_none())
        .map(|arg| &arg.value)
    {
        Some(Value::Number(value, unit)) => match unit.as_str() {
            "em" => *value,
            "pt" => value / POINTS_PER_EM,
            "" | "fr" => 1.0,
            _ => DEFAULT_EM,
        },
        Some(Value::Int(_)) => 1.0,
        _ => DEFAULT_EM,
    };
    if width < 0.0 {
        return "\u{200b}".to_string();
    }
    let whole = width.floor();
    let mut fraction = width - whole;
    let mut out = String::new();
    while fraction > 0.5 {
        out.push('\u{2000}');
        fraction -= 0.5;
    }
    out.push(fraction_space(fraction));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let quads = whole.min(f64::from(MAX_QUADS)) as usize;
    for _ in 0..quads {
        out.push('\u{2001}');
    }
    out
}

/// The single space that sets a fraction of an em, in eighteenths.
fn fraction_space(fraction: f64) -> char {
    const STEPS: [(f64, char); 5] = [
        (2.0, '\u{200a}'),
        (3.0, '\u{2006}'),
        (4.0, '\u{a0}'),
        (5.0, '\u{2005}'),
        (7.0, '\u{2004}'),
    ];
    let eighteenths = fraction * 18.0;
    for (limit, space) in STEPS {
        if eighteenths <= limit {
            return space;
        }
    }
    '\u{2000}'
}

/// Read the term list entries, each written as a term and description pair.
fn term_entries(args: &[Arg]) -> Vec<(Vec<Inline>, Vec<Vec<Block>>)> {
    args.iter()
        .filter(|arg| arg.name.is_none())
        .filter_map(|arg| match &arg.value {
            Value::Array(items) if items.len() == 2 => {
                let term = items.first()?.clone().into_inlines();
                let description = items.get(1)?.clone().into_blocks();
                Some((term, vec![description]))
            }
            _ => None,
        })
        .collect()
}

/// Build raw text, as a code block when `block: true` asks for one.
fn raw_call(args: &[Arg]) -> Value {
    let body = positional_text(args);
    if matches!(named(args, "block"), Some(Value::Bool(true))) {
        let language = named(args, "lang").map(Value::as_text).unwrap_or_default();
        return Value::Content(vec![code_block(&language, &body)]);
    }
    Value::Inlines(vec![Inline::Code(Box::default(), body.as_str().into())])
}

fn link_call(args: &[Arg]) -> Value {
    // A label target names a place in this document, which the URL fragment syntax addresses.
    let url = match positional(args) {
        Some(Value::Label(name)) => format!("#{name}"),
        _ => positional_text(args),
    };
    // Past the destination sits the link text, which the destination itself stands in for when the
    // call writes none. An empty content block writes none; a body holding a rule writes one.
    let past_target: Vec<&Value> = args
        .iter()
        .filter(|arg| arg.name.is_none())
        .skip(1)
        .map(|arg| &arg.value)
        .collect();
    let written = past_target
        .into_iter()
        .rev()
        .find(|value| is_body(value))
        .filter(|value| !matches!(value, Value::Content(blocks) if blocks.is_empty()));
    let body = match written {
        Some(value) => value.clone().into_inlines(),
        None => vec![Inline::Str(url.as_str().into())],
    };
    Value::Inlines(vec![Inline::Link(
        Box::default(),
        body,
        Box::new(Target {
            url: url.as_str().into(),
            title: Text::default(),
        }),
    )])
}

/// Place a file reference under the directory its source was named in. A reference that names an
/// absolute location, and one made from a source with no directory, stands on its own.
fn under_base(base: Option<&Path>, reference: &str) -> String {
    let Some(base) = base.filter(|_| !reference.is_empty()) else {
        return reference.to_string();
    };
    base.join(reference)
        .to_str()
        .map_or_else(|| reference.to_string(), str::to_string)
}

fn image(args: &[Arg], base: Option<&Path>) -> Inline {
    let path = under_base(base, &positional_text(args));
    let alt = named(args, "alt")
        .map(|value| value.clone().into_inlines())
        .unwrap_or_default();
    let attributes = ["width", "height"]
        .into_iter()
        .filter_map(|key| Some((key.into(), sized_attribute(args, key)?)))
        .collect();
    Inline::Image(
        Box::new(Attr {
            attributes,
            ..Attr::default()
        }),
        alt,
        Box::new(Target {
            url: path.as_str().into(),
            title: Text::default(),
        }),
    )
}

/// The units a `width:`/`height:` setting may carry into a dimension attribute. A track fraction or
/// a bare number states no measurable size, so it yields no attribute.
const IMAGE_UNITS: &[&str] = &["pt", "mm", "cm", "in", "em"];

/// The dimension attribute a sizing argument contributes, if it states a measurable size.
fn sized_attribute(args: &[Arg], key: &str) -> Option<Text> {
    let Some(Value::Number(size, unit)) = named(args, key) else {
        return None;
    };
    if unit == "%" {
        return Some(format!("{}%", size.floor()).into());
    }
    IMAGE_UNITS
        .contains(&unit.as_str())
        .then(|| format!("{}{unit}", show_double(*size)).into())
}

/// Render a length in the decimal notation a dimension attribute carries: fixed-point when the
/// magnitude falls in `[10^-1, 10^7)`, scientific notation otherwise, always with a fractional part.
fn show_double(value: f64) -> String {
    if !value.is_finite() {
        return String::new();
    }
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent_text) = scientific
        .split_once('e')
        .unwrap_or((scientific.as_str(), "0"));
    let exponent: i64 = exponent_text.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let body = if (-1..=6).contains(&exponent) {
        fixed_point(&digits, exponent)
    } else {
        let (first, rest) = digits.split_at(1.min(digits.len()));
        let fraction = if rest.is_empty() { "0" } else { rest };
        format!("{first}.{fraction}e{exponent}")
    };
    if value.is_sign_negative() {
        format!("-{body}")
    } else {
        body
    }
}

/// Place the decimal point of `digits` so the value reads as `0.d…` scaled by `10^(exponent + 1)`.
fn fixed_point(digits: &str, exponent: i64) -> String {
    let point = exponent + 1;
    if point <= 0 {
        let zeros = usize::try_from(-point).unwrap_or(0);
        return format!("0.{}{digits}", "0".repeat(zeros));
    }
    let point = usize::try_from(point).unwrap_or(usize::MAX);
    if point >= digits.len() {
        format!("{digits}{}.0", "0".repeat(point - digits.len()))
    } else {
        let (whole, fraction) = digits.split_at(point);
        format!("{whole}.{fraction}")
    }
}

/// The `align:` setting rendered as the attribute value a span carries.
fn alignment_name(args: &[Arg]) -> String {
    args.iter()
        .find(|arg| arg.name.is_none() && !matches!(arg.value, Value::Content(_)))
        .map(|arg| arg.value.as_text())
        .unwrap_or_default()
}

/// The bodies of the items an explicit `#list(..)` / `#enum(..)` call carries.
fn item_bodies(args: &[Arg]) -> Vec<Vec<Block>> {
    args.iter()
        .filter(|arg| arg.name.is_none())
        .map(|arg| {
            let blocks = arg.value.clone().into_blocks();
            if blocks.is_empty() {
                vec![Block::Para(Vec::new())]
            } else {
                blocks
            }
        })
        .collect()
}

/// A canned filler text, cycled to the requested word count.
fn lorem(args: &[Arg]) -> String {
    let count = args
        .iter()
        .find(|arg| arg.name.is_none())
        .and_then(|arg| arg.value.as_number())
        .unwrap_or(0.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let count = count.clamp(0.0, 10_000.0) as usize;
    let words: Vec<&str> = LOREM.split(' ').collect();
    if words.is_empty() {
        return String::new();
    }
    (0..count)
        .filter_map(|index| words.get(index % words.len()).copied())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The filler paragraph `#lorem` draws from.
const LOREM: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod \
tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud \
exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in \
reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur. Excepteur sint \
occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum.";

/// Build a figure: a wrapped table takes the caption directly, anything else becomes a figure block.
fn figure(args: &[Arg]) -> Block {
    let mut caption_inlines = named(args, "caption")
        .map(|value| value.clone().into_inlines())
        .unwrap_or_default();
    trim_edge_space(&mut caption_inlines);
    let caption = Caption {
        short: None,
        long: if caption_inlines.is_empty() {
            Vec::new()
        } else {
            vec![Block::Para(caption_inlines)]
        },
    };
    let body = first_blocks(args);
    if let [Block::Table(table)] = body.as_slice() {
        let mut table = table.clone();
        table.caption = caption;
        return Block::Table(table);
    }
    Block::Figure(Box::default(), Box::new(caption), body)
}

/// Assemble a table from the column settings and the flat sequence of cells that follows them.
fn build_table(args: &[Arg], caption: Caption) -> Block {
    let columns = column_specs(args);
    let count = columns.len().max(1);
    let mut head_rows = Vec::new();
    let mut foot_rows = Vec::new();
    let mut body_rows = Vec::new();
    let mut pending: Vec<Cell> = Vec::new();
    for arg in args.iter().filter(|arg| arg.name.is_none()) {
        match &arg.value {
            Value::Group(GroupKind::Header, inner) => {
                body_rows.extend(lay_out_rows(std::mem::take(&mut pending), count));
                head_rows.extend(lay_out_rows(cells_from(inner), count));
            }
            Value::Group(GroupKind::Footer, inner) => {
                body_rows.extend(lay_out_rows(std::mem::take(&mut pending), count));
                foot_rows.extend(lay_out_rows(cells_from(inner), count));
            }
            Value::Group(GroupKind::Rule, _) => {}
            other => pending.push(cell_from(other)),
        }
    }
    body_rows.extend(lay_out_rows(pending, count));
    Block::Table(Box::new(Table {
        attr: Attr::default(),
        caption,
        col_specs: columns,
        head: TableHead {
            attr: Attr::default(),
            rows: head_rows,
        },
        bodies: vec![TableBody {
            attr: Attr::default(),
            row_head_columns: 0,
            head: Vec::new(),
            body: body_rows,
        }],
        foot: TableFoot {
            attr: Attr::default(),
            rows: foot_rows,
        },
    }))
}

/// The cells a `table.header(..)` / `table.footer(..)` grouping contributes.
fn cells_from(args: &[Arg]) -> Vec<Cell> {
    args.iter()
        .filter(|arg| arg.name.is_none())
        .map(|arg| cell_from(&arg.value))
        .collect()
}

/// An empty cell, standing where a row ran out of content before it ran out of columns.
fn blank_cell() -> Cell {
    Cell {
        attr: Attr::default(),
        align: Alignment::AlignDefault,
        row_span: 1,
        col_span: 1,
        content: Vec::new(),
    }
}

/// Close the row under construction, filling the columns it left free with empty cells and ageing
/// the row spans reaching into the next row.
fn close_row(
    rows: &mut Vec<Row>,
    current: &mut Vec<Cell>,
    covered: &mut [usize],
    column: &mut usize,
) {
    for slot in covered.iter().skip(*column) {
        if *slot == 0 {
            current.push(blank_cell());
        }
    }
    rows.push(Row {
        attr: Attr::default(),
        cells: std::mem::take(current),
    });
    for slot in covered.iter_mut() {
        *slot = slot.saturating_sub(1);
    }
    *column = 0;
}

/// Place a flat sequence of cells into rows of `count` columns.
///
/// Cells fill the grid left to right, skipping the slots a row span from above still covers and
/// wrapping to a fresh row when the next cell is too wide for the space left. A row short of
/// content is filled out with empty cells, and a row span reaching past the last row is trimmed to
/// the rows that exist.
fn lay_out_rows(cells: Vec<Cell>, count: usize) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    let mut current: Vec<Cell> = Vec::new();
    let mut covered = vec![0usize; count];
    let mut spans: Vec<(usize, usize, usize)> = Vec::new();
    let mut column = 0usize;

    for mut cell in cells {
        let width = usize::try_from(cell.col_span.max(1))
            .unwrap_or(usize::MAX)
            .clamp(1, count);
        let height = usize::try_from(cell.row_span.max(1)).unwrap_or(usize::MAX);
        loop {
            while covered.get(column).is_some_and(|slot| *slot > 0) {
                column += 1;
            }
            if column.saturating_add(width) <= count {
                break;
            }
            close_row(&mut rows, &mut current, &mut covered, &mut column);
        }
        cell.col_span = i64::try_from(width).unwrap_or(i64::MAX);
        spans.push((rows.len(), current.len(), height));
        current.push(cell);
        for slot in covered.iter_mut().skip(column).take(width) {
            *slot = height;
        }
        column += width;
        if column >= count {
            close_row(&mut rows, &mut current, &mut covered, &mut column);
        }
    }
    if !current.is_empty() {
        close_row(&mut rows, &mut current, &mut covered, &mut column);
    }

    let total = rows.len();
    for (row_index, cell_index, height) in spans {
        let Some(cell) = rows
            .get_mut(row_index)
            .and_then(|row| row.cells.get_mut(cell_index))
        else {
            continue;
        };
        cell.row_span =
            i64::try_from(height.min(total.saturating_sub(row_index)).max(1)).unwrap_or(i64::MAX);
    }
    rows
}

fn cell_from(value: &Value) -> Cell {
    if let Value::Group(GroupKind::Cell, inner) = value {
        let col_span = named(inner, "colspan")
            .and_then(Value::as_number)
            .map_or(1, |n| {
                #[allow(clippy::cast_possible_truncation)]
                let span = n as i64;
                span.max(1)
            });
        let row_span = named(inner, "rowspan")
            .and_then(Value::as_number)
            .map_or(1, |n| {
                #[allow(clippy::cast_possible_truncation)]
                let span = n as i64;
                span.max(1)
            });
        return Cell {
            attr: Attr::default(),
            align: named(inner, "align").map_or(Alignment::AlignDefault, alignment_of),
            row_span,
            col_span,
            content: first_blocks(inner),
        };
    }
    Cell {
        attr: Attr::default(),
        align: Alignment::AlignDefault,
        row_span: 1,
        col_span: 1,
        content: value.clone().into_blocks(),
    }
}

/// The per-column alignment and width a table's `columns:` and `align:` settings describe.
fn column_specs(args: &[Arg]) -> Vec<ColSpec> {
    let widths = named(args, "columns").map_or_else(Vec::new, track_widths);
    let count = if widths.is_empty() { 1 } else { widths.len() };
    let alignments = match named(args, "align") {
        Some(Value::Array(items)) => items.iter().map(alignment_of).collect(),
        Some(single) => vec![alignment_of(single); count],
        None => Vec::new(),
    };
    let shares = column_shares(&widths, count);
    let total: f64 = shares.iter().flatten().sum();
    (0..count)
        .map(|index| ColSpec {
            align: alignments
                .get(index)
                .cloned()
                .unwrap_or(Alignment::AlignDefault),
            width: match shares.get(index).copied().flatten() {
                Some(share) => ColWidth::ColWidth(share / total),
                None => ColWidth::ColWidthDefault,
            },
        })
        .collect()
}

/// The relative share of each of `count` columns, or `None` where no proportion can be given.
///
/// Only fractional tracks state a proportion; a track sized any other way stands in for the average
/// of them, so it takes an equal part of the space they divide. A table with no fractional track at
/// all leaves every column unsized.
fn column_shares(widths: &[Option<f64>], count: usize) -> Vec<Option<f64>> {
    let mut sum = 0.0;
    let mut fractional = 0.0_f64;
    for width in widths.iter().flatten() {
        sum += *width;
        fractional += 1.0;
    }
    let filler = (fractional > 0.0).then(|| sum / fractional);
    (0..count)
        .map(|index| widths.get(index).copied().flatten().or(filler))
        .collect()
}

/// The fractional share of each column, or `None` for a column sized by its content.
fn track_widths(value: &Value) -> Vec<Option<f64>> {
    match value {
        Value::Int(n) => vec![None; n.to_usize().unwrap_or_default().min(1024)],
        Value::Array(items) => items.iter().map(track_width).collect(),
        other => vec![track_width(other)],
    }
}

fn track_width(value: &Value) -> Option<f64> {
    match value {
        Value::Number(size, unit) if unit == "fr" => Some(*size),
        _ => None,
    }
}

fn alignment_of(value: &Value) -> Alignment {
    match value.as_text().as_str() {
        "left" | "start" => Alignment::AlignLeft,
        "right" | "end" => Alignment::AlignRight,
        "center" => Alignment::AlignCenter,
        _ => Alignment::AlignDefault,
    }
}

/// Join text nodes that ended up next to each other, so each run of text is one node.
fn merge_text_runs(inlines: &mut Vec<Inline>) {
    let mut out: Vec<Inline> = Vec::with_capacity(inlines.len());
    for inline in inlines.drain(..) {
        match (out.last_mut(), inline) {
            (Some(Inline::Str(previous)), Inline::Str(text)) => previous.push_str(&text),
            (_, other) => out.push(other),
        }
    }
    *inlines = out;
}

/// Fold a run of citations, which may be separated by single spaces, into one citation group.
fn merge_citations(inlines: &mut Vec<Inline>) {
    let mut out: Vec<Inline> = Vec::with_capacity(inlines.len());
    let mut held_space = false;
    for inline in inlines.drain(..) {
        let after_cite = matches!(out.last(), Some(Inline::Cite(..)));
        if matches!(inline, Inline::Space) && after_cite && !held_space {
            held_space = true;
            continue;
        }
        match (out.last_mut(), inline) {
            (Some(Inline::Cite(citations, body)), Inline::Cite(mut next, mut text)) => {
                citations.append(&mut next);
                body.append(&mut text);
            }
            (_, other) => {
                if held_space {
                    out.push(Inline::Space);
                }
                out.push(other);
            }
        }
        held_space = false;
    }
    if held_space {
        out.push(Inline::Space);
    }
    *inlines = out;
}

// References

/// Remove a leading empty label span, yielding the identifier it carried.
fn take_leading_label(inlines: &mut Vec<Inline>) -> Option<Text> {
    let Some(Inline::Span(attr, children)) = inlines.first() else {
        return None;
    };
    if !children.is_empty() || attr.id.is_empty() {
        return None;
    }
    let id = attr.id.clone();
    inlines.remove(0);
    if matches!(inlines.first(), Some(Inline::Space | Inline::SoftBreak)) {
        inlines.remove(0);
    }
    Some(id)
}

/// Demote every reference whose label this document never defines into a bibliography citation.
fn resolve_references(blocks: &mut [Block], mut labels: BTreeSet<Text>) {
    walk_inlines(blocks, &mut |inlines| {
        for inline in inlines.iter() {
            if let Inline::Span(attr, children) = inline
                && children.is_empty()
                && !attr.id.is_empty()
            {
                labels.insert(attr.id.clone());
            }
        }
    });
    walk_inlines(blocks, &mut |inlines| {
        for inline in inlines.iter_mut() {
            let Inline::Link(attr, _, target) = inline else {
                continue;
            };
            if !attr.classes.iter().any(|class| class == "ref") {
                continue;
            }
            let Some(key) = target.url.as_str().strip_prefix('#') else {
                continue;
            };
            if !labels.contains(key) {
                *inline = citation(key, CitationMode::NormalCitation);
            }
        }
    });
}

/// Apply a visitor to every inline sequence in the tree, innermost sequences last.
fn walk_inlines(blocks: &mut [Block], visit: &mut dyn FnMut(&mut Vec<Inline>)) {
    let apply = visit_inlines;
    for block in blocks {
        match block {
            Block::Para(inlines) | Block::Plain(inlines) | Block::Header(_, _, inlines) => {
                apply(inlines, visit);
            }
            Block::LineBlock(lines) => {
                for line in lines {
                    apply(line, visit);
                }
            }
            Block::BlockQuote(children) | Block::Div(_, children) => {
                walk_inlines(children, visit);
            }
            Block::Figure(_, caption, children) => {
                if let Some(short) = caption.short.as_mut() {
                    apply(short, visit);
                }
                walk_inlines(&mut caption.long, visit);
                walk_inlines(children, visit);
            }
            Block::BulletList(items) | Block::OrderedList(_, items) => {
                for item in items {
                    walk_inlines(item, visit);
                }
            }
            Block::DefinitionList(entries) => {
                for (term, definitions) in entries {
                    apply(term, visit);
                    for definition in definitions {
                        walk_inlines(definition, visit);
                    }
                }
            }
            Block::Table(table) => {
                if let Some(short) = table.caption.short.as_mut() {
                    apply(short, visit);
                }
                let mut long = std::mem::take(&mut table.caption.long);
                walk_inlines(&mut long, visit);
                table.caption.long = long;
                for row in table_rows(table) {
                    for cell in row {
                        walk_inlines(cell, visit);
                    }
                }
            }
            _ => {}
        }
    }
}

fn visit_inlines(inlines: &mut Vec<Inline>, visit: &mut dyn FnMut(&mut Vec<Inline>)) {
    visit(inlines);
    for inline in inlines.iter_mut() {
        match inline {
            Inline::Emph(children)
            | Inline::Underline(children)
            | Inline::Strong(children)
            | Inline::Strikeout(children)
            | Inline::Superscript(children)
            | Inline::Subscript(children)
            | Inline::SmallCaps(children)
            | Inline::Quoted(_, children)
            | Inline::Cite(_, children)
            | Inline::Link(_, children, _)
            | Inline::Image(_, children, _)
            | Inline::Span(_, children) => visit_inlines(children, visit),
            Inline::Note(blocks) => walk_inlines(blocks, visit),
            _ => {}
        }
    }
}

// East Asian line breaks

/// Drop the soft breaks that sit between two wide East Asian characters, where the source newline
/// carries no width of its own.
fn strip_wide_line_breaks(blocks: &mut [Block]) {
    for block in blocks {
        match block {
            Block::Para(inlines) | Block::Plain(inlines) | Block::Header(_, _, inlines) => {
                strip_wide_in_inlines(inlines);
            }
            Block::BlockQuote(children) | Block::Div(_, children) => {
                strip_wide_line_breaks(children);
            }
            Block::Figure(_, caption, children) => {
                strip_wide_in_caption(caption);
                strip_wide_line_breaks(children);
            }
            Block::BulletList(items) | Block::OrderedList(_, items) => {
                for item in items {
                    strip_wide_line_breaks(item);
                }
            }
            Block::DefinitionList(entries) => {
                for (term, definitions) in entries {
                    strip_wide_in_inlines(term);
                    for definition in definitions {
                        strip_wide_line_breaks(definition);
                    }
                }
            }
            Block::Table(table) => {
                strip_wide_in_caption(&mut table.caption);
                for row in table_rows(table) {
                    for cell in row {
                        strip_wide_line_breaks(cell);
                    }
                }
            }
            _ => {}
        }
    }
}

fn strip_wide_in_caption(caption: &mut Caption) {
    if let Some(short) = caption.short.as_mut() {
        strip_wide_in_inlines(short);
    }
    strip_wide_line_breaks(&mut caption.long);
}

/// Every cell body of a table, so a recursive pass can reach the inlines inside.
fn table_rows(table: &mut Table) -> Vec<Vec<&mut Vec<Block>>> {
    let mut out = Vec::new();
    let rows = table
        .head
        .rows
        .iter_mut()
        .chain(table.bodies.iter_mut().flat_map(|body| {
            let TableBody { head, body, .. } = body;
            head.iter_mut().chain(body.iter_mut())
        }))
        .chain(table.foot.rows.iter_mut());
    for row in rows {
        out.push(row.cells.iter_mut().map(|cell| &mut cell.content).collect());
    }
    out
}

fn strip_wide_in_inlines(inlines: &mut Vec<Inline>) {
    for inline in inlines.iter_mut() {
        match inline {
            Inline::Emph(children)
            | Inline::Underline(children)
            | Inline::Strong(children)
            | Inline::Strikeout(children)
            | Inline::Superscript(children)
            | Inline::Subscript(children)
            | Inline::SmallCaps(children)
            | Inline::Quoted(_, children)
            | Inline::Cite(_, children)
            | Inline::Link(_, children, _)
            | Inline::Image(_, children, _)
            | Inline::Span(_, children) => strip_wide_in_inlines(children),
            Inline::Note(blocks) => strip_wide_line_breaks(blocks),
            _ => {}
        }
    }
    let mut index = 0;
    while index < inlines.len() {
        if matches!(inlines.get(index), Some(Inline::SoftBreak)) {
            let before = index
                .checked_sub(1)
                .and_then(|previous| inlines.get(previous))
                .and_then(last_char)
                .is_some_and(is_east_asian_wide);
            let after = inlines
                .get(index.saturating_add(1))
                .and_then(first_char)
                .is_some_and(is_east_asian_wide);
            if before && after {
                inlines.remove(index);
                continue;
            }
        }
        index = index.saturating_add(1);
    }
}

fn last_char(inline: &Inline) -> Option<char> {
    match inline {
        Inline::Str(s) | Inline::Code(_, s) | Inline::Math(_, s) => s.chars().last(),
        Inline::Emph(children)
        | Inline::Underline(children)
        | Inline::Strong(children)
        | Inline::Strikeout(children)
        | Inline::Superscript(children)
        | Inline::Subscript(children)
        | Inline::SmallCaps(children)
        | Inline::Quoted(_, children)
        | Inline::Cite(_, children)
        | Inline::Link(_, children, _)
        | Inline::Span(_, children) => children.iter().rev().find_map(last_char),
        _ => None,
    }
}

fn first_char(inline: &Inline) -> Option<char> {
    match inline {
        Inline::Str(s) | Inline::Code(_, s) | Inline::Math(_, s) => s.chars().next(),
        Inline::Emph(children)
        | Inline::Underline(children)
        | Inline::Strong(children)
        | Inline::Strikeout(children)
        | Inline::Superscript(children)
        | Inline::Subscript(children)
        | Inline::SmallCaps(children)
        | Inline::Quoted(_, children)
        | Inline::Cite(_, children)
        | Inline::Link(_, children, _)
        | Inline::Span(_, children) => children.iter().find_map(first_char),
        _ => None,
    }
}

/// Whether a character occupies a wide cell in East Asian text (Unicode East Asian Width Wide or
/// Fullwidth). Halfwidth and Ambiguous-width characters are excluded.
/// Whether a character joins with its neighbours into a word, so an emphasis delimiter wedged
/// between two of them is plain text rather than a marker.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() && !is_unspaced_script(c)
}

/// Whether a character belongs to a script written without spaces between words.
///
/// Covers the ideographic, kana, bopomofo, Yi, and halfwidth or fullwidth blocks. Hangul is
/// excluded: Korean separates its words with spaces.
fn is_unspaced_script(c: char) -> bool {
    matches!(c as u32,
        0x3000..=0x318F
        | 0x31F0..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFFEF
        | 0x1B000..=0x1B16F
        | 0x1F200..=0x1F2FF
        | 0x20000..=0x3FFFD)
}

fn is_east_asian_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F
        | 0x2E80..=0x2EFF
        | 0x2F00..=0x2FDF
        | 0x2FF0..=0x2FFF
        | 0x3000..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE19
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1B000..=0x1B16F
        | 0x1F200..=0x1F2FF
        | 0x20000..=0x2FFFD
        | 0x30000..=0x3FFFD)
}

// Math mode

/// A rendered fragment of a math expression, tagged with how it joins its neighbours.
#[derive(Debug, Clone)]
enum Piece {
    /// Set adjacent to its neighbours.
    Atom(String),
    /// A parenthesized group: keeps its parentheses inline, sheds them as a script or fraction part.
    Group(String),
    /// A relation or binary operator, set off by a space on each side.
    Op(String),
    /// Punctuation, set adjacent like an atom but never used as a script base.
    Sep(String),
    /// An explicit line break.
    Break,
    /// An alignment point, which turns the expression into an array.
    Align,
}

impl Piece {
    /// The fragment as written in running math.
    fn text(&self) -> String {
        match self {
            Piece::Atom(text) | Piece::Op(text) | Piece::Sep(text) => text.clone(),
            Piece::Group(inner) => format!("({inner})"),
            Piece::Break => "\\\\".to_string(),
            Piece::Align => "&".to_string(),
        }
    }

    /// The fragment as the argument of a script, fraction, or accent.
    fn operand(&self) -> String {
        match self {
            Piece::Group(inner) => inner.clone(),
            other => other.text(),
        }
    }
}

/// Translate a Typst math body to the TeX the document model stores.
fn math_to_tex(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut math = Math {
        source: &chars,
        pos: 0,
        depth: 0,
    };
    let pieces = math.pieces(&[]);
    render_math(&pieces)
}

/// Join rendered fragments, spacing operators and giving an aligned body its own environment.
fn render_math(pieces: &[Piece]) -> String {
    if !pieces
        .iter()
        .any(|piece| matches!(piece, Piece::Align | Piece::Break))
    {
        return join_pieces(pieces);
    }
    let mut columns = 1usize;
    let mut row = 1usize;
    for piece in pieces {
        match piece {
            Piece::Align => {
                row = row.saturating_add(1);
                columns = columns.max(row);
            }
            Piece::Break => row = 1,
            _ => {}
        }
    }
    let body = join_pieces(pieces);
    // An even column count is exactly the right-left pairing an aligned environment provides.
    if columns.is_multiple_of(2) {
        return format!("\\begin{{aligned}}\n{body}\n\\end{{aligned}}");
    }
    let spec: String = (0..columns)
        .map(|index| if index % 2 == 0 { 'r' } else { 'l' })
        .collect();
    format!("\\begin{{array}}{{{spec}}}\n{body}\n\\end{{array}}")
}

/// Render one branch of a case distinction, where the first alignment point is its own column.
fn render_cases_row(pieces: &[Piece]) -> String {
    let Some(index) = pieces
        .iter()
        .position(|piece| matches!(piece, Piece::Align))
    else {
        return render_math(pieces);
    };
    let head = render_math(pieces.get(..index).unwrap_or_default());
    let tail = render_math(pieces.get(index.saturating_add(1)..).unwrap_or_default());
    format!("{head} & {tail}")
}

/// The TeX for a character that a backslash escaped in the source.
fn escaped_atom(escaped: char) -> String {
    match escaped {
        '\\' => "\\backslash".to_string(),
        '&' | '$' | '_' | '#' | '%' | '{' | '}' => format!("\\{escaped}"),
        other => other.to_string(),
    }
}

/// Whether `text` ends inside a TeX control sequence, so an alphanumeric set right behind it would
/// run into the command name.
fn ends_control_sequence(text: &str) -> bool {
    // Whitespace terminates a command name, so nothing after it can run into one.
    if text.ends_with(char::is_whitespace) {
        return false;
    }
    let head = text.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    if head.len() < text.len() {
        return head.ends_with('\\');
    }
    // A control symbol is a backslash and the one character after it.
    let mut chars = text.chars();
    chars.next_back();
    chars.as_str().ends_with('\\')
}

fn join_pieces(pieces: &[Piece]) -> String {
    let mut out = String::new();
    for piece in pieces {
        if !out.ends_with(' ')
            && ends_control_sequence(&out)
            && piece
                .text()
                .chars()
                .next()
                .is_some_and(char::is_alphanumeric)
        {
            out.push(' ');
        }
        match piece {
            Piece::Op(text) => {
                if !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
                out.push_str(text);
                out.push(' ');
            }
            Piece::Align => {
                if !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
                out.push_str("& ");
            }
            Piece::Break => {
                if !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
                out.push_str("\\\\\n");
            }
            other => out.push_str(&other.text()),
        }
    }
    out.trim_end().to_string()
}

/// The math-mode parser: a cursor over the body between the `$` delimiters.
struct Math<'a> {
    source: &'a [char],
    pos: usize,
    depth: usize,
}

impl Math<'_> {
    fn at(&self, index: usize) -> Option<char> {
        self.source.get(index).copied()
    }

    fn peek(&self) -> Option<char> {
        self.at(self.pos)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos = self.pos.saturating_add(1);
        }
        c
    }

    fn matches(&self, word: &str) -> bool {
        word.chars()
            .enumerate()
            .all(|(offset, c)| self.at(self.pos.saturating_add(offset)) == Some(c))
    }

    /// Step over the gaps between fragments: whitespace, and the comments that carry no math.
    fn skip_space(&mut self) {
        loop {
            match self.peek() {
                Some(' ' | '\t' | '\n') => self.pos = self.pos.saturating_add(1),
                Some('/') if self.matches("//") => {
                    while !matches!(self.peek(), None | Some('\n')) {
                        self.pos = self.pos.saturating_add(1);
                    }
                }
                Some('/') if self.matches("/*") => self.skip_block_comment(),
                _ => break,
            }
        }
    }

    /// Step over a `/* .. */` comment, which may hold further comments of its own.
    fn skip_block_comment(&mut self) {
        self.pos = self.pos.saturating_add(2);
        let mut nesting = 1usize;
        while nesting > 0 && self.peek().is_some() {
            if self.matches("/*") {
                nesting = nesting.saturating_add(1);
                self.pos = self.pos.saturating_add(2);
            } else if self.matches("*/") {
                nesting = nesting.saturating_sub(1);
                self.pos = self.pos.saturating_add(2);
            } else {
                self.pos = self.pos.saturating_add(1);
            }
        }
    }

    /// Read fragments until one of the stop characters is reached at this nesting level.
    fn pieces(&mut self, stop: &[char]) -> Vec<Piece> {
        let mut out: Vec<Piece> = Vec::new();
        if self.depth >= MAX_DEPTH {
            return out;
        }
        loop {
            self.skip_space();
            let Some(c) = self.peek() else { break };
            if stop.contains(&c) {
                break;
            }
            match c {
                '^' | '_' => {
                    let base = out.pop().unwrap_or(Piece::Atom(String::new()));
                    let attached = self.scripts(&base);
                    out.push(Piece::Atom(attached));
                }
                '/' => {
                    self.bump();
                    let numerator = out.pop().unwrap_or(Piece::Atom(String::new()));
                    self.skip_space();
                    let mut denominator = self.unit();
                    self.skip_space();
                    // Scripts bind tighter than the fraction, so they stay inside the denominator.
                    if matches!(self.peek(), Some('^' | '_')) {
                        denominator = Piece::Atom(self.scripts(&denominator));
                    }
                    out.push(Piece::Atom(format!(
                        "\\frac{{{}}}{{{}}}",
                        numerator.operand(),
                        denominator.operand()
                    )));
                }
                '&' => {
                    self.bump();
                    out.push(Piece::Align);
                }
                _ => out.push(self.unit()),
            }
        }
        out
    }

    /// Read the `_`/`^` scripts that follow a base, emitting the subscript before the superscript.
    ///
    /// A slot may be filled once per base; a repeat of either marker starts a fresh attachment
    /// whose base is everything read so far, braced.
    fn scripts(&mut self, base: &Piece) -> String {
        let mut out = base.text();
        if takes_limits(&out) {
            out.push_str("\\limits");
        }
        let mut attached = false;
        loop {
            let (subscript, superscript) = self.script_slots();
            if subscript.is_none() && superscript.is_none() {
                break;
            }
            if attached {
                out = format!("{{{out}}}");
            }
            if let Some(sub) = subscript {
                let _ = write!(out, "_{{{sub}}}");
            }
            if let Some(sup) = superscript {
                let _ = write!(out, "^{{{sup}}}");
            }
            attached = true;
        }
        out
    }

    /// Read at most one subscript and one superscript, stopping at a marker whose slot is taken.
    fn script_slots(&mut self) -> (Option<String>, Option<String>) {
        let mut subscript = None;
        let mut superscript = None;
        loop {
            match self.peek() {
                Some('_') if subscript.is_none() => {
                    self.bump();
                    subscript = Some(self.unit().operand());
                }
                Some('^') if superscript.is_none() => {
                    self.bump();
                    superscript = Some(self.unit().operand());
                }
                _ => break,
            }
        }
        (subscript, superscript)
    }

    /// Read one self-contained fragment: an atom, a group, a call, or an operator.
    fn unit(&mut self) -> Piece {
        self.skip_space();
        let Some(c) = self.peek() else {
            return Piece::Atom(String::new());
        };
        if let Some(piece) = self.shorthand() {
            return piece;
        }
        match c {
            '(' => self.group(')'),
            '[' => self.bracket_group(),
            '{' => self.brace_group(),
            '"' => {
                // The gaps around the string belong to the text it sets, not to the surrounding math.
                let before = self
                    .at(self.pos.saturating_sub(1))
                    .is_some_and(char::is_whitespace)
                    && self.pos > 0;
                let body = self.quoted();
                let after = self.peek().is_some_and(char::is_whitespace);
                let lead = if before { " " } else { "" };
                let trail = if after { " " } else { "" };
                Piece::Atom(format!("\\text{{{lead}{body}{trail}}}"))
            }
            '\\' => {
                self.bump();
                match self.peek() {
                    Some(' ' | '\n') => {
                        self.bump();
                        Piece::Break
                    }
                    Some(escaped) => {
                        self.bump();
                        let mut atom = escaped_atom(escaped);
                        // A control sequence needs the following gap, or its name runs on.
                        if atom.starts_with('\\') && self.peek().is_some_and(char::is_whitespace) {
                            atom.push(' ');
                        }
                        Piece::Atom(atom)
                    }
                    None => Piece::Atom(String::new()),
                }
            }
            '#' => {
                self.skip_code();
                Piece::Atom(String::new())
            }
            '0'..='9' | '.' => {
                let start = self.pos;
                while self.peek().is_some_and(|c| c.is_ascii_digit() || c == '.') {
                    self.bump();
                }
                if self.pos == start {
                    self.bump();
                }
                Piece::Atom(self.text(start, self.pos))
            }
            c if c.is_alphabetic() => self.name(),
            '|' => {
                // A bar set off by gaps on both sides is a relation; anywhere else it fences the
                // fragment it touches, so it stays adjacent to it.
                let spaced = self.pos > 0
                    && self
                        .at(self.pos.saturating_sub(1))
                        .is_some_and(char::is_whitespace)
                    && self
                        .at(self.pos.saturating_add(1))
                        .is_some_and(char::is_whitespace);
                self.bump();
                Piece::Atom(if spaced {
                    "~|~".to_string()
                } else {
                    "|".to_string()
                })
            }
            '+' | '=' | '<' | '>' => {
                self.bump();
                Piece::Op(c.to_string())
            }
            '-' => {
                self.bump();
                Piece::Op("-".to_string())
            }
            ',' | ';' | '!' | ':' | '\'' | ')' | ']' | '}' => {
                self.bump();
                Piece::Sep(c.to_string())
            }
            '%' => {
                self.bump();
                // The escape is a control sequence, so it needs the following gap or its name runs on.
                let gap = if self.peek().is_some_and(char::is_whitespace) {
                    " "
                } else {
                    ""
                };
                Piece::Atom(format!("\\%{gap}"))
            }
            other => {
                self.bump();
                match symbol_for_glyph(other) {
                    Some(tex) => Piece::Op(tex.to_string()),
                    None => Piece::Atom(other.to_string()),
                }
            }
        }
    }

    fn text(&self, start: usize, end: usize) -> String {
        self.source
            .get(start..end)
            .unwrap_or_default()
            .iter()
            .collect()
    }

    /// Read the multi-character operator shorthands Typst spells with ASCII.
    fn shorthand(&mut self) -> Option<Piece> {
        for (token, tex, relation) in MATH_SHORTHANDS {
            if self.matches(token) {
                self.pos = self.pos.saturating_add(token.chars().count());
                return Some(if *relation {
                    Piece::Op((*tex).to_string())
                } else {
                    Piece::Atom((*tex).to_string())
                });
            }
        }
        None
    }

    fn group(&mut self, close: char) -> Piece {
        self.bump();
        self.depth = self.depth.saturating_add(1);
        let inner = self.pieces(&[close]);
        self.depth = self.depth.saturating_sub(1);
        self.bump();
        Piece::Group(render_math(&inner))
    }

    fn bracket_group(&mut self) -> Piece {
        self.bump();
        self.depth = self.depth.saturating_add(1);
        let inner = self.pieces(&[']']);
        self.depth = self.depth.saturating_sub(1);
        self.bump();
        Piece::Atom(format!("[{}]", render_math(&inner)))
    }

    fn brace_group(&mut self) -> Piece {
        self.bump();
        self.depth = self.depth.saturating_add(1);
        let inner = self.pieces(&['}']);
        self.depth = self.depth.saturating_sub(1);
        self.bump();
        Piece::Atom(format!("\\{{{}\\}}", render_math(&inner)))
    }

    fn quoted(&mut self) -> String {
        self.bump();
        let mut out = String::new();
        while let Some(c) = self.bump() {
            match c {
                '"' => break,
                '\\' => {
                    if let Some(escaped) = self.bump() {
                        out.push(escaped);
                    }
                }
                other => out.push(other),
            }
        }
        out
    }

    /// Skip an embedded `#` code expression, which carries no math of its own.
    fn skip_code(&mut self) {
        self.bump();
        let mut nesting = 0usize;
        while let Some(c) = self.peek() {
            match c {
                '(' | '[' => nesting = nesting.saturating_add(1),
                ')' | ']' | ',' | ';' | ' ' | '\n' if nesting == 0 => break,
                ')' | ']' => nesting = nesting.saturating_sub(1),
                _ => {}
            }
            self.bump();
        }
    }

    /// Read an identifier: a symbol name, a math function, or a call.
    fn name(&mut self) -> Piece {
        let start = self.pos;
        while self.peek().is_some_and(char::is_alphanumeric) {
            self.bump();
        }
        let mut end = self.pos;
        let mut scan = end;
        // Every dotted continuation is tried and the longest one the table knows wins, so a name
        // whose leading components are no symbols of their own still resolves as a whole.
        while self.at(scan) == Some('.')
            && self
                .at(scan.saturating_add(1))
                .is_some_and(char::is_alphanumeric)
        {
            scan = scan.saturating_add(1);
            while self.at(scan).is_some_and(char::is_alphanumeric) {
                scan = scan.saturating_add(1);
            }
            if symbol(&self.text(start, scan)).is_some() {
                end = scan;
            }
        }
        self.pos = end;
        let name = self.text(start, end);
        if self.peek() == Some('(') {
            let args = self.call_arguments(&name);
            return math_call(&name, &args);
        }
        if let Some(entry) = symbol(&name) {
            return if entry.relation {
                Piece::Op(entry.tex.to_string())
            } else {
                Piece::Atom(entry.tex.to_string())
            };
        }
        if let Some(tex) = double_struck(&name) {
            return Piece::Atom(tex);
        }
        if MATH_OPERATORS.contains(&name.as_str()) {
            return Piece::Atom(format!("\\{name}"));
        }
        Piece::Atom(name)
    }

    /// Read a call's arguments: rows separated by `;`, arguments within a row separated by `,`.
    fn call_arguments(&mut self, name: &str) -> MathArgs {
        self.bump();
        let mut rows: Vec<Vec<MathArg>> = Vec::new();
        let mut row: Vec<MathArg> = Vec::new();
        let mut named: Vec<(String, String)> = Vec::new();
        self.depth = self.depth.saturating_add(1);
        loop {
            self.skip_space();
            if matches!(self.peek(), None | Some(')')) {
                break;
            }
            let label = self.argument_label();
            let pieces = self.pieces(&[',', ';', ')']);
            let rendered = if name == "cases" && label.is_none() {
                render_cases_row(&pieces)
            } else {
                render_math(&pieces)
            };
            match label {
                Some(key) => named.push((key, rendered)),
                None => row.push(MathArg {
                    text: rendered,
                    atomic: pieces.len() == 1,
                }),
            }
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some(';') => {
                    self.bump();
                    rows.push(std::mem::take(&mut row));
                }
                _ => break,
            }
        }
        self.depth = self.depth.saturating_sub(1);
        self.bump();
        if !row.is_empty() || rows.is_empty() {
            rows.push(row);
        }
        MathArgs { rows, named }
    }

    /// Read a `name:` prefix inside a call's argument list.
    fn argument_label(&mut self) -> Option<String> {
        let start = self.pos;
        let mut index = self.pos;
        while self.at(index).is_some_and(char::is_alphanumeric) {
            index = index.saturating_add(1);
        }
        if index == start || self.at(index) != Some(':') {
            return None;
        }
        // `:=` is an operator, not a label
        if self.at(index.saturating_add(1)) == Some('=') {
            return None;
        }
        let label = self.text(start, index);
        self.pos = index.saturating_add(1);
        Some(label)
    }
}

/// One positional argument of a math call.
#[derive(Debug)]
struct MathArg {
    /// The argument rendered as TeX.
    text: String,
    /// Whether it was a single fragment, so a script binds to it without bracing.
    atomic: bool,
}

/// The arguments of a math call, split into positional rows and named settings.
#[derive(Debug, Default)]
struct MathArgs {
    /// Positional arguments, one inner vector per `;`-separated row.
    rows: Vec<Vec<MathArg>>,
    /// Named arguments, in source order.
    named: Vec<(String, String)>,
}

impl MathArgs {
    fn positional(&self, index: usize) -> String {
        self.rows
            .first()
            .and_then(|row| row.get(index))
            .map(|arg| arg.text.clone())
            .unwrap_or_default()
    }

    /// Whether positional argument `index` binds a script without bracing.
    fn positional_atomic(&self, index: usize) -> bool {
        self.rows
            .first()
            .and_then(|row| row.get(index))
            .is_none_or(|arg| arg.atomic)
    }

    fn count(&self) -> usize {
        self.rows.first().map_or(0, Vec::len)
    }

    fn named(&self, key: &str) -> Option<&str> {
        self.named
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    /// The first row's arguments, one per line, for a stacked environment.
    fn stacked(&self) -> String {
        self.rows
            .first()
            .map(|row| Self::join_row(row, " \\\\\n"))
            .unwrap_or_default()
    }

    /// Every positional argument joined as a literal call, for a function with no TeX counterpart.
    fn joined(&self) -> String {
        self.rows
            .iter()
            .map(|row| Self::join_row(row, ","))
            .collect::<Vec<_>>()
            .join(";")
    }

    /// The arguments laid out as the rows of a TeX matrix environment.
    fn matrix(&self) -> String {
        self.rows
            .iter()
            .map(|row| Self::join_row(row, " & "))
            .collect::<Vec<_>>()
            .join(" \\\\\n")
    }

    fn join_row(row: &[MathArg], separator: &str) -> String {
        row.iter()
            .map(|arg| arg.text.as_str())
            .collect::<Vec<_>>()
            .join(separator)
    }
}

/// Render a math function call as TeX.
fn math_call(name: &str, args: &MathArgs) -> Piece {
    let first = args.positional(0);
    match name {
        "frac" => Piece::Atom(format!("\\frac{{{first}}}{{{}}}", args.positional(1))),
        "binom" => Piece::Atom(format!("\\binom{{{first}}}{{{}}}", args.positional(1))),
        "sqrt" => Piece::Atom(format!("\\sqrt{{{first}}}")),
        "root" => {
            if args.count() >= 2 {
                Piece::Atom(format!("\\sqrt[{first}]{{{}}}", args.positional(1)))
            } else {
                Piece::Atom(format!("\\sqrt{{{first}}}"))
            }
        }
        "abs" => Piece::Atom(format!("|{first}|")),
        "norm" => Piece::Atom(format!("\\left\\| {first} \\right\\|")),
        "floor" => Piece::Atom(format!("\\left\\lfloor {first} \\right\\rfloor")),
        "ceil" => Piece::Atom(format!("\\left\\lceil {first} \\right\\rceil")),
        "round" => Piece::Atom(format!("\\left\\lfloor {first} \\right\\rceil")),
        "lr" => Piece::Atom(format!("\\left. {first} \\right.")),
        "vec" => Piece::Atom(format!(
            "\\begin{{pmatrix}}\n{}\n\\end{{pmatrix}}",
            args.stacked()
        )),
        "mat" => Piece::Atom(format!(
            "\\begin{{pmatrix}}\n{}\n\\end{{pmatrix}}",
            args.matrix()
        )),
        "cases" => Piece::Atom(format!(
            "\\begin{{cases}}\n{}\n\\end{{cases}}",
            args.stacked()
        )),
        "text" => Piece::Atom(format!("\\text{{{}}}", strip_text(&first))),
        "op" => Piece::Atom(format!("\\{}", strip_text(&first))),
        "underbrace" => {
            if args.count() >= 2 {
                Piece::Atom(format!(
                    "\\underset{{{}}}{{\\underbrace{{{first}}}}}",
                    args.positional(1)
                ))
            } else {
                Piece::Atom(format!("\\underbrace{{{first}}}"))
            }
        }
        "overbrace" => {
            if args.count() >= 2 {
                Piece::Atom(format!(
                    "\\overset{{{}}}{{\\overbrace{{{first}}}}}",
                    args.positional(1)
                ))
            } else {
                Piece::Atom(format!("\\overbrace{{{first}}}"))
            }
        }
        "accent" => {
            let accent = accent_command(&args.positional(1));
            Piece::Atom(format!("{accent}{{{first}}}"))
        }
        "attach" => Piece::Atom(attachments(&first, args.positional_atomic(0), args)),
        "display" | "inline" | "script" | "sscript" | "limits" | "scripts" | "cancel"
        | "stretch" => Piece::Atom(first),
        _ => styled_call(name, &first, args),
    }
}

/// Set the six attachment slots of `attach(..)` around a base.
///
/// Corner slots become the base's own scripts; a centred slot joins them there too unless the
/// corner on its side is taken, in which case it stacks over or under the whole attachment. Left
/// corners hang off an empty box in front.
fn attachments(base: &str, atomic: bool, args: &MathArgs) -> String {
    let slots = ["t", "b", "tl", "tr", "bl", "br"];
    if !slots.iter().any(|key| args.named(key).is_some()) {
        return base.to_string();
    }
    let mut out = base.to_string();
    if takes_limits(base) {
        out.push_str("\\limits");
    }
    if !atomic {
        out = format!("{{{out}}}");
    }
    if let Some(bottom) = args.named("br").or_else(|| args.named("b")) {
        let _ = write!(out, "_{{{bottom}}}");
    }
    if let Some(top) = args.named("tr").or_else(|| args.named("t")) {
        let _ = write!(out, "^{{{top}}}");
    }
    if let (Some(top), Some(_)) = (args.named("t"), args.named("tr")) {
        out = format!("\\overset{{{top}}}{{{out}}}");
    }
    if let (Some(bottom), Some(_)) = (args.named("b"), args.named("br")) {
        out = format!("\\underset{{{bottom}}}{{{out}}}");
    }
    let mut prefix = String::new();
    if args.named("tl").is_some() || args.named("bl").is_some() {
        prefix.push_str("{}");
        if let Some(bottom) = args.named("bl") {
            let _ = write!(prefix, "_{{{bottom}}}");
        }
        if let Some(top) = args.named("tl") {
            let _ = write!(prefix, "^{{{top}}}");
        }
    }
    format!("{prefix}{out}")
}

/// Whether an operator sets its scripts above and below rather than beside it.
fn takes_limits(base: &str) -> bool {
    matches!(base, "\\lim" | "\\max" | "\\min" | "\\sup" | "\\inf")
}

/// Render a call whose name is a style command, an accent, an operator, or a plain symbol.
fn styled_call(name: &str, first: &str, args: &MathArgs) -> Piece {
    if let Some(command) = STYLE_COMMANDS
        .iter()
        .find(|(typst, _)| *typst == name)
        .map(|(_, tex)| *tex)
    {
        return Piece::Atom(format!("{command}{{{first}}}"));
    }
    if let Some(command) = ACCENTS.iter().find(|(typst, _)| *typst == name) {
        return Piece::Atom(format!("{}{{{first}}}", command.1));
    }
    if MATH_OPERATORS.contains(&name) {
        return Piece::Atom(format!("\\{name}({})", args.joined()));
    }
    let rendered = symbol(name).map_or_else(
        || double_struck(name).unwrap_or_else(|| name.to_string()),
        |entry| entry.tex.to_string(),
    );
    Piece::Atom(format!("{rendered}({})", args.joined()))
}

/// Unwrap the `\text{..}` a quoted argument renders as, so it can be re-wrapped by its caller.
fn strip_text(rendered: &str) -> String {
    rendered
        .strip_prefix("\\text{")
        .and_then(|rest| rest.strip_suffix('}'))
        .unwrap_or(rendered)
        .to_string()
}

/// The TeX accent command an `accent(..)` modifier names.
fn accent_command(modifier: &str) -> &'static str {
    ACCENTS
        .iter()
        .find(|(typst, tex)| *typst == modifier || *tex == modifier)
        .map_or("\\hat", |(_, tex)| *tex)
}

/// The blackboard-bold rendering of a doubled capital (`RR`, `NN`, …).
fn double_struck(name: &str) -> Option<String> {
    let mut chars = name.chars();
    let (first, second, rest) = (chars.next()?, chars.next()?, chars.next());
    if rest.is_none() && first == second && first.is_ascii_uppercase() {
        return Some(format!("\\mathbb{{{first}}}"));
    }
    None
}

/// One entry of the Typst symbol table.
struct Symbol {
    /// The character the name stands for in markup mode.
    glyph: &'static str,
    /// The TeX the name translates to in math mode.
    tex: &'static str,
    /// Whether the symbol is set off by spaces as a relation or binary operator.
    relation: bool,
}

/// The Typst symbol a name refers to, or `None` when the name is not modeled.
fn symbol(name: &str) -> Option<Symbol> {
    SYMBOLS
        .iter()
        .find(|(key, _, _, _)| *key == name)
        .map(|(_, glyph, tex, relation)| Symbol {
            glyph,
            tex,
            relation: *relation,
        })
}

/// The TeX for a math operator written as a raw Unicode glyph.
fn symbol_for_glyph(c: char) -> Option<&'static str> {
    let mut buffer = [0u8; 4];
    let text: &str = c.encode_utf8(&mut buffer);
    SYMBOLS
        .iter()
        .find(|(_, glyph, _, relation)| *relation && *glyph == text)
        .map(|(_, _, tex, _)| *tex)
}

/// The ASCII operator shorthands math mode expands: the token, its TeX, and whether it is spaced as
/// a relation. Listed longest first so a prefix never wins over the token that contains it.
const MATH_SHORTHANDS: &[(&str, &str, bool)] = &[
    ("<==>", "\\Longleftrightarrow", true),
    ("<-->", "\\longleftrightarrow", true),
    ("|->", "\\mapsto", true),
    (">->", "\\rightarrowtail", true),
    ("->>", "\\twoheadrightarrow", true),
    ("<<<", "\\lll", true),
    (">>>", "\\ggg", true),
    ("::=", "::=", true),
    ("<=>", "\\Leftrightarrow", true),
    ("-->", "\\longrightarrow", true),
    ("<--", "\\longleftarrow", true),
    ("==>", "\\Longrightarrow", true),
    ("<==", "\\Longleftarrow", true),
    ("...", "\\ldots", false),
    ("->", "\\rightarrow", true),
    ("<-", "\\leftarrow", true),
    ("=>", "\\Rightarrow", true),
    ("<=", "\\leq", true),
    (">=", "\\geq", true),
    ("!=", "\\neq", true),
    (":=", "\u{2254}", true),
    ("=:", "\u{2255}", true),
    ("[|", "\u{27e6}", false),
    ("|]", "\u{27e7}", false),
    ("||", "\\|", false),
    ("<<", "\\ll", true),
    (">>", "\\gg", true),
    ("~", "\\sim", true),
];

/// Typst style functions and the TeX alphabet commands they select.
const STYLE_COMMANDS: &[(&str, &str)] = &[
    ("bold", "\\mathbf"),
    ("italic", "\\mathit"),
    ("upright", "\\mathrm"),
    ("serif", "\\mathrm"),
    ("sans", "\\mathsf"),
    ("mono", "\\mathtt"),
    ("cal", "\\mathcal"),
    ("scr", "\\mathscr"),
    ("bb", "\\mathbb"),
    ("frak", "\\mathfrak"),
];

/// Typst accent functions and the TeX commands they set over their argument.
const ACCENTS: &[(&str, &str)] = &[
    ("hat", "\\hat"),
    ("widehat", "\\widehat"),
    ("tilde", "\\widetilde"),
    ("dot", "\\dot"),
    ("diaer", "\\ddot"),
    ("grave", "\\grave"),
    ("acute", "\\acute"),
    ("breve", "\\breve"),
    ("caron", "\\check"),
    ("check", "\\check"),
    ("macron", "\\overline"),
    ("overline", "\\overline"),
    ("underline", "\\underline"),
    ("arrow", "\\vec"),
    ("circle", "\\mathring"),
];

/// Function names that TeX sets as upright operators.
const MATH_OPERATORS: &[&str] = &[
    "arccos", "arcsin", "arctan", "arg", "cos", "cosh", "cot", "coth", "csc", "deg", "det", "dim",
    "exp", "gcd", "hom", "inf", "ker", "lg", "lim", "liminf", "limsup", "ln", "log", "max", "min",
    "Pr", "sec", "sin", "sinh", "sup", "tan", "tanh",
];

/// The Typst symbol names this reader models: name, markup glyph, math TeX, and whether the symbol
/// is spaced as a relation.
#[allow(clippy::type_complexity)]
const SYMBOLS: &[(&str, &str, &str, bool)] = &[
    // Greek, lowercase
    ("alpha", "\u{3b1}", "\\alpha", false),
    ("beta", "\u{3b2}", "\\beta", false),
    ("gamma", "\u{3b3}", "\\gamma", false),
    ("delta", "\u{3b4}", "\\delta", false),
    ("epsilon", "\u{3b5}", "\\epsilon", false),
    ("epsilon.alt", "\u{3f5}", "\\varepsilon", false),
    ("zeta", "\u{3b6}", "\\zeta", false),
    ("eta", "\u{3b7}", "\\eta", false),
    ("theta", "\u{3b8}", "\\theta", false),
    ("theta.alt", "\u{3d1}", "\\vartheta", false),
    ("iota", "\u{3b9}", "\\iota", false),
    ("kappa", "\u{3ba}", "\\kappa", false),
    ("lambda", "\u{3bb}", "\\lambda", false),
    ("mu", "\u{3bc}", "\\mu", false),
    ("nu", "\u{3bd}", "\\nu", false),
    ("xi", "\u{3be}", "\\xi", false),
    ("omicron", "\u{3bf}", "o", false),
    ("pi", "\u{3c0}", "\\pi", false),
    ("pi.alt", "\u{3d6}", "\\varpi", false),
    ("rho", "\u{3c1}", "\\rho", false),
    ("rho.alt", "\u{3f1}", "\\varrho", false),
    ("sigma", "\u{3c3}", "\\sigma", false),
    ("sigma.alt", "\u{3c2}", "\\varsigma", false),
    ("tau", "\u{3c4}", "\\tau", false),
    ("upsilon", "\u{3c5}", "\\upsilon", false),
    ("phi", "\u{3c6}", "\\phi", false),
    ("phi.alt", "\u{3d5}", "\\varphi", false),
    ("chi", "\u{3c7}", "\\chi", false),
    ("psi", "\u{3c8}", "\\psi", false),
    ("omega", "\u{3c9}", "\\omega", false),
    // Greek, uppercase. Letters the Latin alphabet shares have no command, so they carry the glyph.
    ("Alpha", "\u{391}", "\u{391}", false),
    ("Beta", "\u{392}", "\u{392}", false),
    ("Gamma", "\u{393}", "\\Gamma", false),
    ("Delta", "\u{394}", "\\Delta", false),
    ("Epsilon", "\u{395}", "\u{395}", false),
    ("Zeta", "\u{396}", "\u{396}", false),
    ("Eta", "\u{397}", "\u{397}", false),
    ("Theta", "\u{398}", "\\Theta", false),
    ("Iota", "\u{399}", "\u{399}", false),
    ("Kappa", "\u{39a}", "\u{39a}", false),
    ("Lambda", "\u{39b}", "\\Lambda", false),
    ("Mu", "\u{39c}", "\u{39c}", false),
    ("Nu", "\u{39d}", "\u{39d}", false),
    ("Xi", "\u{39e}", "\\Xi", false),
    ("Omicron", "\u{39f}", "\u{39f}", false),
    ("Pi", "\u{3a0}", "\\Pi", false),
    ("Rho", "\u{3a1}", "\u{3a1}", false),
    ("Sigma", "\u{3a3}", "\\Sigma", false),
    ("Tau", "\u{3a4}", "\u{3a4}", false),
    ("Upsilon", "\u{3a5}", "\\Upsilon", false),
    ("Phi", "\u{3a6}", "\\Phi", false),
    ("Chi", "\u{3a7}", "\u{3a7}", false),
    ("Psi", "\u{3a8}", "\\Psi", false),
    ("Omega", "\u{3a9}", "\\Omega", false),
    // Relations
    ("eq", "=", "=", true),
    ("eq.not", "\u{2260}", "\\neq", true),
    ("eq.def", "\u{225d}", "\u{225d}", true),
    ("eq.quest", "\u{225f}", "\u{225f}", true),
    ("lt.tri", "\u{22b2}", "\\vartriangleleft", true),
    ("gt.tri", "\u{22b3}", "\\vartriangleright", true),
    ("lt", "<", "<", true),
    ("lt.eq", "\u{2264}", "\\leq", true),
    ("lt.double", "\u{226a}", "\\ll", true),
    ("gt", ">", ">", true),
    ("gt.eq", "\u{2265}", "\\geq", true),
    ("gt.double", "\u{226b}", "\\gg", true),
    ("approx", "\u{2248}", "\\approx", true),
    ("equiv", "\u{2261}", "\\equiv", true),
    ("prop", "\u{221d}", "\\propto", true),
    ("tilde.equiv", "\u{2245}", "\\cong", true),
    ("tilde.op", "\u{223c}", "\\sim", true),
    ("in", "\u{2208}", "\\in", true),
    ("in.not", "\u{2209}", "\\notin", true),
    ("in.rev", "\u{220b}", "\\ni", true),
    ("subset", "\u{2282}", "\\subset", true),
    ("subset.eq", "\u{2286}", "\\subseteq", true),
    ("supset", "\u{2283}", "\\supset", true),
    ("supset.eq", "\u{2287}", "\\supseteq", true),
    ("divides", "\u{2223}", "\\mid", true),
    ("models", "\u{22a8}", "\\models", true),
    ("tack.r", "\u{22a2}", "\\vdash", true),
    ("tack.l", "\u{22a3}", "\\dashv", true),
    ("perp", "\u{22a5}", "\\perp", true),
    ("parallel", "\u{2225}", "\\parallel", true),
    // Binary operators
    ("plus", "+", "+", true),
    ("minus", "\u{2212}", "-", true),
    ("plus.minus", "\u{b1}", "\\pm", true),
    ("minus.plus", "\u{2213}", "\\mp", true),
    ("times", "\u{d7}", "\\times", true),
    ("div", "\u{f7}", "\\div", true),
    ("dot.op", "\u{22c5}", "\\cdot", true),
    ("ast.op", "\u{2217}", "\\ast", true),
    ("star.op", "\u{22c6}", "\\star", true),
    ("plus.circle", "\u{2295}", "\\oplus", true),
    ("times.circle", "\u{2297}", "\\otimes", true),
    ("dot.circle", "\u{2299}", "\\odot", true),
    ("and", "\u{2227}", "\\land", true),
    ("or", "\u{2228}", "\\vee", true),
    ("not", "\u{ac}", "\\neg", false),
    ("union", "\u{222a}", "\\cup", true),
    ("sect", "\u{2229}", "\\cap", true),
    ("union.big", "\u{22c3}", "\\bigcup", false),
    ("sect.big", "\u{22c2}", "\\bigcap", false),
    ("without", "\u{2216}", "\\setminus", true),
    ("compose", "\u{2218}", "\\circ", true),
    ("wreath", "\u{2240}", "\\wr", true),
    // Arrows
    ("arrow", "\u{2192}", "\\rightarrow", true),
    ("arrows", "\u{21c9}", "\\rightrightarrows", true),
    ("arrow.hook", "\u{21aa}", "\\hookrightarrow", true),
    ("mapsto", "\u{21a6}", "\\mapsto", true),
    ("arrow.r", "\u{2192}", "\\rightarrow", true),
    ("arrow.l", "\u{2190}", "\\leftarrow", true),
    ("arrow.t", "\u{2191}", "\\uparrow", true),
    ("arrow.b", "\u{2193}", "\\downarrow", true),
    ("arrow.l.r", "\u{2194}", "\\leftrightarrow", true),
    ("arrow.r.long", "\u{27f6}", "\\longrightarrow", true),
    ("arrow.l.long", "\u{27f5}", "\\longleftarrow", true),
    ("arrow.l.r.long", "\u{27f7}", "\\longleftrightarrow", true),
    ("arrow.r.double", "\u{21d2}", "\\Rightarrow", true),
    ("arrow.l.double", "\u{21d0}", "\\Leftarrow", true),
    ("arrow.l.r.double", "\u{21d4}", "\\Leftrightarrow", true),
    ("arrow.r.bar", "\u{21a6}", "\\mapsto", true),
    ("arrow.r.hook", "\u{21aa}", "\\hookrightarrow", true),
    ("arrow.l.hook", "\u{21a9}", "\\hookleftarrow", true),
    // Large operators
    ("sum", "\u{2211}", "\\sum", false),
    ("product", "\u{220f}", "\\prod", false),
    ("integral", "\u{222b}", "\\int", false),
    ("integral.double", "\u{222c}", "\\iint", false),
    ("integral.triple", "\u{222d}", "\\iiint", false),
    ("integral.cont", "\u{222e}", "\\oint", false),
    // Logic and set theory
    ("forall", "\u{2200}", "\\forall", false),
    ("exists", "\u{2203}", "\\exists", false),
    ("exists.not", "\u{2204}", "\\nexists", false),
    ("top", "\u{22a4}", "\\top", false),
    ("bot", "\u{22a5}", "\\bot", false),
    ("emptyset", "\u{2205}", "\\varnothing", false),
    ("infinity", "\u{221e}", "\\infty", false),
    ("infinity.bar", "\u{29de}", "\u{29de}", false),
    ("oo", "\u{221e}", "\\infty", false),
    ("nabla", "\u{2207}", "\\nabla", false),
    ("partial", "\u{2202}", "\\partial", false),
    ("diff", "\u{2202}", "\\partial", false),
    ("dif", "d", "d", false),
    ("therefore", "\u{2234}", "\\therefore", false),
    ("because", "\u{2235}", "\\because", false),
    // Dots and marks
    ("dots.h", "\u{2026}", "\\ldots", false),
    ("dots.c", "\u{22ef}", "\\cdots", false),
    ("dots.v", "\u{22ee}", "\\vdots", false),
    ("dots.down", "\u{22f1}", "\\ddots", false),
    ("prime", "\u{2032}", "\\prime", false),
    ("degree", "\u{b0}", "^\\circ", false),
    ("angle", "\u{2220}", "\\angle", false),
    ("aleph", "\u{5d0}", "\u{5d0}", false),
    ("ell", "\u{2113}", "\\ell", false),
    ("planck.reduce", "\u{127}", "\u{127}", false),
    ("dagger", "\u{2020}", "\\dagger", false),
    ("dagger.double", "\u{2021}", "\\ddagger", false),
    ("section", "\u{a7}", "\\S", false),
    ("copyright", "\u{a9}", "\\copyright", false),
    ("checkmark", "\u{2713}", "\u{2713}", false),
    ("crossmark", "\u{2717}", "\u{2717}", false),
    ("complement", "\u{2201}", "\\complement", false),
    // Spacing
    ("quad", "\u{2001}", "\\quad", false),
    ("wide", "\u{2001}\u{2001}", "\\quad\\quad", false),
    ("space", "\u{20}", "\\ ", false),
    ("thin", "\u{2009}", "\\,", false),
    ("med", "\u{2005}", "\\:", false),
    ("thick", "\u{2004}", "\\;", false),
    ("space.nobreak", "\u{a0}", "~", false),
    ("space.quad", "\u{2003}", "\\quad", false),
    ("space.thin", "\u{2009}", "\\,", false),
    // Punctuation, currency, and dashes
    ("bullet", "\u{2022}", "\\bullet", true),
    ("dot", "\u{22c5}", "\\cdot", true),
    ("dot.c", "\u{b7}", "\\cdot", true),
    ("dot.basic", ".", ".", false),
    ("dot.double", "\u{a8}", "\u{a8}", false),
    ("dot.triple", "\u{20db}", "\\dddot{}", false),
    ("dot.square", "\u{22a1}", "\\boxdot", true),
    ("euro", "\u{20ac}", "\u{20ac}", false),
    ("dollar", "$", "\\$", false),
    ("pound", "\u{a3}", "\\pounds", false),
    ("yen", "\u{a5}", "\u{a5}", false),
    ("cent", "\u{a2}", "\u{a2}", false),
    ("franc", "\u{20a3}", "\u{20a3}", false),
    ("won", "\u{20a9}", "\u{20a9}", false),
    ("percent", "%", "\\%", false),
    ("permille", "\u{2030}", "\u{2030}", false),
    ("hash", "#", "\\#", false),
    ("at", "@", "@", false),
    ("slash", "/", "/", false),
    ("backslash", "\\", "\\backslash", false),
    ("dash.en", "\u{2013}", "\u{2013}", false),
    ("dash.em", "\u{2014}", "\u{2014}", false),
    ("dash.wave", "\u{301c}", "\u{301c}", false),
    ("hyph", "\u{2010}", "\u{2010}", false),
    ("hyph.nobreak", "\u{2011}", "\u{2011}", false),
    ("hyph.point", "\u{2027}", "\u{2027}", false),
    ("hyph.soft", "\u{ad}", "\u{ad}", false),
    ("excl", "!", "!", false),
    ("excl.double", "\u{203c}", "!!", false),
    ("quest", "?", "?", false),
    ("quest.double", "\u{2047}", "??", false),
    ("interrobang", "\u{203d}", "\u{203d}", false),
    ("comma", ",", ",", false),
    ("semi", ";", ";", false),
    ("colon", ":", ":", false),
    ("colon.eq", "\u{2254}", "\u{2254}", true),
    ("colon.double.eq", "\u{2a74}", "::=", true),
    // Quotes, brackets, and fences
    ("quote.single", "'", "'", false),
    ("quote.double", "\"", "\"", false),
    ("quote.l.single", "\u{2018}", "\u{2018}", false),
    ("quote.l.double", "\u{201c}", "``", false),
    ("quote.r.single", "\u{2019}", "\u{2019}", false),
    ("quote.r.double", "\u{201d}", "\"", false),
    ("quote.angle.l", "\u{ab}", "\u{ab}", false),
    ("quote.angle.r", "\u{bb}", "\u{bb}", false),
    ("quote.angle.l.double", "\u{ab}", "\u{ab}", false),
    ("quote.angle.r.double", "\u{bb}", "\u{bb}", false),
    ("paren.l", "(", "(", false),
    ("paren.r", ")", ")", false),
    ("paren.double", "\u{2985}", "\u{2985}", false),
    ("paren.t", "\u{23dc}", "\u{23dc}", false),
    ("paren.b", "\u{23dd}", "\u{23dd}", false),
    ("brace.l", "{", "\\{", false),
    ("brace.r", "}", "\\}", false),
    ("bracket.l", "[", "\\lbrack", false),
    ("bracket.r", "]", "\\rbrack", false),
    ("bracket.double", "\u{27e6}", "\u{27e6}", false),
    ("bracket.double.l", "\u{27e6}", "\u{27e6}", false),
    ("bracket.double.r", "\u{27e7}", "\u{27e7}", false),
    ("angle.l", "\u{27e8}", "\\langle", false),
    ("angle.r", "\u{27e9}", "\\rangle", false),
    ("angle.l.double", "\u{27ea}", "\u{27ea}", false),
    ("angle.r.double", "\u{27eb}", "\u{27eb}", false),
    ("bar.v", "|", "|", false),
    ("bar.v.double", "\u{2016}", "\\|", false),
    ("bar.h", "\u{2015}", "\u{2015}", false),
    ("fence.l", "\u{29d8}", "\u{29d8}", false),
    ("fence.r", "\u{29d9}", "\u{29d9}", false),
    // Primes and tilde relations
    ("prime.double", "\u{2033}", "''", false),
    ("prime.triple", "\u{2034}", "'''", false),
    ("prime.rev", "\u{2035}", "\\backprime", false),
    ("tilde", "\u{223c}", "\\sim", true),
    ("tilde.dot", "\u{2a6a}", "\u{2a6a}", true),
    ("tilde.eq", "\u{2243}", "\\simeq", true),
    ("tilde.eq.not", "\u{2244}", "\u{2244}", true),
    ("tilde.eq.rev", "\u{22cd}", "\\backsimeq", true),
    ("tilde.nequiv", "\u{2246}", "\u{2246}", true),
    ("tilde.not", "\u{2241}", "\\nsim", true),
    ("tilde.rev", "\u{223d}", "\\backsim", true),
    ("tilde.triple", "\u{224b}", "\u{224b}", true),
    // Shapes, suits, and marks
    ("circle.filled", "\u{25cf}", "\u{25cf}", false),
    ("circle.stroked", "\u{25cb}", "\u{25cb}", true),
    ("circle.small", "\u{26ac}", "\u{26ac}", false),
    ("circle.stroked.tiny", "\u{2218}", "\\circ", true),
    ("circle.stroked.small", "\u{26ac}", "\u{26ac}", false),
    ("circle.stroked.big", "\u{25ef}", "\u{25ef}", false),
    ("circle.filled.tiny", "\u{2981}", "\u{2981}", false),
    ("circle.filled.small", "\u{2219}", "\\bullet", true),
    ("circle.filled.big", "\u{2b24}", "\u{2b24}", false),
    ("square.stroked", "\u{25a1}", "\\square", false),
    ("square.filled", "\u{25a0}", "\\blacksquare", false),
    ("triangle.filled.t", "\u{25b2}", "\u{25b2}", false),
    ("triangle.stroked.r", "\u{25b7}", "\\rhd", true),
    ("triangle.filled.r", "\u{25b6}", "\u{25b6}", true),
    ("star", "\u{22c6}", "\\star", true),
    ("ast", "\u{2217}", "\\ast", true),
    ("star.filled", "\u{2605}", "\\bigstar", false),
    ("star.stroked", "\u{2606}", "\u{2606}", false),
    ("suit.club", "\u{2663}", "\\clubsuit", false),
    ("suit.diamond", "\u{2666}", "\u{2666}", false),
    ("suit.heart", "\u{2665}", "\u{2665}", false),
    ("suit.spade", "\u{2660}", "\\spadesuit", false),
    ("flat", "\u{266d}", "\\flat", false),
    ("natural", "\u{266e}", "\\natural", false),
    ("sharp", "\u{266f}", "\\sharp", false),
    ("copyright.sound", "\u{2117}", "\u{2117}", false),
    ("trademark", "\u{2122}", "\u{2122}", false),
    ("dagger.triple", "\u{2e4b}", "\u{2e4b}", false),
    // Further arrows and harpoons
    ("arrow.tr", "\u{2197}", "\\nearrow", true),
    ("arrow.tl", "\u{2196}", "\\nwarrow", true),
    ("arrow.br", "\u{2198}", "\\searrow", true),
    ("arrow.bl", "\u{2199}", "\\swarrow", true),
    ("arrow.r.tail", "\u{21a3}", "\\rightarrowtail", true),
    ("arrow.l.tail", "\u{21a2}", "\\leftarrowtail", true),
    ("arrow.r.twohead", "\u{21a0}", "\\twoheadrightarrow", true),
    ("arrow.l.twohead", "\u{219e}", "\\twoheadleftarrow", true),
    ("arrow.r.squiggly", "\u{21dd}", "\\rightsquigarrow", true),
    ("arrow.r.dashed", "\u{21e2}", "\u{21e2}", false),
    ("arrow.r.stop", "\u{21e5}", "\u{21e5}", true),
    ("arrow.b.double", "\u{21d3}", "\\Downarrow", true),
    ("arrow.t.double", "\u{21d1}", "\\Uparrow", true),
    ("arrow.r.not", "\u{219b}", "\\nrightarrow", true),
    ("arrow.l.not", "\u{219a}", "\\nleftarrow", true),
    ("arrows.rr", "\u{21c9}", "\\rightrightarrows", true),
    ("arrows.ll", "\u{21c7}", "\\leftleftarrows", true),
    ("harpoon.rt", "\u{21c0}", "\\rightharpoonup", true),
    ("harpoon.lb", "\u{21bd}", "\\leftharpoondown", true),
    ("harpoons.rtlb", "\u{21cc}", "\\rightleftharpoons", true),
    // Further order and set relations
    ("lt.triple", "\u{22d8}", "\\lll", true),
    ("gt.triple", "\u{22d9}", "\\ggg", true),
    ("lt.not", "\u{226e}", "\\nless", true),
    ("gt.not", "\u{226f}", "\\ngtr", true),
    ("lt.eq.not", "\u{2270}", "\\nleq", true),
    ("gt.eq.not", "\u{2271}", "\\ngeq", true),
    ("lt.eq.slant", "\u{2a7d}", "\\leqslant", true),
    ("gt.eq.slant", "\u{2a7e}", "\\geqslant", true),
    ("prec", "\u{227a}", "\\prec", true),
    ("prec.eq", "\u{2aaf}", "\\preceq", true),
    ("succ", "\u{227b}", "\\succ", true),
    ("succ.eq", "\u{2ab0}", "\\succeq", true),
    ("subset.not", "\u{2284}", "\u{2284}", true),
    ("supset.not", "\u{2285}", "\u{2285}", true),
    ("subset.neq", "\u{228a}", "\\subsetneq", true),
    ("supset.neq", "\u{228b}", "\\supsetneq", true),
    ("in.not.rev", "\u{220c}", "\u{220c}", true),
    // Letterlike symbols, shapes, and further operators
    ("nothing", "\u{2205}", "\\varnothing", false),
    ("diameter", "\u{2300}", "\\varnothing", false),
    ("laplace", "\u{2206}", "\\mathrm{\\Delta}", false),
    ("Re", "\u{211c}", "\\Re", false),
    ("Im", "\u{2111}", "\\Im", false),
    ("image", "\u{22b7}", "\u{22b7}", true),
    ("angstrom", "\u{c5}", "\u{c5}", false),
    ("numero", "\u{2116}", "\u{2116}", false),
    ("lozenge", "\u{25ca}", "\\lozenge", false),
    ("diamond", "\u{25c7}", "\\Diamond", false),
    ("diamond.stroked", "\u{25c7}", "\\Diamond", false),
    ("square", "\u{25a1}", "\\square", false),
    ("ballot", "\u{2610}", "\u{2610}", false),
    ("smash", "\u{2a33}", "\u{2a33}", true),
    ("convolve", "\u{2217}", "\\ast", true),
    ("join", "\u{2a1d}", "\\Join", false),
    ("lt.dot", "\u{22d6}", "\\lessdot", true),
    ("gt.dot", "\u{22d7}", "\\gtrdot", true),
    ("plus.dot", "\u{2214}", "\\dotplus", true),
    ("minus.dot", "\u{2238}", "\u{2238}", true),
    ("times.div", "\u{22c7}", "\\divideontimes", true),
    ("divides.not", "\u{2224}", "\\nmid", true),
    ("asymp", "\u{224d}", "\\asymp", true),
    ("approx.eq", "\u{224a}", "\\approxeq", true),
    ("approx.not", "\u{2249}", "\u{2249}", true),
    ("integral.surf", "\u{222f}", "\u{222f}", false),
    ("integral.vol", "\u{2230}", "\u{2230}", false),
    ("integral.quad", "\u{2a0c}", "\\iiiint", false),
    ("union.sq", "\u{2294}", "\\sqcup", true),
    ("sect.sq", "\u{2293}", "\\sqcap", true),
    ("union.plus", "\u{228e}", "\\uplus", true),
    ("union.dot", "\u{228d}", "\u{228d}", true),
    ("union.minus", "\u{2a41}", "\u{2a41}", true),
    ("xor", "\u{2295}", "\\oplus", true),
    ("forces", "\u{22a9}", "\\Vdash", true),
    ("forces.not", "\u{22ae}", "\\nVdash", true),
    ("plus.square", "\u{229e}", "\\boxplus", true),
    ("minus.square", "\u{229f}", "\\boxminus", true),
    ("times.square", "\u{22a0}", "\\boxtimes", true),
    ("plus.triangle", "\u{2a39}", "\u{2a39}", true),
    ("angle.right", "\u{221f}", "\u{221f}", false),
    ("angle.spheric", "\u{2222}", "\\sphericalangle", false),
    ("angle.arc", "\u{2221}", "\\measuredangle", false),
    ("triangle.t", "\u{25b3}", "\\bigtriangleup", true),
    ("triangle.b", "\u{25bd}", "\\bigtriangledown", true),
    ("triangle.l", "\u{25c1}", "\\lhd", true),
    ("triangle.r", "\u{25b7}", "\\rhd", true),
];
