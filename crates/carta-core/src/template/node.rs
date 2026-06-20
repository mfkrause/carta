//! The parsed template tree and its expression nodes.

/// A parsed template: a flat sequence of nodes, with `$if$`/`$for$` holding their bodies inline.
#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    pub(crate) nodes: Vec<Node>,
}

/// One element of a rendered template.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Node {
    /// Verbatim text between directives.
    Literal(String),
    /// A variable interpolation, e.g. `$title$` or `$x.y/uppercase$`.
    Var(Expr),
    /// `$if(..)$ .. $elseif(..)$ .. $else$ .. $endif$`: ordered guarded branches plus an else body.
    If {
        branches: Vec<(Expr, Vec<Node>)>,
        otherwise: Vec<Node>,
    },
    /// `$for(..)$ body $sep$ separator $endfor$`.
    For {
        expr: Expr,
        /// The single bound name when the loop expression is one bare segment, for `$name$` access
        /// inside the body (`$it$` always works regardless).
        bind: Option<String>,
        body: Vec<Node>,
        sep: Vec<Node>,
    },
    /// `$name()$`, or mapped `$xs:name()$` / `$xs:name()[sep]$`.
    Partial {
        name: String,
        map_over: Option<Expr>,
        sep: Option<String>,
    },
}

/// A variable reference: a dotted lookup path plus a chain of pipes applied to the result.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Expr {
    pub(crate) path: Vec<String>,
    pub(crate) pipes: Vec<Pipe>,
}

/// A single filter applied to a value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Pipe {
    Uppercase,
    Lowercase,
    Length,
    Reverse,
    First,
    Last,
    Rest,
    AllButLast,
    Pairs,
    Alpha,
    Roman,
    Chomp,
    Nowrap,
    /// Pad a value into a fixed-width block, optionally framed by border strings.
    Block {
        align: Align,
        width: usize,
        left: String,
        right: String,
    },
}

/// Alignment for the [`Pipe::Block`] padding filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Align {
    Left,
    Right,
    Center,
}
