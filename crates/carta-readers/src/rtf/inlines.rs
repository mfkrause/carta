//! Character formatting state and the assembly of formatting-tagged atoms into nested inlines.

use std::mem::take;

use carta_ast::{Inline, Target};

/// The active character formatting. Copied on group entry and restored on exit. Compared for
/// equality to merge adjacent text sharing the same formatting into one run. Each field is an
/// independent on/off attribute the format toggles separately, so a flat set of flags models it
/// directly.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct CharProps {
    pub(super) bold: bool,
    pub(super) italic: bool,
    pub(super) underline: bool,
    pub(super) strike: bool,
    pub(super) superscript: bool,
    pub(super) subscript: bool,
    pub(super) smallcaps: bool,
    pub(super) allcaps: bool,
    pub(super) hidden: bool,
    /// The active font belongs to the monospace (fixed-pitch) family, so its text is code: a run
    /// spanning a whole paragraph becomes a code block, a shorter run inline code.
    pub(super) mono: bool,
}

impl CharProps {
    /// Whether two runs share the same inline wrappers, so a text run stays unbroken across them.
    /// `allcaps` is folded into each character as it is pushed and `hidden` content is dropped before
    /// it becomes a run, so neither contributes a wrapper and neither alone splits a run.
    pub(super) fn same_run(self, other: Self) -> bool {
        Self {
            allcaps: false,
            hidden: false,
            ..self
        } == Self {
            allcaps: false,
            hidden: false,
            ..other
        }
    }

    /// Whether this state yields the same inline wrapper path as `other` (see [`wrappers`]), comparing
    /// only the wrapper-bearing attributes so no path is built.
    fn same_wrappers(self, other: Self) -> bool {
        self.bold == other.bold
            && self.italic == other.italic
            && self.strike == other.strike
            && self.subscript == other.subscript
            && self.superscript == other.superscript
            && self.smallcaps == other.smallcaps
            && self.underline == other.underline
    }

    /// Whether this state carries any inline wrapper at all (see [`wrappers`]).
    fn has_wrapper(self) -> bool {
        self.bold
            || self.italic
            || self.strike
            || self.subscript
            || self.superscript
            || self.smallcaps
            || self.underline
    }
}

/// The character formatting a paragraph style contributes. Each field is set only for an attribute
/// the style declares, so applying the style overrides exactly those attributes and leaves the rest
/// inherited. `font` records the style's selected font number, resolved to monospace membership when
/// the style is applied so it tracks the font table regardless of table order.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct StyleFormat {
    pub(super) bold: Option<bool>,
    pub(super) italic: Option<bool>,
    pub(super) underline: Option<bool>,
    pub(super) strike: Option<bool>,
    pub(super) superscript: Option<bool>,
    pub(super) subscript: Option<bool>,
    pub(super) smallcaps: Option<bool>,
    pub(super) allcaps: Option<bool>,
    pub(super) hidden: Option<bool>,
    pub(super) font: Option<i32>,
}

impl StyleFormat {
    /// Folds one control word from a style definition into the accumulating format.
    pub(super) fn apply_control(&mut self, word: &str, param: Option<i32>) {
        let on = param != Some(0);
        match word {
            "b" => self.bold = Some(on),
            "i" => self.italic = Some(on),
            "ul" => self.underline = Some(on),
            "ulnone" => self.underline = Some(false),
            "uld" | "uldb" | "ulw" | "uldash" | "uldashd" | "uldashdd" | "ulhwave" | "ulth"
            | "ulthd" | "ulwave" => self.underline = Some(true),
            "strike" | "striked" => self.strike = Some(on),
            "super" | "superscript" => self.superscript = Some(on),
            "sub" | "subscript" => self.subscript = Some(on),
            "nosupersub" => {
                self.superscript = Some(false);
                self.subscript = Some(false);
            }
            "scaps" => self.smallcaps = Some(on),
            "caps" => self.allcaps = Some(on),
            "v" => self.hidden = Some(on),
            "plain" => {
                let font = self.font;
                *self = Self {
                    bold: Some(false),
                    italic: Some(false),
                    underline: Some(false),
                    strike: Some(false),
                    superscript: Some(false),
                    subscript: Some(false),
                    smallcaps: Some(false),
                    allcaps: Some(false),
                    hidden: Some(false),
                    font,
                };
            }
            "f" => self.font = param,
            _ => {}
        }
    }
}

/// A group's saved state: the character formatting plus the Unicode fallback skip count (`\ucN`).
#[derive(Debug, Clone, Copy)]
pub(super) struct GroupState {
    pub(super) props: CharProps,
    pub(super) uc: i32,
}

impl Default for GroupState {
    fn default() -> Self {
        Self {
            props: CharProps::default(),
            uc: 1,
        }
    }
}

/// One enclosing inline wrapper. The declaration order is the nesting order applied to a run:
/// earlier variants wrap later ones, regardless of the order the source enabled them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wrapper {
    Strong,
    Emph,
    Strikeout,
    Subscript,
    Superscript,
    SmallCaps,
    Underline,
}

impl Wrapper {
    fn wrap(self, children: Vec<Inline>) -> Inline {
        match self {
            Wrapper::Strong => Inline::Strong(children),
            Wrapper::Emph => Inline::Emph(children),
            Wrapper::Strikeout => Inline::Strikeout(children),
            Wrapper::Subscript => Inline::Subscript(children),
            Wrapper::Superscript => Inline::Superscript(children),
            Wrapper::SmallCaps => Inline::SmallCaps(children),
            Wrapper::Underline => Inline::Underline(children),
        }
    }
}

/// The wrapper path implied by a character state, outermost first.
fn wrappers(props: CharProps) -> Vec<Wrapper> {
    let mut path = Vec::new();
    if props.bold {
        path.push(Wrapper::Strong);
    }
    if props.italic {
        path.push(Wrapper::Emph);
    }
    if props.strike {
        path.push(Wrapper::Strikeout);
    }
    if props.subscript {
        path.push(Wrapper::Subscript);
    }
    if props.superscript {
        path.push(Wrapper::Superscript);
    }
    if props.smallcaps {
        path.push(Wrapper::SmallCaps);
    }
    if props.underline {
        path.push(Wrapper::Underline);
    }
    path
}

/// A leaf produced within a paragraph, tagged with the formatting active when it was emitted.
#[derive(Debug, Clone)]
pub(super) struct Atom {
    pub(super) props: CharProps,
    pub(super) kind: AtomKind,
}

#[derive(Debug, Clone)]
pub(super) enum AtomKind {
    Text(String),
    Space,
    LineBreak,
    /// An already-built inline (link, image, note, or bookmark span) inserted verbatim.
    Node(Inline),
}

/// Unwraps a bold or italic emphasis that spans an entire heading. A heading's level already conveys
/// prominence, so a single `Strong` or `Emph` enclosing all of its content is replaced by that
/// content, repeatedly while one remains (so nested bold-in-italic collapses fully). Emphasis over
/// only part of the heading, or any other kind of wrapper, is left untouched.
pub(super) fn strip_heading_emphasis(mut inlines: Vec<Inline>) -> Vec<Inline> {
    while inlines.len() == 1 {
        match inlines.first() {
            Some(Inline::Strong(_) | Inline::Emph(_)) => {}
            _ => break,
        }
        match inlines.pop() {
            Some(Inline::Strong(children) | Inline::Emph(children)) => inlines = children,
            _ => break,
        }
    }
    inlines
}

/// Builds nested inlines from a flat, formatting-tagged atom sequence. Adjacent atoms sharing a
/// common wrapper prefix stay inside a single instance of that wrapper; a divergence closes the
/// wrappers past the shared prefix and opens the new ones.
pub(super) fn build_inlines(atoms: Vec<Atom>) -> Vec<Inline> {
    let atoms = collapse_mono(atoms);
    let mut root: Vec<Inline> = Vec::new();
    let mut open: Vec<(Wrapper, Vec<Inline>)> = Vec::new();

    let close_to =
        |open: &mut Vec<(Wrapper, Vec<Inline>)>, root: &mut Vec<Inline>, depth: usize| {
            while open.len() > depth {
                if let Some((wrapper, children)) = open.pop() {
                    let inline = wrapper.wrap(children);
                    match open.last_mut() {
                        Some((_, parent)) => parent.push(inline),
                        None => root.push(inline),
                    }
                }
            }
        };

    for atom in atoms {
        let path = wrappers(atom.props);
        let mut shared = 0;
        while shared < open.len()
            && open
                .get(shared)
                .zip(path.get(shared))
                .is_some_and(|((wrapper, _), next)| wrapper == next)
        {
            shared += 1;
        }
        close_to(&mut open, &mut root, shared);
        for &wrapper in path.get(shared..).unwrap_or(&[]) {
            open.push((wrapper, Vec::new()));
        }
        let base = match atom.kind {
            AtomKind::Text(text) => Inline::Str(text.into()),
            AtomKind::Space => Inline::Space,
            AtomKind::LineBreak => Inline::LineBreak,
            AtomKind::Node(node) => node,
        };
        match open.last_mut() {
            Some((_, children)) => children.push(base),
            None => root.push(base),
        }
    }
    close_to(&mut open, &mut root, 0);
    root
}

/// Collapses each maximal run of contiguous monospace leaf atoms that share the same inline
/// wrappers into one code node, joining their text with a space for each [`AtomKind::Space`] and a
/// newline for each [`AtomKind::LineBreak`]. Atoms outside the monospace family, and already-built
/// nodes, pass through untouched, so a code run ends at the first differing wrapper, non-code atom,
/// or embedded node.
fn collapse_mono(atoms: Vec<Atom>) -> Vec<Atom> {
    let mut out: Vec<Atom> = Vec::new();
    let mut run: Vec<Atom> = Vec::new();
    let flush = |run: &mut Vec<Atom>, out: &mut Vec<Atom>| {
        let Some(first) = run.first() else {
            return;
        };
        let mut props = first.props;
        props.mono = false;
        let mut code = String::new();
        for atom in run.iter() {
            match &atom.kind {
                AtomKind::Text(text) => code.push_str(text),
                AtomKind::Space => code.push(' '),
                AtomKind::LineBreak => code.push('\n'),
                AtomKind::Node(_) => {}
            }
        }
        out.push(Atom {
            props,
            kind: AtomKind::Node(Inline::Code(Box::default(), code.into())),
        });
        run.clear();
    };
    for atom in atoms {
        let mono_leaf = atom.props.mono && !matches!(atom.kind, AtomKind::Node(_));
        if mono_leaf {
            let split = run
                .first()
                .is_some_and(|first| !first.props.same_wrappers(atom.props));
            if split {
                flush(&mut run, &mut out);
            }
            run.push(atom);
        } else {
            flush(&mut run, &mut out);
            out.push(atom);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// When every atom of a paragraph is monospace text carrying no other inline formatting, returns the
/// paragraph body as code (a space for each [`AtomKind::Space`], a newline for each
/// [`AtomKind::LineBreak`]); otherwise returns `None`, so the paragraph is built as inline content.
pub(super) fn mono_code_block(atoms: &[Atom]) -> Option<String> {
    let mut code = String::new();
    for atom in atoms {
        if !atom.props.mono || atom.props.has_wrapper() {
            return None;
        }
        match &atom.kind {
            AtomKind::Text(text) => code.push_str(text),
            AtomKind::Space => code.push(' '),
            AtomKind::LineBreak => code.push('\n'),
            AtomKind::Node(_) => return None,
        }
    }
    Some(code)
}

/// Distributes a hyperlink over already-built display inlines, hoisting character-formatting
/// wrappers outside the link. Adjacent inlines that carry no wrapper share a single link; each
/// formatting wrapper stays outside and has the link distributed into its children.
pub(super) fn linkify(target: &Target, inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    let mut run: Vec<Inline> = Vec::new();
    for inline in inlines {
        match into_wrapper(inline) {
            Ok((wrapper, children)) => {
                flush_link(target, &mut run, &mut out);
                out.push(wrapper.wrap(linkify(target, children)));
            }
            Err(leaf) => run.push(leaf),
        }
    }
    flush_link(target, &mut run, &mut out);
    out
}

/// Emits any accumulated non-wrapper inlines as a single link, resetting the run.
fn flush_link(target: &Target, run: &mut Vec<Inline>, out: &mut Vec<Inline>) {
    if !run.is_empty() {
        out.push(Inline::Link(
            Box::default(),
            take(run),
            Box::new(target.clone()),
        ));
    }
}

/// Decomposes an inline into its character-formatting wrapper and children, or returns it unchanged
/// when it is not one of the wrappers a run can carry.
fn into_wrapper(inline: Inline) -> std::result::Result<(Wrapper, Vec<Inline>), Inline> {
    match inline {
        Inline::Strong(children) => Ok((Wrapper::Strong, children)),
        Inline::Emph(children) => Ok((Wrapper::Emph, children)),
        Inline::Strikeout(children) => Ok((Wrapper::Strikeout, children)),
        Inline::Subscript(children) => Ok((Wrapper::Subscript, children)),
        Inline::Superscript(children) => Ok((Wrapper::Superscript, children)),
        Inline::SmallCaps(children) => Ok((Wrapper::SmallCaps, children)),
        Inline::Underline(children) => Ok((Wrapper::Underline, children)),
        other => Err(other),
    }
}
