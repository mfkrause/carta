//! Presentation MathML → TeX rendering, shared by every reader that carries an embedded `<math>`
//! tree.
//!
//! The element tree is walked into a TeX string: token elements (`<mi>`, `<mn>`, `<mo>`) map to
//! their literal or symbolic form, and layout elements (`<msup>`, `<mfrac>`, `<msqrt>`, …) wrap their
//! rendered children in the matching TeX construct. An operator that reads as a binary or relation
//! symbol is spaced from its neighbors; large operators, punctuation, and fences sit tight. A
//! `mathvariant` selects the face a token is set in and passes down to the tokens a construct holds.
//!
//! Delimiters are assembled across a row rather than emitted where they stand: an opening bracket
//! takes everything up to its match as the content of a `\left`…`\right` pair, a closing bracket
//! with nothing open takes what precedes it, and a dividing or infix symbol stretches its pair over
//! the whole row. A pair that holds a single token and needs no stretching is written without the
//! `\left`…`\right` machinery.
//!
//! The walk is written against [`MathTree`], a minimal read-only view of an element, so the same
//! renderer serves the different element trees the container readers build.

mod symbols;

use symbols::Role;

/// A read-only view of a MathML element: enough of an element's shape to render it, abstracted over
/// the concrete tree a given reader parsed into.
pub(crate) trait MathTree: Sized {
    /// The element's local tag name, with any namespace prefix stripped.
    fn tag(&self) -> &str;
    /// The value of the attribute whose local name is `key`.
    fn attribute(&self, key: &str) -> Option<String>;
    /// The concatenated character data of this element and its descendants.
    fn inner_text(&self) -> String;
    /// The child elements, in order.
    fn element_children(&self) -> Vec<&Self>;
    /// The `index`-th child element, resolved without materializing the whole child list.
    fn nth_element_child(&self, index: usize) -> Option<&Self>;
}

#[cfg(any(
    feature = "docbook",
    feature = "docx",
    feature = "epub",
    feature = "odt"
))]
impl MathTree for crate::xml::Element {
    fn tag(&self) -> &str {
        crate::xml::local_name(&self.name)
    }
    fn attribute(&self, key: &str) -> Option<String> {
        self.attr(key).map(str::to_owned)
    }
    fn inner_text(&self) -> String {
        self.text()
    }
    fn element_children(&self) -> Vec<&Self> {
        self.elements().collect()
    }
    fn nth_element_child(&self, index: usize) -> Option<&Self> {
        self.elements().nth(index)
    }
}

/// Render a `<math>` element's presentation MathML to a TeX string.
pub(crate) fn to_tex<T: MathTree>(math: &T) -> String {
    let piece = wrap(&math.element_children(), Context::default());
    let tex = trim_row(&piece.tex);
    if piece.nucleus {
        format!("{{}}{tex}")
    } else {
        tex
    }
}

/// What an element inherits from the construct that holds it.
#[derive(Clone, Copy)]
struct Context<'a> {
    /// The face the enclosing construct selects.
    variant: Option<&'a str>,
    /// The variant the innermost face command written around the element stands for.
    applied: &'a str,
    /// The faces written around the element, which a character set in one of them drops.
    faces: Faces,
    /// Whether the element sits in a script, where each delimiter stands as it is written instead of
    /// pairing off with the ones around it.
    script: bool,
}

impl Default for Context<'_> {
    fn default() -> Self {
        Self {
            variant: None,
            applied: "normal",
            faces: Faces::default(),
            script: false,
        }
    }
}

impl Context<'_> {
    /// The context the content of an element renders in, where the element writes `face` around it.
    fn inside(self, face: Option<&str>) -> Self {
        Self {
            faces: self.faces.with(face),
            ..self
        }
    }

    /// The context a construct's script arguments render in.
    fn scripted(self) -> Self {
        Self {
            script: true,
            ..self
        }
    }
}

/// The faces written around a fragment, as a set of the commands standing for them.
#[derive(Clone, Copy, Default)]
struct Faces(u8);

impl Faces {
    /// The set with `face` added, where there is a face to add.
    fn with(self, face: Option<&str>) -> Self {
        Self(self.0 | face.map_or(0, Self::bit))
    }

    /// Whether the face is one of those already written.
    fn holds(self, face: &str) -> bool {
        let bit = Self::bit(face);
        bit != 0 && self.0 & bit != 0
    }

    fn bit(face: &str) -> u8 {
        match face {
            "\\mathbf" => 1,
            "\\mathit" => 1 << 1,
            "\\mathcal" => 1 << 2,
            "\\mathfrak" => 1 << 3,
            "\\mathbb" => 1 << 4,
            "\\mathsf" => 1 << 5,
            "\\mathtt" => 1 << 6,
            "\\mathrm" => 1 << 7,
            _ => 0,
        }
    }
}

/// A rendered fragment and what the code around it may do with it.
#[derive(Default)]
#[allow(clippy::struct_excessive_bools)]
struct Piece {
    tex: String,
    /// Whether the fragment needs braces where a single token is expected.
    grouped: bool,
    /// Whether a bracket pair can hold the fragment without stretching to it.
    simple: bool,
    /// Whether the fragment is held apart from its neighbors.
    spaced: bool,
    /// Whether a script attaching to the fragment needs it braced.
    braces_as_base: bool,
    /// Whether the fragment carries a script of its own stacked above and below it.
    limits: bool,
    /// Whether the fragment takes a leading empty group where it stands as a whole formula.
    nucleus: bool,
    /// How the fragment joins to the token after it.
    join: Join,
    /// The content of a pair with no delimiters of its own, which an enclosing pair takes over.
    spanned: Option<String>,
}

impl Piece {
    /// A single token: an identifier, a number, a space, or a symbol.
    fn token(tex: String) -> Self {
        Self {
            tex,
            simple: true,
            ..Self::default()
        }
    }

    /// A fragment that already reads as one unit, whatever its length.
    fn construct(tex: String) -> Self {
        Self {
            tex,
            ..Self::default()
        }
    }

    /// A fragment a script cannot attach to directly, and so takes braces where one does.
    fn compound(tex: String) -> Self {
        Self {
            tex,
            braces_as_base: true,
            ..Self::default()
        }
    }
}

/// A delimiter a `<mo>` stands for: the character, its TeX form, and how it reads.
#[derive(Clone, Copy)]
struct Delimiter {
    character: char,
    tex: &'static str,
    role: Role,
}

impl Delimiter {
    /// Whether TeX writes the delimiter as it stands, with no `\left`/`\right` pair to size it.
    fn is_plain(self) -> bool {
        matches!(self.character, '(' | ')' | '[' | ']' | '|')
    }

    /// The operand of `\left`/`\right`: a sign carries no bracket, leaving the pair empty.
    fn bracket(self) -> &'static str {
        if self.role.is_sign() { "." } else { self.tex }
    }

    /// The sign a delimiter with no bracket of its own contributes to the content.
    fn sign(self) -> &'static str {
        if self.role.is_sign() { self.tex } else { "" }
    }
}

/// One row of elements assembled up to the delimiter that ends it.
struct Level {
    pieces: Vec<Piece>,
    close: Option<Delimiter>,
    /// The index just past the delimiter that ended the level.
    next: usize,
}

/// How a token joins to the one that follows it.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Join {
    /// Separate a following word or number wherever TeX would otherwise read the two as one control
    /// sequence.
    #[default]
    Normal,
    /// The token carries its own boundary; nothing after it needs separating.
    Tight,
    /// Always separate a following word or number, whatever the token ends with.
    Always,
}

/// Accumulates rendered tokens, separating a token from the one before it wherever TeX would
/// otherwise read the two as a single control sequence (`\int f`, not `\intf`).
#[derive(Default)]
struct Tokens {
    out: String,
    join: Join,
}

impl Tokens {
    fn push(&mut self, tex: &str, join: Join) {
        if self.needs_separator(tex) {
            self.out.push(' ');
        }
        self.out.push_str(tex);
        self.join = join;
    }

    /// Open a gap around a spaced operator, unless one is already there.
    fn push_gap(&mut self) {
        if !self.out.ends_with(' ') {
            self.out.push(' ');
            self.join = Join::Normal;
        }
    }

    fn needs_separator(&self, next: &str) -> bool {
        // Only a character TeX reads as part of a control sequence, rather than as the punctuation or
        // backslash that ends one, has to be held off.
        let absorbed = next.starts_with(|c: char| !c.is_ascii() || c.is_ascii_alphanumeric());
        if !absorbed || self.out.ends_with(' ') {
            return false;
        }
        match self.join {
            Join::Always => true,
            Join::Tight => ends_with_control_word(&self.out),
            Join::Normal => {
                ends_with_control_word(&self.out) || ends_with_control_symbol(&self.out)
            }
        }
    }
}

/// Whether `s` ends with a TeX control word: a run of ASCII letters immediately preceded by a
/// backslash.
fn ends_with_control_word(s: &str) -> bool {
    let head = s.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    head.len() < s.len() && ends_with_lone_backslash(head)
}

/// Whether `s` ends with a TeX control symbol: a single non-letter immediately preceded by a
/// backslash, as in `\%` or `\|`.
fn ends_with_control_symbol(s: &str) -> bool {
    let mut chars = s.chars();
    let last = chars.next_back();
    last.is_some_and(|c| !c.is_ascii_alphabetic()) && ends_with_lone_backslash(chars.as_str())
}

/// Whether `s` ends with an odd run of backslashes, so its final backslash stands alone rather than
/// closing an escaped pair.
fn ends_with_lone_backslash(s: &str) -> bool {
    s.chars().rev().take_while(|&c| c == '\\').count() % 2 == 1
}

/// Render a sequence of element children as a row that stands on its own: a whole formula or the
/// argument of a construct.
fn render_row<T: MathTree>(elements: &[&T], ctx: Context<'_>) -> String {
    trim_row(&wrap(elements, ctx).tex)
}

/// Trim the spacing an edge operator leaves around a row, keeping a control space that the trim
/// would otherwise strip to a bare backslash.
fn trim_row(row: &str) -> String {
    let trimmed = row.trim();
    if ends_with_lone_backslash(trimmed) {
        format!("{trimmed} ")
    } else {
        trimmed.to_string()
    }
}

/// Assemble a sequence of elements into one piece: a lone piece stands for the row, and anything
/// else reads as a group, holding no spacing of its own at either edge.
fn wrap<T: MathTree>(elements: &[&T], ctx: Context<'_>) -> Piece {
    let mut pieces = assemble(elements, 0, ctx, false).pieces;
    if pieces.len() == 1 {
        return pieces.remove(0);
    }
    let mut tokens = Tokens::default();
    flatten(&pieces, &mut tokens);
    Piece {
        tex: trim_row(&tokens.out),
        grouped: true,
        ..Piece::default()
    }
}

/// Concatenate rendered pieces, bracing each one that needs to read as a unit and opening a gap
/// around each spaced operator.
fn flatten(pieces: &[Piece], tokens: &mut Tokens) {
    for piece in pieces {
        if piece.spaced {
            tokens.push_gap();
        }
        if piece.grouped {
            tokens.push(&format!("{{{}}}", piece.tex), Join::Normal);
        } else {
            tokens.push(&piece.tex, piece.join);
        }
        if piece.spaced {
            tokens.push_gap();
        }
    }
}

/// Assemble the elements from `start` into one level of delimiter nesting. `enclosed` marks a level
/// that is the content of an open delimiter, and so ends at the first delimiter that closes it.
fn assemble<T: MathTree>(elements: &[&T], start: usize, ctx: Context<'_>, enclosed: bool) -> Level {
    let mut pieces: Vec<Piece> = Vec::new();
    // where the last completed group left off, so a later closing delimiter takes only what follows
    let mut barrier = 0;
    // whether a dividing or infix symbol has claimed the run of pieces since the last group
    let mut spanning = false;
    // whether such a symbol found anything in that run to divide
    let mut spans = false;
    let mut index = start;
    while index < elements.len() {
        // a fence pair around an <mtable> reads as one delimited matrix, not three loose tokens
        if !ctx.script
            && let (Some(open), Some(table), Some(close)) = (
                elements.get(index),
                elements.get(index + 1),
                elements.get(index + 2),
            )
            && let Some(rendered) = matrix_fence(*open, *table, *close, ctx)
        {
            pieces.push(Piece::construct(rendered));
            barrier = pieces.len();
            index += 3;
            continue;
        }
        let Some(element) = elements.get(index) else {
            break;
        };
        if !ctx.script
            && let Some(delimiter) = delimiter_of(*element)
        {
            let leads = !enclosed && index == start;
            let last = index + 1 == elements.len();
            // A dividing symbol opens a group where nothing is open for it to divide.
            if matches!(delimiter.role, Role::Open | Role::OpenSymbol)
                || (leads && (delimiter.role.opens() || delimiter.role == Role::Middle))
            {
                let inner = assemble(elements, index + 1, ctx, true);
                pieces.push(render_group(Some(delimiter), &inner.pieces, inner.close));
                barrier = pieces.len();
                index = inner.next;
                continue;
            }
            if delimiter.role.closes() || (delimiter.role == Role::Middle && last) {
                if enclosed {
                    return Level {
                        pieces: lone_element(elements, start, index, pieces, ctx),
                        close: Some(delimiter),
                        next: index + 1,
                    };
                }
                let content = pieces.split_off(barrier);
                pieces.push(render_group(None, &content, Some(delimiter)));
                spanning = false;
                spans = false;
                barrier = pieces.len();
                index += 1;
                continue;
            }
            if delimiter.role == Role::Middle || delimiter.role == Role::Infix {
                pieces.push(if delimiter.role == Role::Middle {
                    Piece {
                        tex: format!("\\middle{}", delimiter.tex),
                        spaced: true,
                        ..Piece::default()
                    }
                } else {
                    Piece::construct(delimiter.tex.to_string())
                });
                spanning = true;
                spans = spans || pieces.len() > barrier + 1;
                index += 1;
                continue;
            }
        }
        pieces.push(render(*element, ctx));
        index += 1;
        spans = spans || (spanning && pieces.len() > barrier + 1);
    }
    let pieces = lone_element(elements, start, elements.len(), pieces, ctx);
    let pieces = if !enclosed && spans {
        vec![render_group(None, &pieces, None)]
    } else {
        pieces
    };
    Level {
        pieces,
        close: None,
        next: elements.len(),
    }
}

/// A level spanning one delimiter renders that delimiter as written, with no inferred pair.
fn lone_element<T: MathTree>(
    elements: &[&T],
    start: usize,
    end: usize,
    pieces: Vec<Piece>,
    ctx: Context<'_>,
) -> Vec<Piece> {
    if end.saturating_sub(start) != 1 {
        return pieces;
    }
    match elements.get(start) {
        Some(element) if delimiter_of(*element).is_some() => vec![render(*element, ctx)],
        _ => pieces,
    }
}

/// Render a delimited group: a pair that holds a single token and needs no sizing is written as it
/// stands, and anything else takes a `\left`…`\right` pair, with `.` where a side has no delimiter.
fn render_group(open: Option<Delimiter>, content: &[Piece], close: Option<Delimiter>) -> Piece {
    let spanned = match content.first() {
        Some(piece) if content.len() == 1 => piece.spanned.clone(),
        _ => None,
    };
    let plain = open.is_some_and(Delimiter::is_plain) && close.is_some_and(Delimiter::is_plain);
    if spanned.is_none()
        && (content.is_empty() || (plain && content.iter().all(|piece| piece.simple)))
    {
        let mut tokens = Tokens::default();
        if let Some(open) = open {
            tokens.push(open.tex, Join::Normal);
        }
        flatten(content, &mut tokens);
        if let Some(close) = close {
            tokens.push(close.tex, Join::Normal);
        }
        return Piece {
            tex: tokens.out,
            grouped: content.is_empty() && open.is_some() && close.is_some(),
            ..Piece::default()
        };
    }
    let body = if let Some(body) = spanned {
        body
    } else {
        let mut tokens = Tokens::default();
        tokens.push(open.map_or("", Delimiter::sign), Join::Normal);
        flatten(content, &mut tokens);
        trim_row(&tokens.out)
    };
    // Nothing between the delimiters holds no gap of its own.
    let padded = if body.is_empty() {
        String::new()
    } else {
        format!("{body} ")
    };
    let tex = format!(
        "\\left{} {padded}\\right{}{}",
        open.map_or(".", Delimiter::bracket),
        close.map_or(".", Delimiter::bracket),
        close.map_or("", Delimiter::sign),
    );
    Piece {
        spanned: (open.is_none() && close.is_none()).then_some(body),
        tex,
        ..Piece::default()
    }
}

/// The delimiter or infix symbol an element stands for, or `None` for anything that renders as
/// content. Only an operator carries a reading; the same character elsewhere is a literal symbol.
fn delimiter_of<T: MathTree>(e: &T) -> Option<Delimiter> {
    if e.tag() != "mo" {
        return None;
    }
    let text = e.inner_text();
    let text = trim_token(&text);
    let mut characters = text.chars();
    let character = characters.next()?;
    if characters.next().is_some() {
        let (tex, role) = symbols::operator(text)?;
        return (role == Role::Infix).then_some(Delimiter {
            character,
            tex,
            role,
        });
    }
    let (tex, role) = symbols::symbol(character)?;
    (role.is_delimiter() || role == Role::Infix).then_some(Delimiter {
        character,
        tex,
        role,
    })
}

/// Render one element in the context the construct that holds it passes down. An element's own
/// `mathvariant` supersedes the face it inherits and passes on in its place.
fn render<T: MathTree>(e: &T, inherited: Context<'_>) -> Piece {
    let own = e.attribute("mathvariant");
    let selected = own.as_deref().or(inherited.variant).map(selected_face);
    let variant = selected.map(|(variant, _)| variant);
    // The face already written around an element is not written a second time inside it.
    let face = selected
        .filter(|(selected, _)| *selected != inherited.applied)
        .map(|(_, command)| command);
    let ctx = Context {
        variant,
        ..inherited
    };
    let text = e.inner_text();
    match e.tag() {
        "mi" => render_identifier(trim_token(&text), face, ctx.inside(face)),
        "mn" => Piece::token(map_characters(trim_token(&text), ctx.faces)),
        "mo" => render_operator(trim_token(&text), ctx.faces),
        "mtext" if text.trim().is_empty() => Piece::token(String::new()),
        "mtext" => Piece::construct(in_text_face(variant, &escape_text(text.trim(), ctx.faces))),
        "ms" => Piece::construct(render_string(e, ctx)),
        "mspace" => render_space(e),
        "msup" => Piece::compound(render_script(e, '^', ctx)),
        "msub" => Piece::compound(render_script(e, '_', ctx)),
        "msubsup" => Piece::compound(render_subsup(e, ctx)),
        "mfrac" => Piece::construct(render_binary(e, "\\frac", ctx)),
        "msqrt" => Piece::compound(format!(
            "\\sqrt{{{}}}",
            render_row(&e.element_children(), ctx)
        )),
        "mroot" => Piece::compound(render_root(e, ctx)),
        "mover" => Piece::compound(render_over(e, ctx)),
        "munder" => Piece::compound(render_under(e, ctx)),
        "munderover" => Piece::compound(render_underover(e, ctx)),
        "mfenced" => render_fenced(e, ctx),
        "mtable" => Piece::construct(render_mtable(e, "matrix", ctx)),
        "mmultiscripts" => render_mmultiscripts(e, ctx),
        "mphantom" => Piece::compound(format!(
            "\\phantom{{{}}}",
            render_row(&e.element_children(), ctx)
        )),
        "menclose" => Piece::construct(render_menclose(e, ctx)),
        "semantics" => render_semantics(e, ctx),
        // A style wrapper restyles the row it holds as a whole, rather than each token in it, and one
        // left with no variant in force restyles it to the plain face.
        "mstyle" => {
            let (selected, command) = selected_face(variant.unwrap_or("normal"));
            let face = (selected != inherited.applied).then_some(command);
            let held = Context {
                variant: None,
                applied: selected,
                ..ctx.inside(face)
            };
            match face {
                Some(command) => Piece::construct(format!(
                    "{command}{{{}}}",
                    render_row(&e.element_children(), held)
                )),
                None => wrap(&e.element_children(), held),
            }
        }
        // A grouping or presentational wrapper carries no structure of its own: render its content.
        _ => wrap(&e.element_children(), ctx),
    }
}

/// Trim the whitespace an element's markup leaves around a token's text. Only the space characters
/// XML uses for layout are stripped, so that a no-break space keeps its symbol.
fn trim_token(text: &str) -> &str {
    text.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r'))
}

/// The variant an element sets and the TeX command that writes it, where a `mathvariant` with no
/// command of its own reads as the plain face.
fn selected_face(variant: &str) -> (&str, &'static str) {
    match math_face(variant) {
        Some(command) => (variant, command),
        None => ("normal", "\\mathrm"),
    }
}

/// The TeX command a `mathvariant` selects in math mode, or `None` for a variant with no command of
/// its own.
fn math_face(variant: &str) -> Option<&'static str> {
    Some(match variant {
        "bold" | "bold-italic" | "bold-sans-serif" | "sans-serif-bold-italic" => "\\mathbf",
        "italic" => "\\mathit",
        "script" | "bold-script" => "\\mathcal",
        "fraktur" | "bold-fraktur" => "\\mathfrak",
        "double-struck" => "\\mathbb",
        "sans-serif" | "sans-serif-italic" => "\\mathsf",
        "monospace" => "\\mathtt",
        "normal" => "\\mathrm",
        _ => return None,
    })
}

/// Rendered math set in the face given, or left as it stands where there is no face to write.
fn in_face(face: Option<&str>, content: &str) -> String {
    match face {
        Some(command) => format!("{command}{{{content}}}"),
        None => content.to_string(),
    }
}

/// The text-mode commands a `mathvariant` selects, innermost first. An empty list leaves the plain
/// text box.
fn text_face(variant: &str) -> &'static [&'static str] {
    match variant {
        "bold" => &["\\textbf"],
        "italic" => &["\\textit"],
        "bold-italic" => &["\\textbf", "\\textit"],
        "sans-serif" => &["\\textsf"],
        "bold-sans-serif" => &["\\textsf", "\\textbf"],
        "sans-serif-italic" => &["\\textsf", "\\textit"],
        "sans-serif-bold-italic" => &["\\textsf", "\\textit", "\\textbf"],
        "monospace" => &["\\texttt"],
        _ => &[],
    }
}

/// Escaped literal text set in the face its variant selects, in a plain `\text` box when the variant
/// selects none.
fn in_text_face(variant: Option<&str>, escaped: &str) -> String {
    let commands = variant.map(text_face).unwrap_or_default();
    let Some((innermost, rest)) = commands.split_first() else {
        return format!("\\text{{{escaped}}}");
    };
    let mut out = format!("{innermost}{{{escaped}}}");
    for command in rest {
        out = format!("{command}{{{out}}}");
    }
    out
}

/// The `index`-th element child.
fn nth_child<T: MathTree>(e: &T, index: usize) -> Option<&T> {
    e.nth_element_child(index)
}

/// The `index`-th child as a piece, or an empty piece where there is no such child.
fn child_piece<T: MathTree>(e: &T, index: usize, ctx: Context<'_>) -> Piece {
    nth_child(e, index).map_or_else(Piece::default, |child| render(child, ctx))
}

/// The `index`-th child as the argument of a construct, which holds it without needing braces of
/// its own.
fn rendered_child<T: MathTree>(e: &T, index: usize, ctx: Context<'_>) -> String {
    trim_row(&child_piece(e, index, ctx).tex)
}

/// A piece filling a slot that holds it as it stands: a group takes braces, a spaced operator keeps
/// the gaps around it, and an empty piece contributes nothing.
fn as_slot(piece: &Piece) -> String {
    if piece.tex.is_empty() {
        return String::new();
    }
    let mut tokens = Tokens::default();
    flatten(std::slice::from_ref(piece), &mut tokens);
    tokens.out
}

/// A single-script element (`<msup>`/`<msub>`): base plus one script in braces.
fn render_script<T: MathTree>(e: &T, marker: char, ctx: Context<'_>) -> String {
    let base = brace_base(&child_piece(e, 0, ctx));
    let script = rendered_child(e, 1, ctx.scripted());
    format!("{base}{marker}{{{script}}}")
}

/// `<msubsup>`: base with both a subscript and a superscript.
fn render_subsup<T: MathTree>(e: &T, ctx: Context<'_>) -> String {
    let base = brace_base(&child_piece(e, 0, ctx));
    let sub = rendered_child(e, 1, ctx.scripted());
    let sup = rendered_child(e, 2, ctx.scripted());
    format!("{base}_{{{sub}}}^{{{sup}}}")
}

/// A two-argument construct written `cmd{first}{second}`, e.g. `<mfrac>` → `\frac`.
fn render_binary<T: MathTree>(e: &T, command: &str, ctx: Context<'_>) -> String {
    let first = rendered_child(e, 0, ctx);
    let second = rendered_child(e, 1, ctx);
    format!("{command}{{{first}}}{{{second}}}")
}

/// `<mroot>`: base under a radical with an explicit index.
fn render_root<T: MathTree>(e: &T, ctx: Context<'_>) -> String {
    let base = rendered_child(e, 0, ctx);
    let index = as_slot(&child_piece(e, 1, ctx));
    format!("\\sqrt[{index}]{{{base}}}")
}

/// `<mover>`: a base with an overscript. A recognized accent character maps to its accent command, a
/// large operator or limit-like function carries the script with `\limits`, and anything else is
/// stacked over the base with `\overset` so its content is preserved rather than dropped.
fn render_over<T: MathTree>(e: &T, ctx: Context<'_>) -> String {
    let piece = child_piece(e, 0, ctx);
    stack_over(e, 1, &trim_row(&piece.tex), piece.limits, ctx)
}

/// Set the script at `index` above `base`: a recognized accent character takes its accent commands,
/// a base that carries its scripts stacked takes the script with `\limits`, and anything else sits
/// over the base with `\overset` so its content is preserved rather than dropped.
fn stack_over<T: MathTree>(
    e: &T,
    index: usize,
    base: &str,
    limits: bool,
    ctx: Context<'_>,
) -> String {
    let accent = nth_child(e, index)
        .map(|c| c.inner_text().trim().to_string())
        .unwrap_or_default();
    if let Some(commands) = accent_commands(&accent) {
        return commands.iter().fold(base.to_string(), |inner, command| {
            format!("{command}{{{inner}}}")
        });
    }
    let over = rendered_child(e, index, ctx.scripted());
    if limits {
        format!("{base}\\limits^{{{over}}}")
    } else {
        format!("\\overset{{{over}}}{{{base}}}")
    }
}

/// Whether an operator character carries its scripts stacked above and below it (`\sum\limits_{...}`)
/// rather than beside it: the large operators, the integrals, and the n-ary set and logic operators.
/// Anything else, an ordinary symbol, a Greek letter, or a compound expression, takes an
/// `\underset`/`\overset` instead, since `\limits` is only valid after an operator.
fn takes_limits(character: char) -> bool {
    matches!(character,
        '|' | '\u{2140}' | '\u{220f}' | '\u{2210}' | '\u{2211}' | '\u{29f8}' | '\u{29f9}'
        | '\u{2afc}' | '\u{2aff}'
        | '\u{222b}'..='\u{2233}'
        | '\u{22c0}'..='\u{22c3}'
        | '\u{27d5}'..='\u{27d9}'
        | '\u{2a00}'..='\u{2a09}'
        | '\u{2a0b}'..='\u{2a21}')
}

/// `<munder>`: an under-script. A large operator or limit-like function carries its script with
/// `\limits`; anything else uses `\underset`.
fn render_under<T: MathTree>(e: &T, ctx: Context<'_>) -> String {
    let piece = child_piece(e, 0, ctx);
    let base = trim_row(&piece.tex);
    let under = rendered_child(e, 1, ctx.scripted());
    if piece.limits {
        format!("{base}\\limits_{{{under}}}")
    } else {
        format!("\\underset{{{under}}}{{{base}}}")
    }
}

/// `<munderover>`: both an under-script and an over-script on the base.
fn render_underover<T: MathTree>(e: &T, ctx: Context<'_>) -> String {
    let piece = child_piece(e, 0, ctx);
    let base = trim_row(&piece.tex);
    let under = rendered_child(e, 1, ctx.scripted());
    // A script drawn across the whole base covers the underscript with it.
    if nth_child(e, 2).is_some_and(covers_base) {
        let stacked = if piece.limits {
            format!("{base}\\limits_{{{under}}}")
        } else {
            format!("\\underset{{{under}}}{{{base}}}")
        };
        return stack_over(e, 2, &stacked, false, ctx);
    }
    let over = rendered_child(e, 2, ctx.scripted());
    if piece.limits {
        format!("{base}\\limits_{{{under}}}^{{{over}}}")
    } else {
        format!("\\underset{{{under}}}{{\\overset{{{over}}}{{{base}}}}}")
    }
}

/// Whether a script is a lone operator that is drawn across everything below it, an accent, a bar or
/// brace, or a stretchy horizontal arrow.
fn covers_base<T: MathTree>(script: &T) -> bool {
    let mut element = script;
    while element.tag() != "mo" {
        match element.element_children().as_slice() {
            [only] => element = only,
            _ => return false,
        }
    }
    let text = element.inner_text();
    let mut characters = text.trim().chars();
    match (characters.next(), characters.next()) {
        (Some(character), None) => symbols::stacks_over(character),
        _ => false,
    }
}

/// `<mfenced>`: children wrapped in delimiters, defaulting to parentheses with comma separators. The
/// `separators` attribute lists one character per gap between children (whitespace ignored); when the
/// children outnumber the listed separators the last one repeats, and an explicitly empty list places
/// no separators at all. A delimiter that names no bracket is written as an operator beside the
/// content, and a pair of empty delimiters leaves the content bare.
fn render_fenced<T: MathTree>(e: &T, ctx: Context<'_>) -> Piece {
    let open = e.attribute("open").unwrap_or_else(|| "(".to_string());
    let close = e.attribute("close").unwrap_or_else(|| ")".to_string());
    let children = e.element_children();
    if !ctx.script
        && let [table] = children.as_slice()
        && table.tag() == "mtable"
        && let Some(rendered) = fenced_matrix(*table, &open, &close, ctx)
    {
        return Piece::construct(rendered);
    }
    let separators: Vec<char> = e
        .attribute("separators")
        .unwrap_or_else(|| ",".to_string())
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let mut content: Vec<Piece> = Vec::new();
    for (index, child) in children.iter().enumerate() {
        if index > 0
            && let Some(separator) = separators.get(index - 1).or_else(|| separators.last())
        {
            content.push(render_operator(&separator.to_string(), ctx.faces));
        }
        let rendered = render(*child, ctx);
        if !rendered.tex.is_empty() {
            content.push(rendered);
        }
    }
    let content = match content.len() {
        0 => None,
        1 => Some(content.remove(0)),
        _ => Some(as_group(&content)),
    };
    let open_fence = fence_attribute(&open, opens_fence, ctx.faces);
    let close_fence = fence_attribute(&close, closes_fence, ctx.faces);
    if ctx.script {
        return scripted_fence(open_fence, content, close_fence);
    }
    // An opening attribute that only ever closes shuts a pair of its own, ahead of the content.
    let (before, open_fence) = match open_fence {
        Fence::Beside(operator) if operator.closing => (
            Some(render_group(None, &[], operator.delimiter)),
            Fence::None,
        ),
        other => (None, other),
    };
    if before.is_none() && matches!(open_fence, Fence::None) && matches!(close_fence, Fence::None) {
        return content.unwrap_or_default();
    }
    let mut pieces = Vec::new();
    let open_bracket = match open_fence {
        Fence::Bracket(delimiter) => Some(delimiter),
        Fence::Beside(operator) => {
            pieces.push(operator.piece);
            None
        }
        Fence::None => None,
    };
    // A pair with nothing between its delimiters still holds a group, empty as it is.
    pieces.push(content.unwrap_or(Piece {
        grouped: true,
        ..Piece::default()
    }));
    let close_bracket = match close_fence {
        Fence::Bracket(delimiter) => Some(delimiter),
        Fence::Beside(operator) => {
            pieces.push(operator.piece);
            None
        }
        Fence::None => None,
    };
    let group = match (open_bracket, close_bracket) {
        (None, None) => as_group(&pieces),
        (open_bracket, close_bracket) => render_group(open_bracket, &pieces, close_bracket),
    };
    match before {
        Some(before) => as_group(&[before, group]),
        None => group,
    }
}

/// Rendered pieces taken together as one group.
fn as_group(pieces: &[Piece]) -> Piece {
    let mut tokens = Tokens::default();
    flatten(pieces, &mut tokens);
    Piece {
        tex: trim_row(&tokens.out),
        grouped: true,
        ..Piece::default()
    }
}

/// In a script the delimiters stand as they are written, around whatever content there is.
fn scripted_fence(open: Fence, content: Option<Piece>, close: Fence) -> Piece {
    let mut tokens = Tokens::default();
    match open {
        Fence::Bracket(delimiter) => tokens.push(delimiter.tex, Join::Normal),
        Fence::Beside(operator) => flatten(&[operator.piece], &mut tokens),
        Fence::None => {}
    }
    if let Some(content) = content {
        flatten(std::slice::from_ref(&content), &mut tokens);
    }
    match close {
        Fence::Bracket(delimiter) => tokens.push(delimiter.tex, Join::Normal),
        Fence::Beside(operator) => flatten(&[operator.piece], &mut tokens),
        Fence::None => {}
    }
    Piece {
        tex: tokens.out,
        grouped: true,
        ..Piece::default()
    }
}

/// A fence pair given as `mfenced` attributes around a lone table, taken together as one delimited
/// matrix.
fn fenced_matrix<T: MathTree>(
    table: &T,
    open: &str,
    close: &str,
    ctx: Context<'_>,
) -> Option<String> {
    if let Some(env) = matrix_env(open, close) {
        return Some(render_mtable(table, env, ctx));
    }
    let (left, right) = (left_right_delim(open)?, left_right_delim(close)?);
    Some(format!(
        "\\left{left} {} \\right{right}",
        render_mtable(table, "matrix", ctx)
    ))
}

/// What a delimiter attribute contributes on its side of the content.
enum Fence {
    /// Nothing at all.
    None,
    /// A bracket the pair stretches to.
    Bracket(Delimiter),
    /// An operator standing beside the content.
    Beside(FenceOperator),
}

/// An operator a delimiter attribute renders as, and the delimiter it stands for where the attribute
/// names one that reads the wrong way round for its side.
struct FenceOperator {
    piece: Piece,
    closing: bool,
    delimiter: Option<Delimiter>,
}

/// Whether a symbol reads as the opening side of a pair.
fn opens_fence(role: Role) -> bool {
    role.opens() || role == Role::Middle
}

/// Whether a symbol reads as the closing side of a pair.
fn closes_fence(role: Role) -> bool {
    role.closes() || role == Role::Middle
}

/// Read an `mfenced` delimiter attribute: either the bracket it names, or the operator it renders as
/// beside the content. An empty attribute contributes nothing.
fn fence_attribute(text: &str, accepts: fn(Role) -> bool, faces: Faces) -> Fence {
    let mut characters = text.chars();
    let Some(character) = characters.next() else {
        return Fence::None;
    };
    let single = characters.next().is_none();
    let symbol = single.then(|| symbols::symbol(character)).flatten();
    if let Some((tex, role)) = symbol {
        let delimiter = Delimiter {
            character,
            tex,
            role,
        };
        if accepts(role) {
            return Fence::Bracket(delimiter);
        }
        return Fence::Beside(FenceOperator {
            piece: render_operator(text, faces),
            closing: role.closes(),
            delimiter: Some(delimiter),
        });
    }
    Fence::Beside(FenceOperator {
        piece: render_operator(text, faces),
        closing: false,
        delimiter: None,
    })
}

/// The named matrix environment a fence pair selects, or `None` for a fence that keeps an explicit
/// `\left`…`\right` wrapping instead.
fn matrix_env(open: &str, close: &str) -> Option<&'static str> {
    match (open, close) {
        ("(", ")") => Some("pmatrix"),
        ("[", "]") => Some("bmatrix"),
        ("{", "}") => Some("Bmatrix"),
        _ => None,
    }
}

/// The `\left`/`\right` operand a stretchy bar fence maps to, for a delimiter pair with no dedicated
/// matrix environment.
fn left_right_delim(op: &str) -> Option<&'static str> {
    match op {
        "|" => Some("|"),
        "\u{2016}" => Some("\\|"),
        _ => None,
    }
}

/// An open operator, a table, and a close operator taken together as a delimited matrix: a
/// recognized bracket pair becomes the matching matrix environment, and a stretchy bar pair wraps a
/// plain matrix in `\left`…`\right`.
fn matrix_fence<T: MathTree>(open: &T, table: &T, close: &T, ctx: Context<'_>) -> Option<String> {
    if open.tag() != "mo" || table.tag() != "mtable" || close.tag() != "mo" {
        return None;
    }
    let open_text = open.inner_text();
    let close_text = close.inner_text();
    fenced_matrix(table, open_text.trim(), close_text.trim(), ctx)
}

/// `<mtable>`: rows of cells laid out as a TeX matrix, cells separated by `&` and rows by `\\`. Every
/// row is padded to the widest so the columns line up, and a multi-token cell is braced so its
/// content reads as one grid entry.
fn render_mtable<T: MathTree>(e: &T, env: &str, ctx: Context<'_>) -> String {
    let rows: Vec<Vec<String>> = e
        .element_children()
        .into_iter()
        .filter(|row| row.tag() == "mtr")
        .map(|row| {
            row.element_children()
                .into_iter()
                .filter(|cell| cell.tag() == "mtd")
                .map(|cell| as_slot(&wrap(&cell.element_children(), ctx)))
                .collect()
        })
        .collect();
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    let lines: Vec<String> = rows
        .into_iter()
        .map(|mut cells| {
            cells.resize(width, String::new());
            join_cells(&cells)
        })
        .collect();
    let last = lines.len().saturating_sub(1);
    let body: Vec<String> = lines
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            if index == last {
                line
            } else {
                format!("{} \\\\", line.trim_end())
            }
        })
        .collect();
    format!("\\begin{{{env}}}\n{}\n\\end{{{env}}}", body.join("\n"))
}

/// One row of rendered cells, each set off from the one before it by an ampersand that keeps the
/// spacing the cell itself ends with, with the gap around the ampersand written once.
fn join_cells(cells: &[String]) -> String {
    let mut row = String::new();
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            if !row.ends_with(' ') {
                row.push(' ');
            }
            row.push_str("& ");
        }
        row.push_str(if row.ends_with(' ') {
            cell.trim_start_matches(' ')
        } else {
            cell
        });
    }
    row
}

/// `<mmultiscripts>`: a base carrying post-scripts and, after an `<mprescripts/>` marker, pre-scripts.
/// The subscripts on a side gather into one subscript group and the superscripts into one superscript
/// group, so the base takes at most a single `_` and `^` per side rather than an invalid chain of
/// them. A side carrying any slot at all emits both its groups even when a `<none/>` slot leaves one
/// empty, and pre-scripts sit behind a leading empty nucleus that gives them something to attach to.
#[allow(clippy::similar_names)]
fn render_mmultiscripts<T: MathTree>(e: &T, ctx: Context<'_>) -> Piece {
    let children = e.element_children();
    let mut iter = children.into_iter();
    let base = iter
        .next()
        .map_or_else(Piece::default, |element| render(element, ctx));
    let mut pre = ScriptSide::default();
    let mut post = ScriptSide::default();
    let mut in_pre = false;
    while let Some(sub_element) = iter.next() {
        if sub_element.tag() == "mprescripts" {
            in_pre = true;
            continue;
        }
        let target_pre = in_pre;
        let sub = script_token(sub_element, ctx);
        let sup = match iter.next() {
            Some(element) if element.tag() == "mprescripts" => {
                in_pre = true;
                String::new()
            }
            Some(element) => script_token(element, ctx),
            None => String::new(),
        };
        let side = if target_pre { &mut pre } else { &mut post };
        side.push(&sub, &sup);
    }
    let nucleus = if pre.present { "{}" } else { "" };
    Piece {
        tex: format!(
            "{nucleus}{}{}{}",
            pre.render(),
            brace_base(&base),
            post.render()
        ),
        grouped: pre.present,
        braces_as_base: pre.present || post.present,
        nucleus: !pre.present,
        ..Piece::default()
    }
}

/// The accumulated scripts on one side of a multiscript base: every subscript concatenated and every
/// superscript concatenated, tracking whether the side carried any slot at all.
#[derive(Default)]
struct ScriptSide {
    present: bool,
    sub: String,
    sup: String,
}

impl ScriptSide {
    fn push(&mut self, sub: &str, sup: &str) {
        self.present = true;
        self.sub.push_str(sub);
        self.sup.push_str(sup);
    }

    /// The `_{sub}^{sup}` group for a side that carried scripts, or nothing for a side with none.
    fn render(&self) -> String {
        if self.present {
            format!("_{{{}}}^{{{}}}", self.sub, self.sup)
        } else {
            String::new()
        }
    }
}

/// A single multiscript slot: an explicit empty (`<none/>`) contributes nothing, anything else its
/// rendered form.
fn script_token<T: MathTree>(e: &T, ctx: Context<'_>) -> String {
    if e.tag() == "none" {
        String::new()
    } else {
        trim_row(&render(e, ctx).tex)
    }
}

/// `<menclose>`: content wrapped in the TeX command its `notation` denotes (a boxed frame or a
/// cancel line), or left bare for a notation with no TeX equivalent.
fn render_menclose<T: MathTree>(e: &T, ctx: Context<'_>) -> String {
    let inner = render_row(&e.element_children(), ctx);
    match enclose_command(&e.attribute("notation").unwrap_or_default()) {
        Some(command) => format!("{command}{{{inner}}}"),
        None => inner,
    }
}

/// The TeX command an `menclose` notation set maps to: diagonal strikes become cancels (up, down, or
/// both crossed), a box becomes `\boxed`, and anything else has no command.
fn enclose_command(notation: &str) -> Option<&'static str> {
    let up = notation
        .split_whitespace()
        .any(|token| token == "updiagonalstrike");
    let down = notation
        .split_whitespace()
        .any(|token| token == "downdiagonalstrike");
    match (up, down) {
        (true, true) => Some("\\xcancel"),
        (true, false) => Some("\\cancel"),
        (false, true) => Some("\\bcancel"),
        (false, false) => notation
            .split_whitespace()
            .any(|token| token == "box")
            .then_some("\\boxed"),
    }
}

/// `<semantics>`: render the presentation child, dropping any annotation payload.
fn render_semantics<T: MathTree>(e: &T, ctx: Context<'_>) -> Piece {
    for element in e.element_children() {
        if element.tag() == "annotation" || element.tag() == "annotation-xml" {
            continue;
        }
        return render(element, ctx);
    }
    Piece::default()
}

/// `<ms>`: a string literal set in a text box between quotation marks. The `lquote` and `rquote`
/// attributes supply the marks, defaulting to typographic double quotes, and the literal text has
/// its LaTeX specials escaped.
fn render_string<T: MathTree>(e: &T, ctx: Context<'_>) -> String {
    let open = e
        .attribute("lquote")
        .unwrap_or_else(|| "\u{201c}".to_string());
    let close = e
        .attribute("rquote")
        .unwrap_or_else(|| "\u{201d}".to_string());
    in_text_face(
        ctx.variant,
        &format!(
            "{open}{}{close}",
            escape_text(e.inner_text().trim(), ctx.faces)
        ),
    )
}

/// Escape text bound for a TeX text box (`\text{...}`): the characters LaTeX reads as control syntax
/// take their text-mode escapes. The three that expand to a control word are held apart from a
/// following letter or digit so the command does not absorb it.
fn escape_text(text: &str, faces: Faces) -> String {
    let mut out = String::new();
    for ch in text.chars().map(|character| unstyled(character, faces)) {
        match ch {
            '%' => out.push_str("\\%"),
            '&' => out.push_str("\\&"),
            '_' => out.push_str("\\_"),
            '#' => out.push_str("\\#"),
            '$' => out.push_str("\\$"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '~' => out.push_str("\\textasciitilde"),
            '^' => out.push_str("\\textasciicircum"),
            '\\' => out.push_str("\\textbackslash"),
            other => {
                if other.is_ascii_alphanumeric() && ends_with_control_word(&out) {
                    out.push(' ');
                }
                out.push(other);
            }
        }
    }
    out
}

/// A script base, braced where it would otherwise read as more than one token or already carries a
/// part of its own where the script has to go.
fn brace_base(base: &Piece) -> String {
    let content = trim_row(&base.tex);
    if base.grouped || base.braces_as_base {
        format!("{{{content}}}")
    } else if base.spaced {
        // A script closes the gap after the operator it attaches to, leaving the one before it.
        format!(" {content}")
    } else {
        content
    }
}

/// `<mi>`: an identifier. A known function name takes its control word; anything else maps character
/// by character, and reads as a group unless it comes to a single plain token.
fn render_identifier(ident: &str, face: Option<&str>, ctx: Context<'_>) -> Piece {
    if ident.is_empty() {
        return Piece::default();
    }
    if is_function(ident) {
        return Piece {
            limits: true,
            ..Piece::construct(in_face(face, &format!("\\{ident}")))
        };
    }
    let styled = ident.chars().count() > 1
        || ident
            .chars()
            .next()
            .is_some_and(|character| character_tex(character, ctx.faces).1 == Role::Styled);
    let tex = map_characters(ident, ctx.faces);
    Piece {
        tex: in_face(face, &tex),
        grouped: styled && face.is_none(),
        simple: true,
        ..Piece::default()
    }
}

/// The TeX form of a run of literal characters, each mapped to its symbol and held apart from the
/// one after it where TeX needs the separation.
fn map_characters(text: &str, faces: Faces) -> String {
    let mut tokens = Tokens::default();
    for character in text.chars() {
        let (tex, role) = character_tex(character, faces);
        tokens.push(&tex, join_after(role));
    }
    tokens.out
}

/// How a token of the given reading joins to the token after it: a spacing symbol carries its own
/// boundary.
fn join_after(role: Role) -> Join {
    if role == Role::Tight {
        Join::Tight
    } else {
        Join::Normal
    }
}

/// The TeX form and reading of a single character: its table entry, the face command a styled letter
/// of the mathematical alphanumeric plane stands for, or the character itself. A character whose face
/// is one of those already written comes down to the letter it is set from.
fn character_tex(character: char, faces: Faces) -> (String, Role) {
    if let Some((tex, role)) = symbols::symbol(character) {
        return match within_face(tex, faces) {
            Some(base) => character_tex(base, faces),
            None => (tex.to_string(), role),
        };
    }
    if let Some((face, base)) = symbols::styled_letter(character) {
        let (tex, role) = character_tex(base, faces);
        return match face {
            Some(face) if !faces.holds(face) => (format!("{face}{{{tex}}}"), Role::Styled),
            Some(_) => (tex, role),
            None => (tex, Role::Plain),
        };
    }
    (character.to_string(), Role::Plain)
}

/// The single character a face command in `tex` is written around, where that face is already
/// written and so leaves the character to stand on its own.
fn within_face(tex: &str, faces: Faces) -> Option<char> {
    let (command, rest) = tex.split_once('{')?;
    if !faces.holds(command) {
        return None;
    }
    let mut inner = rest.strip_suffix('}')?.chars();
    let base = inner.next()?;
    inner.next().is_none().then_some(base)
}

/// The letter a character already set in one of the faces written around it comes down to, or the
/// character as it stands.
fn unstyled(character: char, faces: Faces) -> char {
    if let Some((tex, _)) = symbols::symbol(character) {
        return within_face(tex, faces).unwrap_or(character);
    }
    match symbols::styled_letter(character) {
        Some((Some(face), base)) if faces.holds(face) => base,
        _ => character,
    }
}

/// The function names that take a control word of their own.
fn is_function(name: &str) -> bool {
    matches!(
        name,
        "sin"
            | "cos"
            | "tan"
            | "cot"
            | "sec"
            | "csc"
            | "sinh"
            | "cosh"
            | "tanh"
            | "coth"
            | "arcsin"
            | "arccos"
            | "arctan"
            | "log"
            | "ln"
            | "lg"
            | "exp"
            | "lim"
            | "limsup"
            | "liminf"
            | "max"
            | "min"
            | "sup"
            | "inf"
            | "det"
            | "dim"
            | "gcd"
            | "hom"
            | "ker"
            | "arg"
            | "deg"
            | "Pr"
    )
}

/// Map an accent character to its TeX accent command, or `None` when the overscript is not a
/// recognized accent and should be stacked over the base generically instead. Only characters with a
/// dedicated accent command appear here; near-miss glyphs (a plain ASCII tilde, a period, a
/// right-arrow operator) fall through to the generic `\overset` stacking.
fn accent_commands(accent: &str) -> Option<&'static [&'static str]> {
    Some(match accent {
        "^" => &["\\hat"],
        "\u{2c6}" | "\u{302}" => &["\\widehat"],
        "\u{2dc}" | "\u{303}" => &["\\widetilde"],
        "\u{2c7}" | "\u{30c}" => &["\\check"],
        "\u{b4}" | "\u{301}" => &["\\acute"],
        "\u{60}" | "\u{300}" => &["\\grave"],
        "\u{306}" | "\u{2d8}" => &["\\breve"],
        "\u{307}" => &["\\dot"],
        "\u{308}" => &["\\ddot"],
        "\u{20db}" => &["\\dddot"],
        "\u{20dc}" => &["\\ddddot"],
        "\u{30a}" => &["\\mathring"],
        "\u{af}" | "\u{305}" => &["\\overline"],
        "\u{33f}" => &["\\overline", "\\overline"],
        "\u{304}" | "\u{203e}" => &["\\bar"],
        "\u{20d7}" => &["\\overrightarrow"],
        "\u{20d6}" => &["\\overleftarrow"],
        "\u{23de}" => &["\\overbrace"],
        "\u{23b4}" => &["\\overbracket"],
        _ => return None,
    })
}

/// The four Unicode invisible operators (function application, invisible times, and separators)
/// carry no printed form.
fn is_invisible(op: &str) -> bool {
    matches!(op, "\u{2061}" | "\u{2062}" | "\u{2063}" | "\u{2064}")
}

/// `<mo>`: an operator. An invisible operator vanishes; a single character takes its symbol, spaced
/// from its operands where it reads as a binary or relation sign; a known function name takes its
/// control word; and any other run of characters is set as an operator name.
fn render_operator(op: &str, faces: Faces) -> Piece {
    if is_invisible(op) {
        return Piece::token(String::new());
    }
    let mut characters = op.chars();
    if let Some(character) = characters.next()
        && characters.next().is_none()
    {
        let (tex, role) = character_tex(character, faces);
        return Piece {
            tex,
            spaced: role == Role::Spaced,
            join: join_after(role),
            simple: role != Role::Infix,
            limits: takes_limits(character),
            ..Piece::default()
        };
    }
    if let Some((tex, role)) = symbols::operator(op) {
        return Piece {
            tex: tex.to_string(),
            spaced: role == Role::Spaced,
            simple: true,
            ..Piece::default()
        };
    }
    // A named operator stacks its scripts above and below itself, as the large operators do.
    if is_function(op) {
        return Piece {
            limits: true,
            ..Piece::construct(format!("\\{op}"))
        };
    }
    Piece {
        limits: true,
        ..Piece::construct(format!("\\operatorname{{{}}}", map_characters(op, faces)))
    }
}

/// `<mspace>`: a horizontal gap rendered as the TeX spacing command its `width` selects. A named
/// math-space keyword or an `em` length is honored; a width in any other unit yields no command.
fn render_space<T: MathTree>(e: &T) -> Piece {
    let mu = e.attribute("width").and_then(|w| space_mu(&w)).unwrap_or(0);
    Piece {
        join: Join::Always,
        ..Piece::token(space_command(mu))
    }
}

/// The width of an `<mspace>` in math units: a named math space, or an `em` length scaled at eighteen
/// mu to the em with ties rounded to even. `None` for a width given in any other form.
fn space_mu(width: &str) -> Option<i32> {
    if let Some(mu) = named_space_mu(width) {
        return Some(mu);
    }
    let em = width.strip_suffix("em")?;
    if em.starts_with('+') {
        return None;
    }
    let value: f64 = em.parse().ok()?;
    if !value.is_finite() {
        return None;
    }
    // The measure is finite; the saturating cast bounds an extreme scaled value into `i32`.
    #[allow(clippy::cast_possible_truncation)]
    let mu = (value * 18.0).round_ties_even() as i32;
    Some(mu)
}

/// The math-unit width of a named MathML space keyword, thin through very-very-thick and their
/// negatives, each one mu apart.
fn named_space_mu(name: &str) -> Option<i32> {
    Some(match name {
        "veryverythinmathspace" => 1,
        "verythinmathspace" => 2,
        "thinmathspace" => 3,
        "mediummathspace" => 4,
        "thickmathspace" => 5,
        "verythickmathspace" => 6,
        "veryverythickmathspace" => 7,
        "negativeveryverythinmathspace" => -1,
        "negativeverythinmathspace" => -2,
        "negativethinmathspace" => -3,
        "negativemediummathspace" => -4,
        "negativethickmathspace" => -5,
        "negativeverythickmathspace" => -6,
        "negativeveryverythickmathspace" => -7,
        _ => return None,
    })
}

/// The TeX spacing command for a width in math units: the short control-symbol spaces where one
/// exists, `\quad`/`\qquad` at the em and double-em, an empty command at zero width, and an explicit
/// `\mspace` for every other amount. Four mu is the control space, whose own trailing space both
/// separates it and keeps it from reading as a bare backslash.
fn space_command(mu: i32) -> String {
    match mu {
        0 => String::new(),
        3 => "\\,".to_string(),
        4 => "\\ ".to_string(),
        5 => "\\;".to_string(),
        -3 => "\\!".to_string(),
        18 => "\\quad".to_string(),
        36 => "\\qquad".to_string(),
        other => format!("\\mspace{{{other}mu}}"),
    }
}

#[cfg(test)]
mod tests;
