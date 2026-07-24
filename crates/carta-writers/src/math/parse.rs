//! A small, bounded TeX-math parser. It tokenizes the source and builds a flat list of [`Atom`]s,
//! each optionally carrying a subscript and a superscript group. Both conversion backends consume
//! this single representation.
//!
//! The parser never panics and never recurses unboundedly: brace groups are parsed with an
//! explicit depth limit so pathological nesting (`{{{{…}}}}`) returns `None` rather than
//! overflowing the stack.

/// Maximum brace-group nesting depth before the parser gives up and reports the input as
/// unconvertible.
const MAX_DEPTH: usize = 64;

mod commands;
mod environments;
mod lexer;
mod scripts;
mod text;

use commands::{
    binom_kind, is_style_switch, not_over_number, parse_command, parse_required_group,
    skip_balanced_group,
};
use environments::parse_environment;
use lexer::{expand_macros, tokenize};
use scripts::{ScriptChain, attach_prime, attach_script, prime_detaches};

/// One lexical token of TeX math source.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Token {
    /// A control sequence without its leading backslash (e.g. `alpha`, `frac`).
    Command(String),
    /// A single ordinary character (letter, operator, …).
    Char(char),
    /// A maximal run of adjacent digits, optionally with one interior decimal point between two
    /// digits (`12`, `3.14`). Lexed as one token so it forms a single numeric atom.
    Number(String),
    Sub,
    Sup,
    GroupOpen,
    GroupClose,
    /// A run of one or more source spaces, which TeX collapses.
    Space,
}

/// The body of an atom: what it actually is.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Body {
    /// A control sequence (its name, without backslash).
    Command(String),
    /// A single ordinary character.
    Char(char),
    /// The relation digraph `:=` (a colon immediately followed by an equals, with no space between
    /// them in the source). The two characters form one relation atom that prints as the literal
    /// `:=`, binding tightly inside while taking relation spacing against its neighbours. A spaced
    /// `: =` stays two separate atoms instead.
    ColonEq,
    /// A run of adjacent digits forming one numeric literal, optionally with an embedded decimal
    /// point (`12`, `3.14`). The whole run is one atom so a style applies to it as a unit.
    Number(String),
    /// A synthesized empty nucleus: a script written with no preceding base, e.g. `_x` or `^2`.
    Empty,
    /// A run of one or more prime marks (`'`). Primes flow through the script chain like a
    /// superscript, so their place among other scripts is preserved (`a^2'` nests the prime inside
    /// the `2`, `a'^2` keeps them as siblings); the count records how many marks were written.
    Prime(u8),
    /// An explicitly written empty group `{}`, which occupies an atom slot for spacing and can
    /// carry scripts.
    EmptyGroup,
    /// A braced group of atoms.
    Group(Vec<Atom>),
    /// An accent command applied to a base group, e.g. `\bar{x}`.
    Accent(String, Vec<Atom>),
    /// A styled-alphabet/operator command applied to a single brace group, e.g. `\mathbb{R}`.
    Styled(String, Vec<Atom>),
    /// `\frac{..}{..}` with its bar style, numerator and denominator. `\frac`/`\dfrac` draw a
    /// horizontal bar; `\tfrac` sets the two arguments on one line without a bar.
    Frac(FracStyle, Vec<Atom>, Vec<Atom>),
    /// `\sqrt{..}` or `\sqrt[n]{..}` with an optional index.
    Sqrt(Option<Vec<Atom>>, Vec<Atom>),
    /// A text-mode command (`\text{..}`, `\operatorname{..}`, …) and its content. The content is a
    /// sequence of literal runs and inter-run spacing, since a math spacing inside the wrapper breaks
    /// the run and applies the wrapper's formatting to each side independently.
    Text(String, Vec<TextPiece>),
    /// `\binom{n}{k}` (or the infix `\choose`/`\brace`/`\brack`, or `\genfrac`) with its upper and
    /// lower arguments. The kind selects the surrounding bracket.
    Binom(BinomKind, Vec<Atom>, Vec<Atom>),
    /// A matrix environment: the delimiter kind and the grid of cells (rows of cell atom lists).
    Matrix(MatrixDelim, Vec<Vec<Vec<Atom>>>),
    /// A `\left<delim> … \right<delim>` group: the opening and closing delimiters (each absent when
    /// written as `.`) and the enclosed run.
    Delimited(Option<Delim>, Option<Delim>, Vec<Atom>),
    /// A `\middle<delim>` divider inside a `\left … \right` group: the delimiter (absent when
    /// written as `.`) and whether the opening-side glyph applies (a one-sided delimiter such as a
    /// paren takes its left form when written as `\middle(`, its right form as `\middle)`; a
    /// symmetric bar carries no side).
    Middle(Option<Delim>, bool),
    /// A modulo operator. The kind selects the spacing and bracketing; the optional argument is the
    /// bracketed modulus for the parenthesised forms.
    Mod(ModKind, Option<Vec<Atom>>),
    /// A `\not`-negated relation: the name of the relation being struck through (a command name, or a
    /// single character written as a one-character string for the literal forms `\not=`/`\not<`).
    Negated(String),
    /// A `\not` over a braced group (`\not{a}`, `\not{\alpha}`): the lowered content carries a
    /// combining long solidus. Two-dimensional Typst output overlays the strike on the whole group;
    /// linear output has no place to put it and falls back to verbatim.
    NegatedGroup(Vec<Atom>),
    /// A fixed-size delimiter (`\big(`, `\Bigl[`, …): the percentage the delimiter is scaled to and
    /// the bare delimiter atom. Inline output renders just the delimiter; Typst scales it.
    Big(u16, Box<Atom>),
    /// A horizontal brace spanning a group: `\overbrace{..}` (above) or `\underbrace{..}` (below).
    /// A label written as the atom's matching script (a superscript for an over-brace, a subscript
    /// for an under-brace) annotates the brace.
    Brace(BraceKind, Vec<Atom>),
    /// A two-dimensional stack (`\overset{mark}{base}`, `\underset`, `\stackrel`): the mark is set
    /// over (or under) the base. Only Typst output can place it; linear output falls back to verbatim.
    Stack(StackSide, Vec<Atom>, Vec<Atom>),
    /// An aligned/grid environment (`aligned`, `cases`, `substack`, …): the kind selects the
    /// rendering, the per-column justification an `array` block declares in its column specification
    /// (empty for every other kind, which fixes its own column layout), and the grid of rows of cell
    /// atom lists.
    Grid(GridKind, Vec<ColumnAlign>, Vec<Vec<Vec<Atom>>>),
    /// An extensible arrow (`\xrightarrow`, `\xleftarrow`): the base arrow name, an optional label
    /// set below the arrow, and the label set above it. Only Typst output renders it.
    ExtArrow(&'static str, Option<Vec<Atom>>, Vec<Atom>),
    /// An equation `\label{name}`, carrying the verbatim label name. It has no visible glyph: linear
    /// output drops it entirely, and Typst output lifts the first such label out of the body to a
    /// trailing reference label after the closing `$`. An empty name carries no label at all.
    Label(String),
}

/// Whether a stack places its mark above or below the base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StackSide {
    Over,
    Under,
}

/// How a fraction sets its numerator and denominator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FracStyle {
    /// `\frac`/`\dfrac`: a horizontal bar separates the two arguments.
    Bar,
    /// `\tfrac`: the two arguments sit on one line with no bar.
    Linear,
}

/// The flavour of an aligned/grid environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GridKind {
    /// An `aligned`/`align`/`split`/`alignat` block: cells joined by ` & `, rows by a trailing `\`
    /// and a line break.
    Aligned,
    /// An `array` block: a matrix of centered columns. Its column specification does not affect the
    /// cells.
    Array,
    /// A `cases` block: rendered as Typst's `cases(..)`.
    Cases,
    /// A `\substack{..}`: rows stacked, one per line.
    Substack,
    /// A `gathered`/`gather`/`smallmatrix`/`multline` block: rows stacked and centered.
    Gathered,
    /// An `eqnarray` block: columns cycle right, center, left so each alignment marker meets a
    /// column boundary.
    Eqnarray,
    /// A `flalign` block: columns cycle left, right for a flush-both-sides layout.
    Flalign,
}

/// One column's horizontal justification, as an `array` block declares it in its `{lcr}`-style
/// column specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ColumnAlign {
    Left,
    Center,
    Right,
}

/// The bracket a binomial-style stack is wrapped in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BinomKind {
    /// `\binom`/`\choose`/`\genfrac`: a parenthesised stack (Typst `binom`).
    Paren,
    /// `\brace`: a brace-bracketed stack.
    Brace,
    /// `\brack`: a square-bracketed stack.
    Brack,
}

/// Which side a horizontal brace spans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BraceKind {
    Over,
    Under,
}

/// The flavour of a modulo operator, selecting its spacing and bracketing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ModKind {
    /// `\bmod`: the bare `mod` operator with relation spacing on both sides.
    Bmod,
    /// `\pmod{..}`: a parenthesised `(mod ..)` trailing form.
    Pmod,
    /// `\mod ..`: the `mod` operator that leads its following operand, which stays a separate atom.
    Mod,
    /// `\pod{..}`: a parenthesised `(..)` trailing form without the `mod` word.
    Pod,
}

/// A stretchable delimiter that can open or close a `\left … \right` group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Delim {
    Paren,
    Bracket,
    Brace,
    /// A single vertical bar `|` (a stretchy `\lvert`/`\rvert` or literal `|`).
    Bar,
    /// A double bar that prints as the parallel sign `∥` (U+2225): `\|`, or `\lVert`/`\rVert` used as
    /// an explicit `\left`/`\right` delimiter.
    BarVert,
    /// A stretchy double vertical line `‖` (U+2016): `\Vert`, or a balanced bare `\lVert … \rVert`
    /// pair, used as a `\left`/`\right`-style delimiter.
    DoubleBar,
    Angle,
    Floor,
    Ceil,
    /// The upper-left quine corner `⌜` (U+231C): `\ulcorner`. The glyph is fixed to its side
    /// regardless of which delimiter slot it fills.
    CornerUpperLeft,
    /// The upper-right quine corner `⌝` (U+231D): `\urcorner`.
    CornerUpperRight,
}

/// A segment of text-wrapper content: a literal character run, a math spacing that breaks the run, or
/// a `$…$` math sub-expression.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum TextPiece {
    /// A run of literal characters (the unescaped literals, an escaped or literal space, a `~`).
    Run(String),
    /// A math spacing command, which separates the runs on either side.
    Space(TextSpace),
    /// A `$…$` math sub-expression: the wrapper switches back to math mode for its content, which
    /// renders as math (unaffected by the wrapper's formatting) and splices in among the runs.
    Math(Vec<Atom>),
}

/// How a text-mode group treats whitespace and spacing. A text wrapper (`\text`, `\textbf`, …) keeps
/// literal spaces and splits its run at an explicit spacing; an operator-name group (`\operatorname`)
/// is math content set upright: inter-token spaces drop, an escaped space and `~` become a
/// non-breaking space, and an explicit spacing folds into the run as its codepoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TextMode {
    Wrapper,
    Math,
}

/// A math spacing written inside a text wrapper. Each width has an inline-tree codepoint and a Typst
/// spacing token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TextSpace {
    /// `\,`: a thin space (U+2006 / Typst `thin`).
    Thin,
    /// `\;`: a thick space (U+2005 / Typst `#h(0em)`).
    Thick,
    /// `\:`: a medium space (U+00A0 / Typst `med`).
    Medium,
    /// `\!`: a negative thin space (U+200A / Typst `#h(-1em)`).
    NegThin,
}

impl TextSpace {
    /// The character the inline-tree backend writes between the two runs.
    pub(super) fn codepoint(self) -> char {
        match self {
            TextSpace::Thin => '\u{2006}',
            TextSpace::Thick => '\u{2005}',
            TextSpace::Medium => '\u{00A0}',
            TextSpace::NegThin => '\u{200A}',
        }
    }

    /// The Typst spacing token the Typst backend writes between the two runs.
    pub(super) fn typst_token(self) -> &'static str {
        match self {
            TextSpace::Thin => "thin",
            TextSpace::Thick => "#h(0em)",
            TextSpace::Medium => "med",
            TextSpace::NegThin => "#h(-1em)",
        }
    }
}

/// The bracketing of a matrix environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MatrixDelim {
    None,
    Paren,
    Bracket,
    Brace,
    Bar,
    DoubleBar,
}

/// A math atom: a nucleus plus an optional subscript and superscript group, sibling scripts kept in
/// source order, and an explicit limit-placement override from `\limits`/`\nolimits`.
///
/// The `sub`/`sup` slots hold the first subscript and superscript. A base may carry further
/// *sibling* scripts of the same kind when a script lands on an already-filled slot across a
/// boundary that does not nest (e.g. the outer script of `{x^2}^3`); those go in [`siblings`],
/// preserving source order. Prime marks flow through the same chain as superscripts.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Atom {
    pub body: Body,
    pub sub: Option<Vec<Atom>>,
    pub sup: Option<Vec<Atom>>,
    /// Additional scripts beyond the first sub/sup, kept in source order. Empty for the common
    /// single-script case.
    pub siblings: Vec<Sibling>,
    /// `\limits` (`Some(true)`, scripts stacked above/below) or `\nolimits` (`Some(false)`, scripts
    /// beside) when the source forces a placement; `None` for the operator's default.
    pub limits: Option<bool>,
}

/// Whether a script sits below (a subscript) or above (a superscript) its base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScriptKind {
    Sub,
    Sup,
}

/// One script written after the base's primary sub/superscript slot was unavailable, kept in source
/// order. `sealed` marks a sibling created across a group boundary (`{x^2}^3`): such scripts pack
/// directly onto the rendered base, whereas a sibling formed in a flat chain (`x^2_3^4`) starts a
/// fresh base in formats that bind one script of each kind per atom.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Sibling {
    pub kind: ScriptKind,
    pub atoms: Vec<Atom>,
    pub sealed: bool,
}

/// One script in a normalized render run: its kind and the atoms it carries. The lifetime borrows
/// the atom whose scripts are being sequenced.
pub(super) struct RunScript<'a> {
    pub kind: ScriptKind,
    pub atoms: &'a [Atom],
}

/// One render run of an atom's scripts: a maximal group in which each kind appears at most once,
/// reordered so the subscript renders before the superscript (the way a paired sub/superscript
/// stacks). `restart` marks a run that begins a fresh base in formats that bind one script of each
/// kind per atom (a flat-chain sibling group); a sealed run packs directly onto the preceding base.
pub(super) struct ScriptRun<'a> {
    pub scripts: Vec<RunScript<'a>>,
    pub restart: bool,
}

impl Atom {
    fn new(body: Body) -> Self {
        Atom {
            body,
            sub: None,
            sup: None,
            siblings: Vec::new(),
            limits: None,
        }
    }

    /// Sequence this atom's scripts into render runs. The primary subscript and superscript form the
    /// first run (subscript before superscript); the sibling scripts split into further runs at each
    /// point a kind repeats, each run reordered subscript-before-superscript. A flat-chain (unsealed)
    /// sibling run restarts on a fresh base, but two consecutive such runs share one restart base, so
    /// only every other unsealed run is flagged `restart`; a sealed run never restarts.
    #[allow(clippy::similar_names)]
    pub(super) fn script_runs(&self) -> Vec<ScriptRun<'_>> {
        let mut runs: Vec<ScriptRun<'_>> = Vec::new();
        let mut primary = Vec::new();
        if let Some(sub) = self.sub.as_deref() {
            primary.push(RunScript {
                kind: ScriptKind::Sub,
                atoms: sub,
            });
        }
        if let Some(sup) = self.sup.as_deref() {
            primary.push(RunScript {
                kind: ScriptKind::Sup,
                atoms: sup,
            });
        }
        if !primary.is_empty() {
            runs.push(ScriptRun {
                scripts: primary,
                restart: false,
            });
        }
        let mut unsealed_run_index = 0usize;
        let mut i = 0;
        while i < self.siblings.len() {
            let mut has_sub = false;
            let mut has_sup = false;
            let start = i;
            let sealed = self.siblings.get(start).is_some_and(|s| s.sealed);
            while let Some(sib) = self.siblings.get(i) {
                let seen = match sib.kind {
                    ScriptKind::Sub => has_sub,
                    ScriptKind::Sup => has_sup,
                };
                if seen {
                    break;
                }
                match sib.kind {
                    ScriptKind::Sub => has_sub = true,
                    ScriptKind::Sup => has_sup = true,
                }
                i += 1;
            }
            let mut scripts = Vec::new();
            for sib in self.siblings.get(start..i).into_iter().flatten() {
                if sib.kind == ScriptKind::Sub {
                    scripts.push(RunScript {
                        kind: ScriptKind::Sub,
                        atoms: &sib.atoms,
                    });
                }
            }
            for sib in self.siblings.get(start..i).into_iter().flatten() {
                if sib.kind == ScriptKind::Sup {
                    scripts.push(RunScript {
                        kind: ScriptKind::Sup,
                        atoms: &sib.atoms,
                    });
                }
            }
            let restart = if sealed {
                false
            } else {
                let first = unsealed_run_index.is_multiple_of(2);
                unsealed_run_index += 1;
                first
            };
            runs.push(ScriptRun { scripts, restart });
        }
        runs
    }

    /// Whether this atom carries any script or prime mark, used to seal a group whose last atom is
    /// already scripted.
    fn is_scripted(&self) -> bool {
        self.sub.is_some() || self.sup.is_some() || !self.siblings.is_empty()
    }

    /// Whether this atom is a bare prime mark. Nothing nests into a prime, so a script landing on a
    /// prime active siblings its base instead.
    fn is_prime(&self) -> bool {
        matches!(self.body, Body::Prime(_))
    }
}

/// Parse TeX math source into a flat atom list, or `None` if it nests too deep or is malformed.
pub(super) fn parse(src: &str) -> Option<Vec<Atom>> {
    let tokens = expand_macros(tokenize(src));
    let mut pos = 0;
    let mut atoms = parse_atoms(&tokens, &mut pos, 0, false)?;
    if pos != tokens.len() {
        return None;
    }
    pair_double_bars(&mut atoms);
    Some(atoms)
}

/// Promote a balanced `\lVert … \rVert` run to a stretchy double-bar delimited group. Written alone
/// each command is the loose parallel sign, but as a matched opening/closing pair around content the
/// run becomes a `\left\Vert … \right\Vert`-style group: a taller double vertical line that stretches
/// to its content. The match is stack-balanced so nested pairs resolve innermost-first, and the pass
/// recurses into every nested atom run first so inner pairs are formed before an enclosing one.
fn pair_double_bars(atoms: &mut Vec<Atom>) {
    for atom in atoms.iter_mut() {
        recurse_pair_double_bars(atom);
    }
    let mut opens: Vec<usize> = Vec::new();
    let mut close: Option<(usize, usize)> = None;
    for index in 0..atoms.len() {
        match atoms.get(index).map(|atom| &atom.body) {
            Some(Body::Command(name)) if name == "lVert" => opens.push(index),
            Some(Body::Command(name)) if name == "rVert" => {
                if let Some(open) = opens.pop() {
                    close = Some((open, index));
                    break;
                }
            }
            _ => {}
        }
    }
    if let Some((open, end)) = close
        && open < end
        && end < atoms.len()
    {
        let inner: Vec<Atom> = atoms.splice(open + 1..end, std::iter::empty()).collect();
        // After the splice the closing command sits directly after the opener.
        let closer = atoms.get(open + 1).cloned();
        atoms.remove(open + 1);
        if let Some(opener) = atoms.get_mut(open) {
            opener.body = Body::Delimited(Some(Delim::DoubleBar), Some(Delim::DoubleBar), inner);
            if let Some(closer) = closer {
                if opener.sub.is_none() {
                    opener.sub = closer.sub;
                }
                if opener.sup.is_none() {
                    opener.sup = closer.sup;
                }
                opener.siblings.extend(closer.siblings);
            }
        }
        pair_double_bars(atoms);
    }
}

/// Apply [`pair_double_bars`] to every atom run nested inside one atom.
fn recurse_pair_double_bars(atom: &mut Atom) {
    if let Some(sub) = atom.sub.as_mut() {
        pair_double_bars(sub);
    }
    if let Some(sup) = atom.sup.as_mut() {
        pair_double_bars(sup);
    }
    match &mut atom.body {
        Body::Group(inner)
        | Body::Accent(_, inner)
        | Body::Styled(_, inner)
        | Body::Delimited(_, _, inner)
        | Body::Brace(_, inner)
        | Body::Mod(_, Some(inner)) => pair_double_bars(inner),
        Body::Frac(_, a, b) | Body::Binom(_, a, b) => {
            pair_double_bars(a);
            pair_double_bars(b);
        }
        Body::Sqrt(index, radicand) => {
            if let Some(index) = index.as_mut() {
                pair_double_bars(index);
            }
            pair_double_bars(radicand);
        }
        Body::Matrix(_, rows) | Body::Grid(_, _, rows) => {
            for row in rows.iter_mut() {
                for cell in row.iter_mut() {
                    pair_double_bars(cell);
                }
            }
        }
        Body::Stack(_, mark, base) => {
            pair_double_bars(mark);
            pair_double_bars(base);
        }
        Body::ExtArrow(_, below, above) => {
            if let Some(below) = below.as_mut() {
                pair_double_bars(below);
            }
            pair_double_bars(above);
        }
        _ => {}
    }
}

/// Parse a run of atoms until end-of-input or (when `in_group`) the matching `}`.
// One flat dispatch over token kinds; splitting would scatter the shared script-chain state.
#[allow(clippy::too_many_lines)]
fn parse_atoms(
    tokens: &[Token],
    pos: &mut usize,
    depth: usize,
    in_group: bool,
) -> Option<Vec<Atom>> {
    if depth > MAX_DEPTH {
        return None;
    }
    let mut atoms: Vec<Atom> = Vec::new();
    // Same-kind scripts nest (`a^b^c`); cross-group or mixed-kind scripts sibling (`{x^2}^3`).
    // Reset whenever a fresh nucleus or group is pushed.
    let mut chain = ScriptChain::default();
    while let Some(tok) = tokens.get(*pos) {
        match tok {
            Token::GroupClose => {
                if in_group {
                    *pos += 1;
                    return Some(atoms);
                }
                return None;
            }
            Token::Space => {
                *pos += 1;
            }
            Token::Sub | Token::Sup => {
                let kind = if matches!(tok, Token::Sup) {
                    ScriptKind::Sup
                } else {
                    ScriptKind::Sub
                };
                *pos += 1;
                let script = parse_script(tokens, pos, depth + 1)?;
                if atoms.is_empty() {
                    atoms.push(Atom::new(Body::Empty));
                    chain = ScriptChain::default();
                }
                let base = atoms.last_mut()?;
                attach_script(base, &mut chain, kind, script)?;
            }
            Token::GroupOpen => {
                // A brace group is transparent: atoms splice in and a following script binds to the
                // last one; an empty group becomes a bare empty nucleus.
                *pos += 1;
                let inner = parse_atoms(tokens, pos, depth + 1, true)?;
                if inner.is_empty() {
                    atoms.push(Atom::new(Body::EmptyGroup));
                } else {
                    atoms.extend(inner);
                }
                // An already-scripted spliced atom is sealed: later scripts sibling in source order.
                let sealed = atoms.last().is_some_and(Atom::is_scripted);
                chain = ScriptChain {
                    sealed,
                    ..ScriptChain::default()
                };
            }
            // An apostrophe is a prime on the preceding atom, flowing through the chain as a
            // superscript; with no preceding atom it is a bare glyph, and consecutive bare primes merge.
            Token::Char('\'') => {
                *pos += 1;
                match atoms.last_mut() {
                    Some(Atom {
                        body: Body::Prime(count),
                        ..
                    }) if chain.last.is_none() => {
                        *count = count.saturating_add(1);
                    }
                    None => {
                        atoms.push(Atom::new(Body::Prime(1)));
                        chain = ScriptChain::default();
                    }
                    // With both primary slots taken TeX detaches the prime: a bare glyph starting a
                    // fresh base (a braced subscript seals its base and nests instead).
                    Some(base) if prime_detaches(base, &chain) => {
                        atoms.push(Atom::new(Body::Prime(1)));
                        chain = ScriptChain::default();
                    }
                    Some(base) => attach_prime(base, &mut chain)?,
                }
            }
            Token::Command(c) if c == "limits" || c == "nolimits" => {
                let forced = c == "limits";
                *pos += 1;
                let last = atoms.last_mut()?;
                last.limits = Some(forced);
            }
            // Numbering annotations have no glyph: drop `\nonumber`/`\tag`, capture `\label` as a
            // `Label` atom; consumed at top level so they never force a verbatim fallback.
            Token::Command(c) if !in_group && c == "nonumber" => {
                *pos += 1;
            }
            Token::Command(c) if !in_group && (c == "tag" || c == "label") => {
                let is_label = c == "label";
                *pos += 1;
                let name = read_label_arg(tokens, pos)?;
                if is_label && let Some(name) = name {
                    atoms.push(Atom::new(Body::Label(name)));
                }
            }
            Token::Command(c) if is_style_switch(c) => {
                *pos += 1;
            }
            // Colour is invisible in linear and Typst output; consume and drop the spec group.
            Token::Command(c) if c == "color" => {
                *pos += 1;
                parse_required_group(tokens, pos, depth, &mut None)?;
            }
            // A preamble declaration with no glyph: consume both groups (the operator is not
            // registered for later use).
            Token::Command(c) if c == "DeclareMathOperator" => {
                *pos += 1;
                if matches!(tokens.get(*pos), Some(Token::Char('*'))) {
                    *pos += 1;
                }
                skip_balanced_group(tokens, pos)?;
                skip_balanced_group(tokens, pos)?;
            }
            // An infix binomial (`a \choose b`) splits its group: atoms so far are the upper
            // argument, the rest the lower; the run collapses to one stacked atom.
            Token::Command(c) if binom_kind(c).is_some() => {
                let kind = binom_kind(c)?;
                *pos += 1;
                let top = std::mem::take(&mut atoms);
                let bottom = parse_atoms(tokens, pos, depth + 1, in_group)?;
                return Some(vec![Atom::new(Body::Binom(kind, top, bottom))]);
            }
            // A transparent single-line equation wrapper splices into the surrounding run; every
            // other environment is a single atom.
            Token::Command(c) if c == "begin" => {
                *pos += 1;
                let spliced = parse_environment(tokens, pos, depth)?;
                let sealed = spliced.last().is_some_and(Atom::is_scripted);
                atoms.extend(spliced);
                chain = ScriptChain {
                    sealed,
                    ..ScriptChain::default()
                };
            }
            // `\not` strikes only the number's first digit; the number is one token, so split the
            // tail into its own atom here.
            Token::Command(c) if c == "not" && not_over_number(tokens, *pos) => {
                *pos += 1;
                skip_spaces(tokens, pos);
                let Some(Token::Number(digits)) = tokens.get(*pos) else {
                    return None;
                };
                let mut chars = digits.chars();
                let first = chars.next()?;
                atoms.push(Atom::new(Body::Negated(first.to_string())));
                let rest: String = chars.collect();
                if !rest.is_empty() {
                    atoms.push(Atom::new(Body::Number(rest)));
                }
                *pos += 1;
                chain = ScriptChain::default();
            }
            _ => {
                parse_atom_into(tokens, pos, depth, &mut atoms)?;
                chain = ScriptChain::default();
            }
        }
    }
    if in_group { None } else { Some(atoms) }
}

/// Parse the argument of a `_`/`^`: a single atom, a braced group, or (when the argument is itself a
/// script operator) a synthesized empty nucleus carrying the nested script.
///
/// A script written directly as another script's argument (`a^^b`, `a___b`) has no base of its own,
/// so it binds to an empty nucleus that then nests the inner script. An empty *implicit* base
/// (`^^`, with no braces) is [`Body::Empty`]; an empty *braced* base (`^{}`) is [`Body::EmptyGroup`],
/// which the backends distinguish (the implicit form prints as the empty-content marker, the braced
/// form as a zero-width-space marker when it carries a script).
fn parse_script(tokens: &[Token], pos: &mut usize, depth: usize) -> Option<Vec<Atom>> {
    if depth > MAX_DEPTH {
        return None;
    }
    match tokens.get(*pos)? {
        Token::GroupOpen => {
            *pos += 1;
            let inner = parse_atoms(tokens, pos, depth + 1, true)?;
            // `^{}` is an explicit empty nucleus: it occupies the slot so a following script can nest.
            if inner.is_empty() {
                return Some(vec![Atom::new(Body::EmptyGroup)]);
            }
            // A braced prime run (`^{'''''}`) is a prime nucleus: the transparent group routes it
            // through the nucleus path so a long run stacks; a direct trailing prime stays flat.
            if let [
                Atom {
                    body: Body::Prime(_),
                    sub: None,
                    sup: None,
                    siblings,
                    limits: None,
                },
            ] = inner.as_slice()
                && siblings.is_empty()
            {
                return Some(vec![Atom::new(Body::Group(inner))]);
            }
            Some(inner)
        }
        Token::Space => {
            *pos += 1;
            parse_script(tokens, pos, depth)
        }
        // A script argument that is itself a script operator (`a^^b`) nests onto an empty implicit nucleus.
        tok @ (Token::Sub | Token::Sup) => {
            let kind = if matches!(tok, Token::Sup) {
                ScriptKind::Sup
            } else {
                ScriptKind::Sub
            };
            *pos += 1;
            let nested = parse_script(tokens, pos, depth + 1)?;
            let mut base = Atom::new(Body::Empty);
            match kind {
                ScriptKind::Sub => base.sub = Some(nested),
                ScriptKind::Sup => base.sup = Some(nested),
            }
            Some(vec![base])
        }
        _ => {
            let mut slot = Vec::new();
            parse_atom_into(tokens, pos, depth, &mut slot)?;
            Some(slot)
        }
    }
}

/// Parse one nucleus atom (a char, a command with any arguments, or a braced group). An unbraced
/// multi-digit argument to a command is a single digit, so a command may leave the remaining digits in
/// `tail` for the caller to place as a following number atom (`\frac12` → `1` over `2`; `\sqrt12` →
/// `\sqrt1` then a loose `2`).
#[allow(clippy::match_same_arms)]
fn parse_atom(
    tokens: &[Token],
    pos: &mut usize,
    depth: usize,
    tail: &mut Option<String>,
) -> Option<Atom> {
    if depth > MAX_DEPTH {
        return None;
    }
    match tokens.get(*pos)? {
        // A dangling bare backslash is not a complete expression; the input stays verbatim.
        Token::Char('\\') => None,
        Token::Char(':') if matches!(tokens.get(*pos + 1), Some(Token::Char('='))) => {
            *pos += 2;
            Some(Atom::new(Body::ColonEq))
        }
        // Bare `"`, backtick, or `$` has no math-mode meaning: unparsable, emitted verbatim (escaped
        // forms tokenize as commands; the text path consumes them inside `\text{…}`).
        Token::Char('"' | '`' | '$') => None,
        Token::Char(c) => {
            let c = *c;
            *pos += 1;
            Some(Atom::new(Body::Char(c)))
        }
        Token::Number(digits) => {
            let digits = digits.clone();
            *pos += 1;
            Some(Atom::new(Body::Number(digits)))
        }
        Token::GroupOpen => {
            *pos += 1;
            let inner = parse_atoms(tokens, pos, depth + 1, true)?;
            Some(Atom::new(Body::Group(inner)))
        }
        Token::Command(name) => {
            let name = name.clone();
            *pos += 1;
            parse_command(&name, tokens, pos, depth, tail)
        }
        Token::Sub | Token::Sup | Token::GroupClose | Token::Space => None,
    }
}

/// Parse one nucleus atom into `out`, appending any digits an unbraced multi-digit command argument
/// left over (see [`parse_atom`]) as a following number atom.
fn parse_atom_into(
    tokens: &[Token],
    pos: &mut usize,
    depth: usize,
    out: &mut Vec<Atom>,
) -> Option<()> {
    let mut tail = None;
    let atom = parse_atom(tokens, pos, depth, &mut tail)?;
    out.push(atom);
    if let Some(rest) = tail {
        out.push(Atom::new(Body::Number(rest)));
    }
    Some(())
}

fn skip_spaces(tokens: &[Token], pos: &mut usize) {
    while matches!(tokens.get(*pos), Some(Token::Space)) {
        *pos += 1;
    }
}

/// Read a `\label{name}` argument as a flat verbatim run, returning its reconstructed source text and
/// advancing past the closing brace. The argument must be a single flat group: a nested `{…}` (e.g.
/// `\label{\sqrt{x}}`) leaves the whole expression unhandled and is reported as `None`. The name keeps
/// its source spelling: a control word is rebuilt with its backslash (`\alpha`), an escaped character
/// keeps the escape, so an inner `$`, command, or space round-trips into the label verbatim. A
/// missing opening brace yields `Some(None)`: the `\label` is consumed but carries no name.
// Nested option is load-bearing: the outer layer drives `?`-propagation of the shared unhandled
// fallback; the inner layer reports whether a name was present.
#[allow(clippy::option_option)]
fn read_label_arg(tokens: &[Token], pos: &mut usize) -> Option<Option<String>> {
    skip_spaces(tokens, pos);
    if matches!(tokens.get(*pos), Some(Token::Char('*'))) {
        *pos += 1;
        skip_spaces(tokens, pos);
    }
    if !matches!(tokens.get(*pos), Some(Token::GroupOpen)) {
        return Some(None);
    }
    let mut probe = *pos + 1;
    let mut name = String::new();
    while let Some(tok) = tokens.get(probe) {
        match tok {
            Token::GroupClose => {
                *pos = probe + 1;
                return Some(if name.is_empty() { None } else { Some(name) });
            }
            // A nested group makes the argument two-dimensional, which has no flat source form here.
            Token::GroupOpen => return None,
            Token::Command(c) => {
                name.push('\\');
                name.push_str(c);
            }
            Token::Char(c) => name.push(*c),
            Token::Number(d) => name.push_str(d),
            Token::Sub => name.push('_'),
            Token::Sup => name.push('^'),
            Token::Space => name.push(' '),
        }
        probe += 1;
    }
    None
}

/// Format a captured label name as its trailing Typst reference token. An identifier-shaped name
/// (letters, digits, and `:._-`) renders as a bare `<name>` label; any other name is quoted as
/// `#label("name")` with backslashes and quotes escaped for the Typst string literal.
pub(super) fn format_label(name: &str) -> String {
    let identifier = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '.' | '_' | '-'));
    if identifier {
        format!("<{name}>")
    } else {
        let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
        format!("#label(\"{escaped}\")")
    }
}
