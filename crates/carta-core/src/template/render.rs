//! Rendering a parsed [`Template`] against a [`Value`] context.
//!
//! ## Indentation
//!
//! When a variable, partial, or loop-item value spans multiple lines and the current output line so
//! far is entirely ASCII spaces, those spaces become the indent prefixed to every continuation line
//! of the value. A line prefix containing anything else (a tab, any non-space character) suppresses
//! the indent. Literal template text is always emitted verbatim.

use std::borrow::Cow;

use super::node::{Expr, Node, Template};
use super::pipe;
use super::{TemplateError, Value};

/// Guards against unbounded partial recursion (a partial that includes itself).
const MAX_DEPTH: usize = 64;

/// A loop binding: the current element, and the bare name it is reachable under (besides `$it$`).
struct Scope {
    bind: Option<String>,
    value: Value,
}

/// The growing output, plus a one-bit memory of how the last text reached it.
///
/// A block-level value (a rendered body or block metadata) carries a trailing blank line. When such
/// a value sits on its own line in the template — `$body$` followed by a newline — that line's own
/// break would stack onto the value's trailing blank line and open an extra empty line. So a value
/// that ends in a newline absorbs every newline that immediately follows it: its own trailing blank
/// line stands, and the line break (or blank lines) written after it in the template are dropped.
/// Literal template text never arms this, so blank lines an author writes between literals are
/// preserved exactly.
#[derive(Default)]
struct Sink {
    buf: String,
    absorb_newline: bool,
}

impl Sink {
    /// Append literal template text verbatim, save that a preceding value's trailing newline first
    /// swallows any leading newlines of this text.
    fn push_literal(&mut self, text: &str) {
        let text = self.take_absorbed(text);
        self.buf.push_str(text);
        self.absorb_newline = false;
    }

    /// Append an interpolated value, indenting its continuation lines to a space-only current-line
    /// prefix. A preceding value's trailing newline swallows any leading newlines of this value; a
    /// value that ends in a newline arms the same rule for whatever follows.
    fn push_value(&mut self, text: &str) {
        let text = self.take_absorbed(text);
        let ends_with_newline = text.ends_with('\n');
        let indent = self.current_indent();
        if indent == 0 || !text.contains('\n') {
            self.buf.push_str(text);
        } else {
            let pad = " ".repeat(indent);
            let mut lines = text.split('\n');
            if let Some(first) = lines.next() {
                self.buf.push_str(first);
            }
            for line in lines {
                self.buf.push('\n');
                // A blank line stays blank: indenting it would leave trailing spaces on an otherwise
                // empty line, so the prefix is applied only to lines that carry content.
                if !line.is_empty() {
                    self.buf.push_str(&pad);
                    self.buf.push_str(line);
                }
            }
        }
        self.absorb_newline = ends_with_newline;
    }

    /// Drop every leading newline from `text` when a preceding value armed the rule.
    fn take_absorbed<'a>(&self, text: &'a str) -> &'a str {
        if self.absorb_newline {
            text.trim_start_matches('\n')
        } else {
            text
        }
    }

    /// The indentation to apply to a value's continuation lines: the current line's width when it is
    /// all spaces, else zero.
    fn current_indent(&self) -> usize {
        let line = match self.buf.rfind('\n') {
            Some(k) => self.buf.get(k + 1..).unwrap_or(""),
            None => self.buf.as_str(),
        };
        if !line.is_empty() && line.bytes().all(|b| b == b' ') {
            line.len()
        } else {
            0
        }
    }
}

impl Template {
    /// Render the template against `context`. `resolve_partial` maps a partial name to its source
    /// text; pass a closure returning `None` for templates that use no partials.
    ///
    /// # Errors
    /// [`TemplateError`] when a referenced partial cannot be resolved (`resolve_partial` returns
    /// `None` for a name the template actually uses).
    pub fn render(
        &self,
        context: &Value,
        resolve_partial: &dyn Fn(&str) -> Option<String>,
    ) -> Result<String, TemplateError> {
        let mut sink = Sink::default();
        let mut scopes = Vec::new();
        render_nodes(
            &self.nodes,
            context,
            &mut scopes,
            resolve_partial,
            0,
            &mut sink,
        )?;
        Ok(sink.buf)
    }
}

fn render_nodes(
    nodes: &[Node],
    ctx: &Value,
    scopes: &mut Vec<Scope>,
    resolve: &dyn Fn(&str) -> Option<String>,
    depth: usize,
    out: &mut Sink,
) -> Result<(), TemplateError> {
    for node in nodes {
        render_node(node, ctx, scopes, resolve, depth, out)?;
    }
    Ok(())
}

fn render_node(
    node: &Node,
    ctx: &Value,
    scopes: &mut Vec<Scope>,
    resolve: &dyn Fn(&str) -> Option<String>,
    depth: usize,
    out: &mut Sink,
) -> Result<(), TemplateError> {
    match node {
        Node::Literal(text) => out.push_literal(text),
        Node::Var(expr) => {
            if let Some(value) = eval(expr, ctx, scopes) {
                out.push_value(&pipe::stringify(&value));
            } else if !expr.pipes.is_empty() {
                // An absent path is an empty value; its pipe chain still applies, so `$x/length$`
                // on a missing `x` yields `0` rather than vanishing.
                let mut value = Value::Str(String::new());
                for filter in &expr.pipes {
                    value = pipe::apply(&value, filter);
                }
                out.push_value(&pipe::stringify(&value));
            }
        }
        Node::If {
            branches,
            otherwise,
        } => {
            for (cond, body) in branches {
                if eval(cond, ctx, scopes)
                    .as_deref()
                    .is_some_and(Value::is_truthy)
                {
                    return render_nodes(body, ctx, scopes, resolve, depth, out);
                }
            }
            render_nodes(otherwise, ctx, scopes, resolve, depth, out)?;
        }
        Node::For {
            expr,
            bind,
            body,
            sep,
        } => render_for(
            expr,
            bind.as_ref(),
            body,
            sep,
            ctx,
            scopes,
            resolve,
            depth,
            out,
        )?,
        Node::Partial {
            name,
            map_over,
            sep,
        } => render_partial(
            name,
            map_over.as_ref(),
            sep.as_ref(),
            ctx,
            scopes,
            resolve,
            depth,
            out,
        )?,
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_for(
    expr: &Expr,
    bind: Option<&String>,
    body: &[Node],
    sep: &[Node],
    ctx: &Value,
    scopes: &mut Vec<Scope>,
    resolve: &dyn Fn(&str) -> Option<String>,
    depth: usize,
    out: &mut Sink,
) -> Result<(), TemplateError> {
    let Some(items) = eval(expr, ctx, scopes).map(into_items) else {
        return Ok(());
    };
    for (i, item) in items.into_iter().enumerate() {
        if i > 0 {
            render_nodes(sep, ctx, scopes, resolve, depth, out)?;
        }
        scopes.push(Scope {
            bind: bind.cloned(),
            value: item,
        });
        let result = render_nodes(body, ctx, scopes, resolve, depth, out);
        scopes.pop();
        result?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_partial(
    name: &str,
    map_over: Option<&Expr>,
    sep: Option<&String>,
    ctx: &Value,
    scopes: &mut Vec<Scope>,
    resolve: &dyn Fn(&str) -> Option<String>,
    depth: usize,
    out: &mut Sink,
) -> Result<(), TemplateError> {
    if depth >= MAX_DEPTH {
        return Ok(());
    }
    let Some(source) = resolve(name) else {
        return Err(TemplateError::new(format!(
            "partial `{name}` could not be found"
        )));
    };
    // A partial drops a single trailing newline from its source, so the line a `$name()$` sits on
    // is not forced open by the partial file's own final newline.
    let source = source.strip_suffix('\n').unwrap_or(&source);
    let template = Template::parse(source)?;
    match map_over {
        None => {
            let rendered = render_to_string(&template.nodes, ctx, scopes, resolve, depth + 1)?;
            out.push_value(&rendered);
        }
        Some(expr) => {
            let Some(items) = eval(expr, ctx, scopes).map(into_items) else {
                return Ok(());
            };
            let separator = sep.cloned().unwrap_or_default();
            let mut pieces = Vec::new();
            for item in items {
                scopes.push(Scope {
                    bind: None,
                    value: item,
                });
                let result = render_to_string(&template.nodes, ctx, scopes, resolve, depth + 1);
                scopes.pop();
                pieces.push(result?);
            }
            out.push_value(&pieces.join(&separator));
        }
    }
    Ok(())
}

/// Render `nodes` into an independent string, used for a partial's body before it is interpolated as
/// a single value into the surrounding output.
fn render_to_string(
    nodes: &[Node],
    ctx: &Value,
    scopes: &mut Vec<Scope>,
    resolve: &dyn Fn(&str) -> Option<String>,
    depth: usize,
) -> Result<String, TemplateError> {
    let mut sink = Sink::default();
    render_nodes(nodes, ctx, scopes, resolve, depth, &mut sink)?;
    Ok(sink.buf)
}

/// A scalar or map iterates as a single element; a list iterates its elements.
fn into_items(value: Cow<'_, Value>) -> Vec<Value> {
    match value.into_owned() {
        Value::List(items) => items,
        other => vec![other],
    }
}

/// Resolve an expression to a value, applying its pipes. `None` means the path is absent.
fn eval<'a>(expr: &Expr, ctx: &'a Value, scopes: &'a [Scope]) -> Option<Cow<'a, Value>> {
    let base = lookup(&expr.path, ctx, scopes)?;
    if expr.pipes.is_empty() {
        return Some(Cow::Borrowed(base));
    }
    let mut value = Cow::Borrowed(base);
    for filter in &expr.pipes {
        value = Cow::Owned(pipe::apply(value.as_ref(), filter));
    }
    Some(value)
}

/// Walk a dotted path. The head segment resolves against loop scopes (`it`, then bound names) before
/// the root context; the rest descends through maps.
fn lookup<'a>(path: &[String], ctx: &'a Value, scopes: &'a [Scope]) -> Option<&'a Value> {
    let (head, rest) = path.split_first()?;
    let base = if head == "it"
        && let Some(scope) = scopes.last()
    {
        &scope.value
    } else if let Some(scope) = scopes
        .iter()
        .rev()
        .find(|s| s.bind.as_deref() == Some(head.as_str()))
    {
        &scope.value
    } else if let Value::Map(map) = ctx {
        map.get(head)?
    } else {
        return None;
    };
    let mut current = base;
    for segment in rest {
        match current {
            Value::Map(map) => current = map.get(segment)?,
            _ => return None,
        }
    }
    Some(current)
}
