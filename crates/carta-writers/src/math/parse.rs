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

/// The per-column justifications an `array` column specification declares, in column order. Each of
/// `l`, `c`, `r` (and the paragraph column types `p`, `m`, `b`, which set flush-left) contributes one
/// column; rules, inter-column material, and their braced arguments are skipped.
fn parse_column_aligns(spec: &str) -> Vec<ColumnAlign> {
    let mut aligns = Vec::new();
    let mut brace_depth = 0i32;
    for c in spec.chars() {
        match c {
            '{' => brace_depth += 1,
            '}' => brace_depth -= 1,
            _ if brace_depth > 0 => {}
            'l' | 'p' | 'm' | 'b' => aligns.push(ColumnAlign::Left),
            'c' => aligns.push(ColumnAlign::Center),
            'r' => aligns.push(ColumnAlign::Right),
            _ => {}
        }
    }
    aligns
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
    /// `\,` — a thin space (U+2006 / Typst `thin`).
    Thin,
    /// `\;` — a thick space (U+2005 / Typst `#h(0em)`).
    Thick,
    /// `\:` — a medium space (U+00A0 / Typst `med`).
    Medium,
    /// `\!` — a negative thin space (U+200A / Typst `#h(-1em)`).
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
        // Sibling scripts split into runs at each repeated kind; each run is reordered sub-then-sup.
        // Unsealed runs restart on a fresh base, two per base, so the restart flag alternates.
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

    /// Whether this atom carries any script or prime mark — used to seal a group whose last atom is
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

/// Whether the character one position past the current peek is a digit. Used to decide whether a
/// leading `.` begins a decimal number (`.5`) or is an ordinary punctuation character.
fn peek_is_digit(chars: &std::iter::Peekable<std::str::Chars<'_>>) -> bool {
    let mut lookahead = chars.clone();
    lookahead.next();
    matches!(lookahead.next(), Some(d) if d.is_ascii_digit())
}

/// Consume one numeric literal from the front of `chars`: a run of digits with at most one interior
/// decimal point that is immediately followed by a digit. The cursor is left just past the number.
fn lex_number(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut number = String::new();
    let mut took_dot = false;
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            number.push(c);
            chars.next();
        } else if c == '.' && !took_dot && peek_is_digit(chars) {
            took_dot = true;
            number.push(c);
            chars.next();
        } else {
            break;
        }
    }
    number
}

/// Tokenize TeX math source. Returns `None` only on a malformed control sequence we never accept.
fn tokenize(src: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = src.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            '\\' => {
                chars.next();
                match chars.peek().copied() {
                    Some(next) if next.is_ascii_alphabetic() => {
                        let mut name = String::new();
                        while let Some(&a) = chars.peek() {
                            if a.is_ascii_alphabetic() {
                                name.push(a);
                                chars.next();
                            } else {
                                break;
                            }
                        }
                        tokens.push(Token::Command(name));
                    }
                    Some(sym) => {
                        chars.next();
                        tokens.push(Token::Command(sym.to_string()));
                    }
                    None => tokens.push(Token::Char('\\')),
                }
            }
            '^' => {
                chars.next();
                tokens.push(Token::Sup);
            }
            '_' => {
                chars.next();
                tokens.push(Token::Sub);
            }
            '{' => {
                chars.next();
                tokens.push(Token::GroupOpen);
            }
            '}' => {
                chars.next();
                tokens.push(Token::GroupClose);
            }
            c if c.is_whitespace() => {
                chars.next();
                while let Some(&w) = chars.peek() {
                    if w.is_whitespace() {
                        chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push(Token::Space);
            }
            // A numeric literal: a maximal run of digits with at most one interior decimal point
            // that is itself flanked by digits. A leading `.` is taken only when a digit follows.
            c if c.is_ascii_digit() || (c == '.' && peek_is_digit(&chars)) => {
                let number = lex_number(&mut chars);
                tokens.push(Token::Number(number));
            }
            _ => {
                chars.next();
                tokens.push(Token::Char(c));
            }
        }
    }
    tokens
}

/// The most user-macro expansions performed for one expression, and the most tokens the expanded
/// stream may hold. Both bound a recursive definition (`\renewcommand{\a}{\a\a}`) so expansion always
/// halts; once either ceiling is reached, remaining uses stay unexpanded and fall back to verbatim.
const MACRO_EXPANSION_BUDGET: usize = 4096;
const MACRO_EXPANSION_MAX_TOKENS: usize = 65_536;

/// One `\newcommand`/`\renewcommand` definition: its mandatory-argument count and its replacement
/// body, in which each `#n` placeholder is recorded as a parameter reference.
struct Macro {
    params: usize,
    body: Vec<BodyPiece>,
}

/// One element of a macro's replacement body: either a literal token or a reference to the nth
/// argument the use supplies.
enum BodyPiece {
    Literal(Token),
    Param(usize),
}

/// Read a balanced brace group's inner tokens, assuming the cursor sits on its opening `{` and
/// advancing past the matching `}`. Returns `None` if the group never closes.
fn read_group(tokens: &[Token], pos: &mut usize) -> Option<Vec<Token>> {
    if !matches!(tokens.get(*pos), Some(Token::GroupOpen)) {
        return None;
    }
    *pos += 1;
    let mut depth = 1usize;
    let mut inner = Vec::new();
    while let Some(tok) = tokens.get(*pos) {
        match tok {
            Token::GroupOpen => depth += 1,
            Token::GroupClose => {
                depth -= 1;
                if depth == 0 {
                    *pos += 1;
                    return Some(inner);
                }
            }
            _ => {}
        }
        inner.push(tok.clone());
        *pos += 1;
    }
    None
}

/// Compile a macro's raw body tokens into replacement pieces, turning each `#n` (with `n` a valid
/// parameter index) into a parameter reference and leaving every other token literal.
fn compile_macro_body(tokens: &[Token], params: usize) -> Vec<BodyPiece> {
    let mut body = Vec::new();
    let mut index = 0;
    while let Some(tok) = tokens.get(index) {
        if matches!(tok, Token::Char('#'))
            && let Some(Token::Number(digits)) = tokens.get(index + 1)
            && let Some(first) = digits.chars().next()
            && let Some(reference) = first.to_digit(10)
            && (1..=params).contains(&(reference as usize))
        {
            body.push(BodyPiece::Param(reference as usize));
            let rest: String = digits.chars().skip(1).collect();
            if !rest.is_empty() {
                body.push(BodyPiece::Literal(Token::Number(rest)));
            }
            index += 2;
            continue;
        }
        body.push(BodyPiece::Literal(tok.clone()));
        index += 1;
    }
    body
}

/// Parse one `\newcommand`/`\renewcommand` definition beginning at `start` (the control-word token).
/// On success returns the macro name, its compiled form, and the position just past the definition;
/// `None` for any shape outside the supported form (a braced or bare name, an optional `[N]`
/// argument count with no default, and a braced body), leaving the caller to treat the token
/// literally.
fn parse_macro_definition(tokens: &[Token], start: usize) -> Option<(String, Macro, usize)> {
    let mut pos = start + 1;
    skip_spaces(tokens, &mut pos);
    let name = match tokens.get(pos)? {
        Token::Command(name) => {
            pos += 1;
            name.clone()
        }
        Token::GroupOpen => {
            pos += 1;
            let Token::Command(name) = tokens.get(pos)? else {
                return None;
            };
            let name = name.clone();
            pos += 1;
            match tokens.get(pos)? {
                Token::GroupClose => pos += 1,
                _ => return None,
            }
            name
        }
        _ => return None,
    };
    skip_spaces(tokens, &mut pos);
    let mut params = 0usize;
    if matches!(tokens.get(pos), Some(Token::Char('['))) {
        pos += 1;
        let Token::Number(count) = tokens.get(pos)? else {
            return None;
        };
        let count = count.parse::<usize>().ok()?;
        if count > 9 {
            return None;
        }
        pos += 1;
        match tokens.get(pos)? {
            Token::Char(']') => pos += 1,
            _ => return None,
        }
        params = count;
        skip_spaces(tokens, &mut pos);
        // An optional-argument default (`\newcommand{\x}[1][d]{…}`) is a shape this does not model;
        // leaving it unexpanded keeps the whole expression verbatim rather than substituting wrongly.
        if matches!(tokens.get(pos), Some(Token::Char('['))) {
            return None;
        }
    }
    let body_tokens = read_group(tokens, &mut pos)?;
    Some((
        name,
        Macro {
            params,
            body: compile_macro_body(&body_tokens, params),
        },
        pos,
    ))
}

/// Read the `params` arguments a macro use supplies: a braced group contributes its inner tokens, an
/// unbraced token contributes itself. Returns `None` if the stream runs out before every argument is
/// read, so the use is left unexpanded.
fn read_macro_arguments(
    tokens: &[Token],
    pos: &mut usize,
    params: usize,
) -> Option<Vec<Vec<Token>>> {
    let mut arguments = Vec::with_capacity(params);
    for _ in 0..params {
        skip_spaces(tokens, pos);
        match tokens.get(*pos)? {
            Token::GroupOpen => arguments.push(read_group(tokens, pos)?),
            single => {
                arguments.push(vec![single.clone()]);
                *pos += 1;
            }
        }
    }
    Some(arguments)
}

/// Expand `\newcommand`/`\renewcommand` macros in a token stream: collect every definition and drop
/// it, then replace each later use with its body, substituting `#n` placeholders with the supplied
/// arguments. A stream that defines no macro is returned untouched. Expansion is bounded so a
/// self-referential definition halts, leaving any still-unexpanded use to fall back to verbatim.
fn expand_macros(tokens: Vec<Token>) -> Vec<Token> {
    let mut macros: std::collections::BTreeMap<String, Macro> = std::collections::BTreeMap::new();
    let mut stripped = Vec::new();
    let mut pos = 0;
    while let Some(tok) = tokens.get(pos) {
        if let Token::Command(name) = tok
            && (name == "newcommand" || name == "renewcommand")
            && let Some((name, definition, next)) = parse_macro_definition(&tokens, pos)
        {
            macros.insert(name, definition);
            pos = next;
            continue;
        }
        stripped.push(tok.clone());
        pos += 1;
    }
    if macros.is_empty() {
        return tokens;
    }
    let mut current = stripped;
    let mut budget = MACRO_EXPANSION_BUDGET;
    loop {
        let mut expanded = Vec::new();
        let mut changed = false;
        let mut index = 0;
        while let Some(tok) = current.get(index) {
            if budget > 0
                && let Token::Command(name) = tok
                && let Some(definition) = macros.get(name)
            {
                let mut after = index + 1;
                if let Some(arguments) =
                    read_macro_arguments(&current, &mut after, definition.params)
                {
                    for piece in &definition.body {
                        match piece {
                            BodyPiece::Literal(token) => expanded.push(token.clone()),
                            BodyPiece::Param(reference) => {
                                if let Some(argument) = arguments.get(reference - 1) {
                                    expanded.extend(argument.iter().cloned());
                                }
                            }
                        }
                    }
                    index = after;
                    changed = true;
                    budget -= 1;
                    continue;
                }
            }
            expanded.push(tok.clone());
            index += 1;
        }
        current = expanded;
        if !changed || budget == 0 || current.len() > MACRO_EXPANSION_MAX_TOKENS {
            return current;
        }
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
        // After the splice the closing command sits directly after the opener. Take its trailing
        // scripts and primes onto the group, then drop it, and replace the opener with the
        // delimited group carrying the captured content.
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
        // Continue pairing any further runs in the remainder.
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

/// State for binding a chain of scripts to the most recent atom of a run.
///
/// `path` descends from the run's last atom (the base) to the *active* atom, where the next nesting
/// script lands: the first step optionally selects a sibling group of the base, and each remaining
/// step takes the last element of the active atom's subscript or superscript group. `last` is the
/// kind of the script applied most recently, used to decide whether an occupied-slot script nests
/// (consecutive same kind) or siblings. `sealed` is set after a script lands across a group boundary
/// (`{x^2}…`): every later script then siblings in source order instead of filling a primary slot.
#[derive(Default)]
struct ScriptChain {
    /// `Some(i)` when the active atom lives inside the base's `i`th sibling group; the active is the
    /// last atom of that group descended through `steps`.
    sibling: Option<usize>,
    steps: Vec<ScriptKind>,
    last: Option<ScriptKind>,
    sealed: bool,
}

/// Bind a script of `kind` to `base` (the run's last atom), following `chain` to the active atom.
///
/// Scripts written in sequence build a tree. The *active* atom is the last atom of the script
/// content applied most recently; `chain` records the path to it. A script that repeats the previous
/// kind nests one level deeper into the active atom (`a^b^c` → `a^{b^c}`); a script of the other kind
/// applies to the active atom's parent, so a fresh base collects both its scripts (`a^b_c`). Across a
/// group boundary the base is sealed: a non-nesting script siblings the base in source order
/// (`{x^2}_3`) instead of filling a primary slot.
fn attach_script(
    base: &mut Atom,
    chain: &mut ScriptChain,
    kind: ScriptKind,
    script: Vec<Atom>,
) -> Option<()> {
    // The first script of a chain fills the base's free matching slot directly; a sealed base or an
    // occupied slot falls through to a sibling.
    if chain.last.is_none() && !chain.sealed {
        let free = match kind {
            ScriptKind::Sub => base.sub.is_none(),
            ScriptKind::Sup => base.sup.is_none(),
        };
        if free {
            let descent = empty_base_descent(&script);
            match kind {
                ScriptKind::Sub => base.sub = Some(script),
                ScriptKind::Sup => base.sup = Some(script),
            }
            chain.steps.push(kind);
            chain.steps.extend(descent);
            chain.last = chain.steps.last().copied();
            return Some(());
        }
    }
    // A pathologically long nesting chain (`a^a^a^…`) is rejected rather than built into a tree deep
    // enough to overflow the stack when it is later rendered.
    if chain.steps.len() >= MAX_DEPTH {
        return None;
    }
    // Repeating the previous kind nests one level deeper into the active atom: fill the active's
    // matching slot, then descend so the new content becomes the active. Nothing nests into a prime
    // mark, so a script landing on a prime active siblings the base instead.
    if chain.last == Some(kind) && !active_atom(base, chain).is_some_and(|a| a.is_prime()) {
        let descent = empty_base_descent(&script);
        let active = active_atom(base, chain)?;
        match kind {
            ScriptKind::Sub if active.sub.is_none() => active.sub = Some(script),
            ScriptKind::Sup if active.sup.is_none() => active.sup = Some(script),
            _ => return None,
        }
        chain.steps.push(kind);
        chain.steps.extend(descent);
        chain.last = chain.steps.last().copied();
        return Some(());
    }
    // The other kind applies to the active atom's parent — one step up the chain — so a deeper base
    // collects the matching pair beside its existing script. This reaches up only when the chain has
    // descended at least one level; at the chain root (or when sealed) a different-kind script
    // siblings the base in source order.
    if !chain.sealed && !chain.steps.is_empty() {
        chain.steps.pop();
        let parent = active_atom(base, chain)?;
        let free = match kind {
            ScriptKind::Sub => parent.sub.is_none(),
            ScriptKind::Sup => parent.sup.is_none(),
        };
        if free {
            match kind {
                ScriptKind::Sub => parent.sub = Some(script),
                ScriptKind::Sup => parent.sup = Some(script),
            }
            chain.steps.push(kind);
            chain.last = Some(kind);
            return Some(());
        }
    }
    // Otherwise the script siblings the base in source order and becomes the active group.
    base.siblings.push(Sibling {
        kind,
        atoms: script,
        sealed: chain.sealed,
    });
    chain.sibling = Some(base.siblings.len() - 1);
    chain.steps.clear();
    chain.last = Some(kind);
    Some(())
}

/// Append a prime mark to an atom outside a script chain (matrix cells, `\left…\right` runs): merge
/// into a trailing prime superscript when one is already present, else add a fresh prime sibling.
fn push_prime(atom: &mut Atom) {
    if let Some(sibling) = atom.siblings.last_mut()
        && sibling.kind == ScriptKind::Sup
        && let Some(last) = sibling.atoms.last_mut()
        && let Body::Prime(count) = &mut last.body
    {
        *count = count.saturating_add(1);
        return;
    }
    if atom.sup.is_none() && atom.siblings.is_empty() {
        atom.sup = Some(vec![Atom::new(Body::Prime(1))]);
    } else {
        atom.siblings.push(Sibling {
            kind: ScriptKind::Sup,
            atoms: vec![Atom::new(Body::Prime(1))],
            sealed: false,
        });
    }
}

/// Whether a prime mark detaches from `base` to surface as a bare prime glyph rather than nesting as
/// a superscript. A prime is a superscript; it nests when the chain offers a free superscript slot,
/// either on the active atom (a repeated-superscript nest) or on its parent (the matching-pair reach
/// up one level). When neither offers a slot the superscript would have to start a fresh sibling
/// group, with nowhere to nest — there TeX detaches the prime, so it surfaces as a bare glyph that
/// starts a new base. The shapes this covers include `a_b'` and the mirror `a_b^c'`, as well as the
/// deeper `a^c'_d'`, where the active atom already carries a primary prime.
///
/// A sealed base (`{…}'`) or a chain already pointing at a sibling group keeps its prime nested, so
/// only an unsealed primary chain can detach.
fn prime_detaches(base: &Atom, chain: &ScriptChain) -> bool {
    if chain.sealed || chain.sibling.is_some() {
        return false;
    }
    // At the chain root a prime fills the base's own free superscript slot, so it never detaches.
    if chain.last.is_none() {
        return false;
    }
    let Some(active) = active_atom_ref(base, chain) else {
        return false;
    };
    // A prime written onto an active prime merges into its count (`a''` is one double-prime, and a
    // prime that landed on the base after a subscript stays the active atom, so `a_b''` keeps both
    // primes together), so it never detaches.
    if active.is_prime() {
        return false;
    }
    // At the primary level — a single script step from the base — a prime detaches once both of the
    // base's primary slots are occupied: the flat `a'_b'` and the mirror `a_b^c'`, in either order.
    // The step's own slot is one of the two, so the test is that the other slot is also filled.
    if chain.steps.len() == 1 && base.sub.is_some() && base.sup.is_some() {
        return true;
    }
    // A repeated superscript nests onto the active atom when its slot is free.
    if chain.last == Some(ScriptKind::Sup) && active.sup.is_none() {
        return false;
    }
    // Otherwise the prime reaches up to the active atom's parent; it nests when that slot is free and
    // detaches when the parent already carries a superscript (the deeper `a^c'_d'` shape).
    parent_atom_ref(base, chain).is_none_or(|parent| parent.sup.is_some())
}

/// The atom one chain step above the active atom — the target of a matching-pair script that reaches
/// up a level. `None` when the active atom is the chain root (the base itself), which has no parent.
fn parent_atom_ref<'a>(base: &'a Atom, chain: &ScriptChain) -> Option<&'a Atom> {
    let parent_steps = chain.steps.split_last()?.1;
    descend_ref(base, chain.sibling, parent_steps)
}

/// Attach one prime mark to `base`, flowing it through `chain` as a superscript so its place among
/// other scripts is preserved. Consecutive primes merge into the count of a single [`Body::Prime`]
/// atom rather than nesting (`a''` is one double-prime, not a prime on a prime).
fn attach_prime(base: &mut Atom, chain: &mut ScriptChain) -> Option<()> {
    if chain.last == Some(ScriptKind::Sup)
        && let Some(active) = active_atom(base, chain)
        && let Body::Prime(count) = &mut active.body
    {
        *count = count.saturating_add(1);
        return Some(());
    }
    attach_script(
        base,
        chain,
        ScriptKind::Sup,
        vec![Atom::new(Body::Prime(1))],
    )
}

/// The chain of script steps that descends from a freshly-attached script's root to the deepest real
/// atom through any synthesized empty nuclei. A script written as a bare operator chain (`a^_b`,
/// `a__b`) attaches as one or more nested [`Body::Empty`] bases; a following flat script must bind
/// onto the deepest of those, so the chain descends through each empty's sole filled slot. Descent
/// stops at the first non-empty nucleus (the real script content), which becomes the new active base.
fn empty_base_descent(script: &[Atom]) -> Vec<ScriptKind> {
    let mut steps = Vec::new();
    let mut current = script;
    while let [atom] = current {
        if !matches!(atom.body, Body::Empty) {
            break;
        }
        if let Some(inner) = atom.sub.as_deref() {
            steps.push(ScriptKind::Sub);
            current = inner;
        } else if let Some(inner) = atom.sup.as_deref() {
            steps.push(ScriptKind::Sup);
            current = inner;
        } else {
            break;
        }
    }
    steps
}

/// The read-only twin of [`active_atom`]: resolve the atom `chain` currently points at within `base`,
/// descending through the chosen sibling group and each script step without taking a mutable borrow.
fn active_atom_ref<'a>(base: &'a Atom, chain: &ScriptChain) -> Option<&'a Atom> {
    descend_ref(base, chain.sibling, &chain.steps)
}

/// Resolve an atom by descending from `base` into the optional sibling group, then following each
/// script `step` into the last atom of the matching slot. The read-only core shared by the active-
/// and parent-atom resolvers.
fn descend_ref<'a>(
    base: &'a Atom,
    sibling: Option<usize>,
    steps: &[ScriptKind],
) -> Option<&'a Atom> {
    let mut atom = match sibling {
        Some(i) => base.siblings.get(i)?.atoms.last()?,
        None => base,
    };
    for step in steps {
        let group = match step {
            ScriptKind::Sub => atom.sub.as_deref()?,
            ScriptKind::Sup => atom.sup.as_deref()?,
        };
        atom = group.last()?;
    }
    Some(atom)
}

/// Resolve the active atom that `chain` currently points at within `base`: descend into the chosen
/// sibling group (or the base itself), then follow each step into the last atom of that script group.
fn active_atom<'a>(base: &'a mut Atom, chain: &ScriptChain) -> Option<&'a mut Atom> {
    let mut atom = match chain.sibling {
        Some(i) => base.siblings.get_mut(i)?.atoms.last_mut()?,
        None => base,
    };
    for step in &chain.steps {
        let group = match step {
            ScriptKind::Sub => atom.sub.as_mut()?,
            ScriptKind::Sup => atom.sup.as_mut()?,
        };
        atom = group.last_mut()?;
    }
    Some(atom)
}

/// Parse a run of atoms until end-of-input or (when `in_group`) the matching `}`.
// One flat dispatch over token kinds, each arm a few lines; splitting it would scatter the
// shared script-chain state across helpers and obscure the single sequential pass.
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
    // Tracks how a chain of scripts descends into the last atom so consecutive same-kind scripts
    // nest (`a^b^c` is `a^{b^c}`) while a script across a group boundary or after a different-kind
    // script siblings (`{x^2}^3`). Reset whenever a fresh nucleus or group is pushed.
    let mut chain = ScriptChain::default();
    while let Some(tok) = tokens.get(*pos) {
        match tok {
            Token::GroupClose => {
                if in_group {
                    *pos += 1;
                    return Some(atoms);
                }
                // An unmatched closing brace is malformed.
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
                // A script with no preceding atom attaches to a synthesized empty nucleus.
                if atoms.is_empty() {
                    atoms.push(Atom::new(Body::Empty));
                    chain = ScriptChain::default();
                }
                let base = atoms.last_mut()?;
                attach_script(base, &mut chain, kind, script)?;
            }
            Token::GroupOpen => {
                // A standalone brace group is transparent: its atoms splice into the surrounding
                // run, so a following script binds to the group's last atom. An empty group is a
                // bare empty nucleus that a following script can attach to.
                *pos += 1;
                let inner = parse_atoms(tokens, pos, depth + 1, true)?;
                if inner.is_empty() {
                    atoms.push(Atom::new(Body::EmptyGroup));
                } else {
                    atoms.extend(inner);
                }
                // A spliced atom that already carries a script is sealed: a following script
                // siblings in source order rather than filling the atom's free primary slot.
                let sealed = atoms.last().is_some_and(Atom::is_scripted);
                chain = ScriptChain {
                    sealed,
                    ..ScriptChain::default()
                };
            }
            // A trailing apostrophe is a prime mark on the preceding atom, flowing through the same
            // script chain as a superscript. With no preceding atom (a leading or lone prime) it is a
            // bare prime glyph in its own right, and consecutive bare primes merge.
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
                    // A prime written right after a primary subscript on a base whose superscript slot
                    // is already filled cannot become a second superscript: TeX detaches it, so it
                    // surfaces as a bare prime glyph after the subscript and starts a fresh base that
                    // later scripts attach to. A braced subscript seals its base and nests instead.
                    Some(base) if prime_detaches(base, &chain) => {
                        atoms.push(Atom::new(Body::Prime(1)));
                        chain = ScriptChain::default();
                    }
                    Some(base) => attach_prime(base, &mut chain)?,
                }
            }
            // `\limits`/`\nolimits` set the limit placement of the preceding operator.
            Token::Command(c) if c == "limits" || c == "nolimits" => {
                let forced = c == "limits";
                *pos += 1;
                let last = atoms.last_mut()?;
                last.limits = Some(forced);
            }
            // An equation-numbering annotation carries no visible glyph: `\nonumber` and `\tag{…}` are
            // dropped, while `\label{…}` is captured as a `Label` atom that the Typst backend lifts to a
            // trailing reference. These annotate the whole expression, so they apply wherever they sit
            // at the top level — before or after rendered content — and are consumed rather than left as
            // unknown control sequences that would force a verbatim fallback.
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
            // A style switch (`\displaystyle`, `\textstyle`, …) only changes typesetting size and has
            // no glyph; it is dropped and the run continues.
            Token::Command(c) if is_style_switch(c) => {
                *pos += 1;
            }
            // `\color{<spec>}` recolours the following content; the colour is invisible in linear and
            // Typst output, so the colour-spec group is consumed and dropped.
            Token::Command(c) if c == "color" => {
                *pos += 1;
                parse_required_group(tokens, pos, depth, &mut None)?;
            }
            // `\DeclareMathOperator{\name}{body}` is a preamble declaration with no typeset glyph of
            // its own: both groups are consumed and the run continues, so a lone declaration produces
            // empty output. (The declared operator is not registered for later use.)
            Token::Command(c) if c == "DeclareMathOperator" => {
                *pos += 1;
                if matches!(tokens.get(*pos), Some(Token::Char('*'))) {
                    *pos += 1;
                }
                skip_balanced_group(tokens, pos)?;
                skip_balanced_group(tokens, pos)?;
            }
            // An infix binomial operator (`a \choose b`, `\brace`, `\brack`) splits its surrounding
            // group: everything parsed so far is the upper argument, everything to the group's end is
            // the lower. The whole run collapses to one stacked atom.
            Token::Command(c) if binom_kind(c).is_some() => {
                let kind = binom_kind(c)?;
                *pos += 1;
                let top = std::mem::take(&mut atoms);
                let bottom = parse_atoms(tokens, pos, depth + 1, in_group)?;
                return Some(vec![Atom::new(Body::Binom(kind, top, bottom))]);
            }
            // A `\begin{env} … \end{env}` environment. A transparent single-line equation wrapper
            // splices its content into the surrounding run (so it reads as ordinary math, not a
            // grouped operand); every other environment is a single atom.
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
            // `\not` over a numeric literal strikes only its first digit; the remaining digits stay a
            // separate number atom. The whole number is one token, so the split happens here where the
            // tail can be pushed as its own atom.
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
    if in_group {
        // Reached end of input without the matching `}`.
        None
    } else {
        Some(atoms)
    }
}

/// Parse the argument of a `_`/`^`: a single atom, a braced group, or — when the argument is itself a
/// script operator — a synthesized empty nucleus carrying the nested script.
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
            // An empty braced argument (`^{}`) is an explicit empty nucleus, not an empty atom run:
            // it occupies the script slot so a following script can nest onto it.
            if inner.is_empty() {
                return Some(vec![Atom::new(Body::EmptyGroup)]);
            }
            // A braced prime run (`^{'''''}`) is a prime *nucleus*, not a trailing prime attached to
            // the base: a long run stacks its remainder into its own superscript rather than spilling
            // flat across the baseline. Wrapping the sole prime atom in a transparent group routes it
            // through the nucleus path, which nests; a direct trailing prime keeps the flat form.
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
        // A script whose argument is itself a script operator (`a^^b`, `a^_b`) takes an empty
        // implicit nucleus and nests the inner script onto it.
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
        // A bare backslash reaches here only as a dangling control sequence (a `\` with no name and
        // nothing following). It is not a complete expression, so the whole input is left verbatim.
        Token::Char('\\') => None,
        // A colon immediately followed by an equals (no space between) is the `:=` relation digraph.
        Token::Char(':') if matches!(tokens.get(*pos + 1), Some(Token::Char('='))) => {
            *pos += 2;
            Some(Atom::new(Body::ColonEq))
        }
        // A bare double quote, backtick, or dollar has no ordinary-symbol meaning in math mode, so an
        // expression containing one is unparsable and the writer emits it verbatim. The backslash
        // forms (`\"`, `` \` ``, `\$`) tokenize as commands, and these characters inside a `\text{…}`
        // group are consumed by the text path, so none reaches here.
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
        // A bare script or close brace is not a valid nucleus here.
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

/// A fixed-size delimiter wrapper (`\big`, `\Big`, `\bigg`, `\Bigg`, with optional `l`/`r` variant)
/// sizes the single delimiter that follows. The size is presentational, so the wrapper is stripped
/// and the bare delimiter rendered. The relation-sizing `m`-suffix variants (`\bigm`, `\Bigm`, …) are
/// excluded: they size a relation rather than a delimiter, so the whole expression falls back to
/// verbatim.
fn big_delim_scale(name: &str) -> Option<u16> {
    let scale = match name {
        "big" | "bigl" | "bigr" => 120,
        "Big" | "Bigl" | "Bigr" => 180,
        "bigg" | "biggl" | "biggr" => 240,
        "Bigg" | "Biggl" | "Biggr" => 300,
        _ => return None,
    };
    Some(scale)
}

/// The nucleus a fixed-size wrapper (`\big…`) sizes, given the single follower token, or `None` when
/// the follower is not a token the wrapper accepts (a letter, digit, TeX-active character, or any
/// command outside the delimiter set), which leaves the wrapper unhandled and the whole expression
/// verbatim.
///
/// The wrapper sizes far more than the stretchy delimiters a `\left … \right` pair takes: any single
/// ordinary, relation, or punctuation character is also accepted and sized as a literal glyph (`\big+`
/// is a sized `+`, `\big<` a sized `<`). The null delimiter `.` is a sized literal period. The result
/// always carries ordinary class, so a sized glyph binds tightly to its neighbours regardless of the
/// character's usual class. Primes and the `:=` digraph span more than one token and are handled by
/// the caller; this single-token set excludes them.
fn big_follower(tok: &Token) -> Option<Body> {
    match tok {
        // Every accepted single character: the stretchy-delimiter characters plus the ordinary,
        // relation, and punctuation marks rendered as literal characters.
        Token::Char(c) => match c {
            '(' | ')' | '[' | ']' | '|' | '<' | '>' | '/' | '.' | '!' | '*' | '+' | ',' | '-'
            | ':' | ';' | '=' | '?' | '@' | '~' => Some(Body::Char(*c)),
            _ => None,
        },
        // The delimiter commands: braces, vertical bars, angle brackets, floors and ceilings.
        Token::Command(c) => matches!(
            c.as_str(),
            "{" | "}"
                | "lbrace"
                | "rbrace"
                | "|"
                | "vert"
                | "Vert"
                | "lVert"
                | "rVert"
                | "lvert"
                | "rvert"
                | "langle"
                | "rangle"
                | "lfloor"
                | "rfloor"
                | "lceil"
                | "rceil"
                | "lbrack"
                | "rbrack"
        )
        .then(|| Body::Command(c.clone())),
        _ => None,
    }
}

/// Parse a fixed-size delimiter (`\big(`, `\Bigl[`, …): the wrapper sizes the single delimiter that
/// follows. See [`big_follower`] for the accepted single-token set; an unaccepted follower leaves the
/// wrapper unhandled so the whole expression falls back to verbatim.
///
/// Two followers span more than one token and are taken as a unit: a run of consecutive primes
/// (`\big''` is one sized double-prime, not a prime carrying a prime script) and the `:=` digraph
/// (`\big:=` is one sized relation, not a sized `:` followed by a loose `=`).
fn parse_big_delim(scale: u16, tokens: &[Token], pos: &mut usize, _depth: usize) -> Option<Atom> {
    skip_spaces(tokens, pos);
    let nucleus = match tokens.get(*pos)? {
        Token::Char('\'') => {
            let mut count: u8 = 0;
            while let Some(Token::Char('\'')) = tokens.get(*pos) {
                count = count.checked_add(1)?;
                *pos += 1;
            }
            Body::Prime(count)
        }
        Token::Char(':') if matches!(tokens.get(*pos + 1), Some(Token::Char('='))) => {
            *pos += 2;
            Body::ColonEq
        }
        tok => {
            let body = big_follower(tok)?;
            *pos += 1;
            body
        }
    };
    Some(Atom::new(Body::Big(scale, Box::new(Atom::new(nucleus)))))
}

#[allow(clippy::too_many_lines)]
fn parse_command(
    name: &str,
    tokens: &[Token],
    pos: &mut usize,
    depth: usize,
    tail: &mut Option<String>,
) -> Option<Atom> {
    if let Some(scale) = big_delim_scale(name) {
        return parse_big_delim(scale, tokens, pos, depth);
    }
    if is_accent_command(name) {
        let arg = parse_required_group(tokens, pos, depth, tail)?;
        return Some(Atom::new(Body::Accent(name.to_string(), arg)));
    }
    if is_text_command(name) {
        // `\operatorname*` reads its content like `\operatorname`, but its recorded name keeps the star
        // so the display backend can center a subscript beneath it (the limits variant).
        let mut wrapper = name;
        if name == "operatorname" && matches!(tokens.get(*pos), Some(Token::Char('*'))) {
            *pos += 1;
            wrapper = "operatorname*";
        }
        // `\operatorname` is math content set upright; the other wrappers hold literal text.
        let mode = if name == "operatorname" {
            TextMode::Math
        } else {
            TextMode::Wrapper
        };
        let text = parse_verbatim_group(tokens, pos, mode)?;
        return Some(Atom::new(Body::Text(wrapper.to_string(), text)));
    }
    if is_styled_command(name) {
        let arg = parse_required_group(tokens, pos, depth, tail)?;
        return Some(Atom::new(Body::Styled(name.to_string(), arg)));
    }
    match name {
        "frac" | "dfrac" | "tfrac" => {
            let num = parse_required_group(tokens, pos, depth, tail)?;
            let den = parse_required_group(tokens, pos, depth, tail)?;
            let style = if name == "tfrac" {
                FracStyle::Linear
            } else {
                FracStyle::Bar
            };
            Some(Atom::new(Body::Frac(style, num, den)))
        }
        "binom" => {
            let top = parse_required_group(tokens, pos, depth, tail)?;
            let bottom = parse_required_group(tokens, pos, depth, tail)?;
            Some(Atom::new(Body::Binom(BinomKind::Paren, top, bottom)))
        }
        "genfrac" => parse_genfrac(tokens, pos, depth, tail),
        "xrightarrow" | "xleftarrow" => parse_extensible_arrow(name, tokens, pos, depth, tail),
        "overbrace" => {
            let arg = parse_required_group(tokens, pos, depth, tail)?;
            Some(Atom::new(Body::Brace(BraceKind::Over, arg)))
        }
        "underbrace" => {
            let arg = parse_required_group(tokens, pos, depth, tail)?;
            Some(Atom::new(Body::Brace(BraceKind::Under, arg)))
        }
        "sqrt" => {
            let index = parse_optional_bracket(tokens, pos, depth);
            // A bare `\sqrt` with neither an index nor a radicand is the lone radical sign `√`. An
            // index without a radicand (`\sqrt[3]`) is malformed and falls back to verbatim.
            match parse_required_group(tokens, pos, depth, tail) {
                Some(radicand) => Some(Atom::new(Body::Sqrt(index, radicand))),
                None if index.is_none() => Some(Atom::new(Body::Char('\u{221A}'))),
                None => None,
            }
        }
        // A bare radical sign takes the single following atom as its radicand.
        "surd" => {
            let radicand = parse_required_group(tokens, pos, depth, tail)?;
            Some(Atom::new(Body::Sqrt(None, radicand)))
        }
        // A `\begin{env} … \end{env}` reached as a nested atom (a script or accent argument) wraps its
        // content in a single transparent group; at the top of a run it is intercepted and spliced.
        "begin" => {
            let spliced = parse_environment(tokens, pos, depth)?;
            Some(Atom::new(Body::Group(spliced)))
        }
        // `\overset{mark}{base}` / `\underset` / `\stackrel{mark}{base}` set a mark over or under a
        // base. `\stackrel` is the over form.
        "overset" | "stackrel" => {
            let mark = parse_required_group(tokens, pos, depth, tail)?;
            let base = parse_required_group(tokens, pos, depth, tail)?;
            Some(Atom::new(Body::Stack(StackSide::Over, mark, base)))
        }
        "underset" => {
            let mark = parse_required_group(tokens, pos, depth, tail)?;
            let base = parse_required_group(tokens, pos, depth, tail)?;
            Some(Atom::new(Body::Stack(StackSide::Under, mark, base)))
        }
        // `\substack{a \\ b}` stacks its `\\`-separated rows.
        "substack" => {
            skip_spaces(tokens, pos);
            if !matches!(tokens.get(*pos), Some(Token::GroupOpen)) {
                return None;
            }
            *pos += 1;
            let rows = parse_grid_rows_braced(tokens, pos, depth)?;
            Some(Atom::new(Body::Grid(GridKind::Substack, Vec::new(), rows)))
        }
        "left" => parse_delimited(tokens, pos, depth),
        // `\bmod` is an infix operator: it needs a following operand. With nothing after it (the end
        // of the input or the close of its group) it is invalid and falls back to verbatim.
        "bmod" => {
            let mut probe = *pos;
            while matches!(tokens.get(probe), Some(Token::Space)) {
                probe += 1;
            }
            if tokens
                .get(probe)
                .is_none_or(|t| matches!(t, Token::GroupClose))
            {
                return None;
            }
            Some(Atom::new(Body::Mod(ModKind::Bmod, None)))
        }
        "pmod" => {
            let arg = parse_required_group(tokens, pos, depth, tail)?;
            Some(Atom::new(Body::Mod(ModKind::Pmod, Some(arg))))
        }
        // `\mod` leads its operand, which stays a separate atom; the operator is invalid with no
        // operand to follow it.
        "mod" => {
            skip_spaces(tokens, pos);
            if tokens.get(*pos).is_none_or(|t| {
                matches!(
                    t,
                    Token::Sub | Token::Sup | Token::GroupClose | Token::Space
                )
            }) {
                return None;
            }
            Some(Atom::new(Body::Mod(ModKind::Mod, None)))
        }
        "pod" => {
            let arg = parse_required_group(tokens, pos, depth, tail)?;
            Some(Atom::new(Body::Mod(ModKind::Pod, Some(arg))))
        }
        // `\not` strikes through the relation that follows it: a command name, or a single relation
        // character such as `=`/`<`/`>`.
        "not" => {
            skip_spaces(tokens, pos);
            // A braced base (`\not{a}`) negates the whole group; a bare command or character negates
            // that single token.
            if matches!(tokens.get(*pos), Some(Token::GroupOpen)) {
                *pos += 1;
                let inner = parse_atoms(tokens, pos, depth + 1, true)?;
                return Some(Atom::new(Body::NegatedGroup(inner)));
            }
            // A literal character base always strikes through; a command base strikes only when it
            // composes into a struck form (a precomposed negated relation or an italic letterlike).
            // Other commands — the bar commands `\|`/`\Vert`, delimiters, operators, upright
            // letterlikes — have no struck form, so `\not\|` and the like are left verbatim.
            let base = match tokens.get(*pos)? {
                Token::Command(c) if super::symbols::command_negatable(c) => c.clone(),
                Token::Char(c) => c.to_string(),
                // A non-negatable command, or any other token, has no struck form.
                _ => return None,
            };
            *pos += 1;
            Some(Atom::new(Body::Negated(base)))
        }
        _ => Some(Atom::new(Body::Command(name.to_string()))),
    }
}

/// Whether a `\not` at `pos` (a command token) is immediately followed by a numeric literal, after
/// any intervening spaces. Such a `\not` strikes only the number's first digit.
fn not_over_number(tokens: &[Token], pos: usize) -> bool {
    let mut probe = pos + 1;
    while matches!(tokens.get(probe), Some(Token::Space)) {
        probe += 1;
    }
    matches!(tokens.get(probe), Some(Token::Number(_)))
}

/// Parse a `\left<delim> … \right<delim>` group: read the opening delimiter, the enclosed run up to
/// the matching `\right`, and the closing delimiter.
fn parse_delimited(tokens: &[Token], pos: &mut usize, depth: usize) -> Option<Atom> {
    if depth > MAX_DEPTH {
        return None;
    }
    let open = parse_delimiter(tokens, pos)?;
    let mut inner: Vec<Atom> = Vec::new();
    loop {
        match tokens.get(*pos) {
            Some(Token::Command(c)) if c == "right" => {
                *pos += 1;
                let close = parse_delimiter(tokens, pos)?;
                return Some(Atom::new(Body::Delimited(open, close, inner)));
            }
            Some(Token::Command(c)) if c == "middle" => {
                *pos += 1;
                let middle = parse_middle_delimiter(tokens, pos)?;
                let (delim, open_side) = match middle {
                    Some((d, side)) => (Some(d), side),
                    None => (None, true),
                };
                inner.push(Atom::new(Body::Middle(delim, open_side)));
            }
            Some(Token::GroupClose) | None => return None,
            Some(Token::Space) => {
                *pos += 1;
            }
            Some(Token::Sub | Token::Sup) => {
                let is_sup = matches!(tokens.get(*pos), Some(Token::Sup));
                *pos += 1;
                let script = parse_script(tokens, pos, depth + 1)?;
                let last = inner.last_mut()?;
                if is_sup {
                    if last.sup.is_some() {
                        return None;
                    }
                    last.sup = Some(script);
                } else {
                    if last.sub.is_some() {
                        return None;
                    }
                    last.sub = Some(script);
                }
            }
            Some(Token::GroupOpen) => {
                *pos += 1;
                let group = parse_atoms(tokens, pos, depth + 1, true)?;
                if group.is_empty() {
                    inner.push(Atom::new(Body::EmptyGroup));
                } else {
                    inner.extend(group);
                }
            }
            Some(Token::Char('\'')) => {
                *pos += 1;
                let last = inner.last_mut()?;
                push_prime(last);
            }
            Some(_) => {
                parse_atom_into(tokens, pos, depth + 1, &mut inner)?;
            }
        }
    }
}

/// Read one delimiter token following `\left`/`\right`. The outer `Option` is the parse result and
/// the inner one distinguishes an absent delimiter (`.`, the inner `None`) from a present one.
#[allow(clippy::option_option)]
fn parse_delimiter(tokens: &[Token], pos: &mut usize) -> Option<Option<Delim>> {
    skip_spaces(tokens, pos);
    let delim = match tokens.get(*pos)? {
        Token::Char('.') => None,
        Token::Char('(' | ')') => Some(Delim::Paren),
        Token::Char('[' | ']') => Some(Delim::Bracket),
        Token::Char('|') => Some(Delim::Bar),
        Token::Char('<' | '>') => Some(Delim::Angle),
        Token::Command(c) => match c.as_str() {
            "{" | "lbrace" | "}" | "rbrace" => Some(Delim::Brace),
            "Vert" => Some(Delim::DoubleBar),
            "|" | "lVert" | "rVert" => Some(Delim::BarVert),
            "lvert" | "rvert" => Some(Delim::Bar),
            "langle" | "rangle" => Some(Delim::Angle),
            "lfloor" | "rfloor" => Some(Delim::Floor),
            "lceil" | "rceil" => Some(Delim::Ceil),
            "ulcorner" => Some(Delim::CornerUpperLeft),
            "urcorner" => Some(Delim::CornerUpperRight),
            _ => return None,
        },
        _ => return None,
    };
    *pos += 1;
    Some(delim)
}

/// Read the delimiter following a `\middle`, with its side. The outer `Option` is the parse result;
/// the inner one distinguishes an absent delimiter (`.`) from a present `(Delim, is_open_side)`. A
/// one-sided delimiter carries the side of the glyph that was written; a symmetric bar takes the
/// opening side by convention (its name has no side suffix). A non-delimiter (`/`, `\backslash`)
/// has no stretchy middle form, so the whole group falls back to verbatim.
#[allow(clippy::option_option)]
fn parse_middle_delimiter(tokens: &[Token], pos: &mut usize) -> Option<Option<(Delim, bool)>> {
    skip_spaces(tokens, pos);
    let middle = match tokens.get(*pos)? {
        Token::Char('.') => None,
        Token::Char('(') => Some((Delim::Paren, true)),
        Token::Char(')') => Some((Delim::Paren, false)),
        Token::Char('[') => Some((Delim::Bracket, true)),
        Token::Char(']') => Some((Delim::Bracket, false)),
        Token::Char('|') => Some((Delim::Bar, true)),
        Token::Char('<') => Some((Delim::Angle, true)),
        Token::Char('>') => Some((Delim::Angle, false)),
        Token::Command(c) => match c.as_str() {
            "{" | "lbrace" => Some((Delim::Brace, true)),
            "}" | "rbrace" => Some((Delim::Brace, false)),
            "Vert" => Some((Delim::DoubleBar, true)),
            "|" | "lVert" | "rVert" => Some((Delim::BarVert, true)),
            "vert" | "lvert" => Some((Delim::Bar, true)),
            "rvert" => Some((Delim::Bar, false)),
            "langle" => Some((Delim::Angle, true)),
            "rangle" => Some((Delim::Angle, false)),
            "lfloor" => Some((Delim::Floor, true)),
            "rfloor" => Some((Delim::Floor, false)),
            "lceil" => Some((Delim::Ceil, true)),
            "rceil" => Some((Delim::Ceil, false)),
            "ulcorner" => Some((Delim::CornerUpperLeft, true)),
            "urcorner" => Some((Delim::CornerUpperRight, false)),
            _ => return None,
        },
        _ => return None,
    };
    *pos += 1;
    Some(middle)
}

/// Parse a `\begin{env} … \end{env}` matrix environment into a grid of cells.
fn parse_environment(tokens: &[Token], pos: &mut usize, depth: usize) -> Option<Vec<Atom>> {
    if depth > MAX_DEPTH {
        return None;
    }
    let env = text_pieces_to_string(&parse_verbatim_group(tokens, pos, TextMode::Math)?);
    // The mathtools starred matrix/cases environments (`matrix*`, `pmatrix*`, … `cases*`,
    // `smallmatrix*`) render as their unstarred form but accept an optional `[align]` argument
    // selecting per-column alignment. The alignment is presentational and not reproduced; the bracket
    // group is consumed as literal leading content of the first cell. The base name without the `*`
    // drives the delimiter/kind lookup, while the full name still matches the `\end{…*}` closing tag.
    let starred_grid = matches!(
        env.as_str(),
        "matrix*"
            | "pmatrix*"
            | "bmatrix*"
            | "Bmatrix*"
            | "vmatrix*"
            | "Vmatrix*"
            | "smallmatrix*"
            | "cases*"
    );
    let base = if starred_grid {
        env.strip_suffix('*').unwrap_or(env.as_str())
    } else {
        env.as_str()
    };
    let matrix_delim = match base {
        "matrix" => Some(MatrixDelim::None),
        "pmatrix" => Some(MatrixDelim::Paren),
        "bmatrix" => Some(MatrixDelim::Bracket),
        "Bmatrix" => Some(MatrixDelim::Brace),
        "vmatrix" => Some(MatrixDelim::Bar),
        "Vmatrix" => Some(MatrixDelim::DoubleBar),
        _ => None,
    };
    let grid_kind = match base {
        // The multi-line equation environments are transparent alignment grids: rows split on `\\`,
        // columns on `&`. The single-line `equation`/`equation*` are handled below.
        "aligned" | "align" | "aligned*" | "align*" | "split" | "alignat" | "alignat*"
        | "alignedat" | "alignedat*" => Some(GridKind::Aligned),
        "gathered" | "gather" | "gather*" | "smallmatrix" | "multline" | "multline*"
        | "multlined" | "multlined*" => Some(GridKind::Gathered),
        "eqnarray" | "eqnarray*" => Some(GridKind::Eqnarray),
        "flalign" | "flalign*" | "flaligned" | "flaligned*" => Some(GridKind::Flalign),
        "array" => Some(GridKind::Array),
        "cases" => Some(GridKind::Cases),
        _ => None,
    };
    // The single-line equation environments wrap one math expression with no alignment: a `&` or a
    // row break is invalid inside them, so a grid with more than one cell falls back to verbatim.
    let single_line = matches!(env.as_str(), "equation" | "equation*");
    if matrix_delim.is_none() && grid_kind.is_none() && !single_line {
        return None;
    }
    // `\begin{array}{cols}` carries a column-specification group declaring each column's
    // justification. The `alignat`/`alignedat` environments likewise carry a mandatory `{N}`
    // column-count group; with no such group they are malformed and fall back to verbatim.
    let array_aligns = if env == "array" {
        let spec = text_pieces_to_string(&parse_verbatim_group(tokens, pos, TextMode::Math)?);
        parse_column_aligns(&spec)
    } else {
        Vec::new()
    };
    if matches!(
        env.as_str(),
        "alignat" | "alignat*" | "alignedat" | "alignedat*"
    ) {
        parse_verbatim_group(tokens, pos, TextMode::Math)?;
    }
    // A starred grid's optional `[align]` argument becomes the literal leading content of the first
    // cell: the bracket characters and their contents are kept verbatim as ordinary atoms.
    let leading = if starred_grid {
        optional_bracket_literal(tokens, pos)
    } else {
        Vec::new()
    };
    let mut rows = parse_grid_rows(tokens, pos, depth, &env)?;
    prepend_first_cell(&mut rows, leading);
    if single_line {
        return single_cell(rows);
    }
    if let Some(delim) = matrix_delim {
        return Some(vec![Atom::new(Body::Matrix(delim, rows))]);
    }
    let kind = grid_kind?;
    Some(vec![Atom::new(Body::Grid(kind, array_aligns, rows))])
}

/// Read an optional `[…]` bracket group immediately following the environment opener as a run of
/// literal atoms (the `[`, its contents, and the `]`, each an ordinary character). Returns an empty
/// run when no bracket follows, leaving the position unchanged. An unterminated `[` is also left in
/// place and yields nothing, so the bracket falls through to the first cell unaltered.
fn optional_bracket_literal(tokens: &[Token], pos: &mut usize) -> Vec<Atom> {
    if !matches!(tokens.get(*pos), Some(Token::Char('['))) {
        return Vec::new();
    }
    let mut probe = *pos + 1;
    let mut literal = vec![Atom::new(Body::Char('['))];
    while let Some(tok) = tokens.get(probe) {
        match tok {
            Token::Char(']') => {
                literal.push(Atom::new(Body::Char(']')));
                *pos = probe + 1;
                return literal;
            }
            Token::Char(c) => literal.push(Atom::new(Body::Char(*c))),
            Token::Number(digits) => literal.push(Atom::new(Body::Number(digits.clone()))),
            Token::Space => literal.push(Atom::new(Body::Char(' '))),
            _ => return Vec::new(),
        }
        probe += 1;
    }
    Vec::new()
}

/// Prepend a run of literal atoms to the first cell of the first row of a grid. A leading atom run
/// captured from a starred grid's `[align]` argument is glued ahead of the first cell's content.
fn prepend_first_cell(rows: &mut [Vec<Vec<Atom>>], leading: Vec<Atom>) {
    if leading.is_empty() {
        return;
    }
    if let Some(cell) = rows.first_mut().and_then(|row| row.first_mut()) {
        cell.splice(0..0, leading);
    }
}

/// The single cell of a grid that must hold exactly one row of one cell, or `None` if it has an
/// alignment column or a row break. The single-line equation environments accept only such a grid.
fn single_cell(rows: Vec<Vec<Vec<Atom>>>) -> Option<Vec<Atom>> {
    let [row] = <[_; 1]>::try_from(rows).ok()?;
    let [cell] = <[_; 1]>::try_from(row).ok()?;
    Some(cell)
}

/// Read the `&`/`\\`-separated grid of cells of an environment up to its matching `\end{env}`.
fn parse_grid_rows(
    tokens: &[Token],
    pos: &mut usize,
    depth: usize,
    env: &str,
) -> Option<Vec<Vec<Vec<Atom>>>> {
    let mut rows: Vec<Vec<Vec<Atom>>> = Vec::new();
    let mut row: Vec<Vec<Atom>> = Vec::new();
    loop {
        let (cell, sep) = parse_matrix_cell(tokens, pos, depth + 1)?;
        match sep {
            CellEnd::Column => row.push(cell),
            CellEnd::Row => {
                row.push(cell);
                rows.push(std::mem::take(&mut row));
            }
            CellEnd::Environment => {
                let closing =
                    text_pieces_to_string(&parse_verbatim_group(tokens, pos, TextMode::Math)?);
                if closing != env {
                    return None;
                }
                row.push(cell);
                rows.push(row);
                return Some(rows);
            }
        }
    }
}

/// What terminated a matrix cell.
enum CellEnd {
    Column,
    Row,
    Environment,
}

/// Read the `\\`-separated rows of a braced grid (`\substack{a \\ b}`) up to its closing `}`. Each
/// row is a single cell, since `\substack` has no column separator.
fn parse_grid_rows_braced(
    tokens: &[Token],
    pos: &mut usize,
    depth: usize,
) -> Option<Vec<Vec<Vec<Atom>>>> {
    if depth > MAX_DEPTH {
        return None;
    }
    let mut rows: Vec<Vec<Vec<Atom>>> = Vec::new();
    let mut atoms: Vec<Atom> = Vec::new();
    while let Some(tok) = tokens.get(*pos) {
        match tok {
            Token::GroupClose => {
                *pos += 1;
                rows.push(vec![std::mem::take(&mut atoms)]);
                return Some(rows);
            }
            Token::Command(c) if c == "\\" => {
                *pos += 1;
                // A row break may carry an optional `[<dim>]` extra-space argument; the bracketed
                // dimension has no glyph and is dropped.
                skip_optional_break_dim(tokens, pos);
                rows.push(vec![std::mem::take(&mut atoms)]);
            }
            Token::Space => {
                *pos += 1;
            }
            _ => parse_atom_into(tokens, pos, depth + 1, &mut atoms)?,
        }
    }
    None
}

/// Parse a single matrix cell: a run of atoms up to the next `&`, `\\`, or `\end`.
fn parse_matrix_cell(
    tokens: &[Token],
    pos: &mut usize,
    depth: usize,
) -> Option<(Vec<Atom>, CellEnd)> {
    if depth > MAX_DEPTH {
        return None;
    }
    let mut atoms: Vec<Atom> = Vec::new();
    while let Some(tok) = tokens.get(*pos) {
        match tok {
            Token::Char('&') => {
                *pos += 1;
                return Some((atoms, CellEnd::Column));
            }
            Token::Command(c) if c == "\\" => {
                *pos += 1;
                // A row break may carry an optional `[<dim>]` extra-space argument; it has no glyph,
                // so the bracketed dimension is consumed and dropped.
                skip_optional_break_dim(tokens, pos);
                return Some((atoms, CellEnd::Row));
            }
            Token::Command(c) if c == "end" => {
                *pos += 1;
                return Some((atoms, CellEnd::Environment));
            }
            // A `\begin{env}` nested directly inside a cell splices its content into the cell, so an
            // alignment grid inside an `equation`/`gather` wrapper or another grid reads as part of
            // the surrounding alignment rather than as a parenthesised operand.
            Token::Command(c) if c == "begin" => {
                *pos += 1;
                let spliced = parse_environment(tokens, pos, depth)?;
                atoms.extend(spliced);
            }
            Token::Space => {
                *pos += 1;
            }
            Token::Sub | Token::Sup => {
                let is_sup = matches!(tok, Token::Sup);
                *pos += 1;
                let script = parse_script(tokens, pos, depth + 1)?;
                let last = atoms.last_mut()?;
                if is_sup {
                    if last.sup.is_some() {
                        return None;
                    }
                    last.sup = Some(script);
                } else {
                    if last.sub.is_some() {
                        return None;
                    }
                    last.sub = Some(script);
                }
            }
            Token::GroupOpen => {
                *pos += 1;
                let inner = parse_atoms(tokens, pos, depth + 1, true)?;
                atoms.extend(inner);
            }
            Token::Char('\'') => {
                *pos += 1;
                let last = atoms.last_mut()?;
                push_prime(last);
            }
            // An equation-numbering annotation carries no visible glyph inside a grid cell.
            // `\nonumber` stands alone; `\tag` (optionally starred `\tag*`) consumes and discards a
            // following braced argument; `\label` captures its argument as a `Label` atom that the
            // Typst backend later lifts out of the body. Each argument is a flat verbatim run, so an
            // inner `$` round-trips, while a nested group leaves the expression unhandled.
            Token::Command(c) if c == "nonumber" => {
                *pos += 1;
            }
            Token::Command(c) if c == "tag" => {
                *pos += 1;
                read_label_arg(tokens, pos)?;
            }
            Token::Command(c) if c == "label" => {
                *pos += 1;
                if let Some(name) = read_label_arg(tokens, pos)? {
                    atoms.push(Atom::new(Body::Label(name)));
                }
            }
            // A horizontal rule between matrix or array rows carries no glyph and does not affect the
            // cells it separates, so it is consumed and dropped.
            Token::Command(c) if c == "hline" || c == "hdashline" => {
                *pos += 1;
            }
            _ => {
                parse_atom_into(tokens, pos, depth, &mut atoms)?;
            }
        }
    }
    None
}

/// Commands that place a mark over a single base group. The set is broader than what inline output
/// can render: a few (e.g. `\ddot`, `\overline`) only translate to Typst and force a verbatim
/// fallback in inline output.
fn is_accent_command(name: &str) -> bool {
    matches!(
        name,
        "bar"
            | "hat"
            | "widehat"
            | "tilde"
            | "widetilde"
            | "vec"
            | "overrightarrow"
            | "overleftarrow"
            | "dot"
            | "ddot"
            | "check"
            | "breve"
            | "acute"
            | "grave"
            | "mathring"
            | "overline"
            | "underline"
            | "dddot"
            | "ddddot"
            | "overleftrightarrow"
            | "underleftarrow"
            | "underrightarrow"
    )
}

/// A style switch that only sets the typesetting size and carries no glyph.
fn is_style_switch(name: &str) -> bool {
    matches!(
        name,
        "displaystyle" | "textstyle" | "scriptstyle" | "scriptscriptstyle"
    )
}

/// The bracket kind of an infix binomial operator (`\choose`, `\brace`, `\brack`), or `None` for any
/// other command.
fn binom_kind(name: &str) -> Option<BinomKind> {
    match name {
        "choose" => Some(BinomKind::Paren),
        "brace" => Some(BinomKind::Brace),
        "brack" => Some(BinomKind::Brack),
        _ => None,
    }
}

/// The bracket kind of a `\genfrac` from its opening-delimiter argument, or `None` when the delimiter
/// is absent or unrecognised (the construct then falls back to verbatim).
fn genfrac_kind(left: &str) -> Option<BinomKind> {
    match left.trim() {
        "(" => Some(BinomKind::Paren),
        "[" => Some(BinomKind::Brack),
        "{" | "\\{" => Some(BinomKind::Brace),
        _ => None,
    }
}

/// Parse `\genfrac{ldelim}{rdelim}{thickness}{style}{num}{den}`: the opening delimiter selects the
/// surrounding bracket; the right-delimiter, rule-thickness and style groups are consumed. With no
/// opening delimiter the construct cannot be laid out and falls back to verbatim.
fn parse_genfrac(
    tokens: &[Token],
    pos: &mut usize,
    depth: usize,
    tail: &mut Option<String>,
) -> Option<Atom> {
    let left = parse_verbatim_group(tokens, pos, TextMode::Math)?;
    let kind = genfrac_kind(&text_pieces_to_string(&left))?;
    for _ in 0..3 {
        parse_required_group(tokens, pos, depth, tail)?;
    }
    let top = parse_required_group(tokens, pos, depth, tail)?;
    let bottom = parse_required_group(tokens, pos, depth, tail)?;
    Some(Atom::new(Body::Binom(kind, top, bottom)))
}

/// Parse an extensible arrow (`\xrightarrow`/`\xleftarrow`): an optional `[below]` label and a
/// required `{above}` label, set as scripts on the arrow glyph.
fn parse_extensible_arrow(
    name: &str,
    tokens: &[Token],
    pos: &mut usize,
    depth: usize,
    tail: &mut Option<String>,
) -> Option<Atom> {
    let arrow = if name == "xrightarrow" {
        "arrow.r"
    } else {
        "arrow.l"
    };
    let below = parse_optional_bracket(tokens, pos, depth);
    let above = parse_required_group(tokens, pos, depth, tail)?;
    Some(Atom::new(Body::ExtArrow(arrow, below, above)))
}

fn is_styled_command(name: &str) -> bool {
    matches!(
        name,
        "mathbb"
            | "mathcal"
            | "mathscr"
            | "mathfrak"
            | "mathbf"
            | "boldsymbol"
            | "bm"
            | "mathit"
            | "mathrm"
            | "mathsf"
            | "mathtt"
            | "pmb"
            // Composed styled alphabets: bold-italic, sans-italic, bold-sans-italic, and the
            // bold variants of the script and fraktur alphabets.
            | "mathbfit"
            | "mathsfit"
            | "mathbfsfit"
            | "mathbfcal"
            | "mathbfscr"
            | "mathbffrak"
            // Alternative spellings of the alphabet wrappers. `\mathds` is the double-struck
            // alphabet; `\symbf` is bold; `\mathup`/`\mathsfup` and `\mathbfup`/`\mathbfsfup` are the
            // explicitly-upright serif/sans and their bold variants. (Of the `\sym…` family only
            // `\symbf` carries a glyph change here; the others keep their default alphabet.)
            | "mathds"
            | "symbf"
            | "mathup"
            | "mathsfup"
            | "mathbfup"
            | "mathbfsfup"
            // Math-class wrappers carry no glyph of their own; they re-class their argument. In
            // linear output the class is invisible, so the argument renders transparently.
            | "mathord"
            | "mathrel"
            | "mathop"
            | "mathbin"
            | "mathopen"
            | "mathclose"
            | "mathpunct"
            // Presentation wrappers that translate only to Typst; in linear output they fall back to
            // verbatim because the styling has no inline equivalent.
            | "phantom"
            | "cancel"
            | "xcancel"
            | "bcancel"
            | "boxed"
            | "overparen"
            | "underparen"
    )
}

/// Text-mode commands: their argument is verbatim text, not math.
fn is_text_command(name: &str) -> bool {
    matches!(
        name,
        "text" | "textrm" | "textbf" | "textit" | "texttt" | "textsf" | "operatorname" | "mbox"
    )
}

/// Flatten a text-piece sequence to a plain string, rendering each spacing as its codepoint. Used
/// where a text-mode group is read only for a delimiter character, never re-emitted as formatted text.
fn text_pieces_to_string(pieces: &[TextPiece]) -> String {
    let mut out = String::new();
    for piece in pieces {
        match piece {
            TextPiece::Run(run) => out.push_str(run),
            TextPiece::Space(space) => out.push(space.codepoint()),
            // A `$…$` cannot occur in the math-mode groups this flattening serves.
            TextPiece::Math(_) => {}
        }
    }
    out
}

/// Finalize a completed text-wrapper run, applying TeX input-mode ligatures when the run is genuine
/// text (a `\text`/`\textbf`/… wrapper). An operator-name run is upright math text, which does not
/// run input ligatures, so it is returned unchanged.
fn finish_text_run(run: String, mode: TextMode) -> String {
    if mode == TextMode::Math {
        run
    } else {
        apply_text_ligatures(&run)
    }
}

/// Apply TeX's text-mode input ligatures to a literal character run. The TeX tokenizer folds straight
/// quote and dash runs into their curly/dash punctuation as the input is read, independent of any
/// smart-typography toggle:
///
/// - a run of backticks pairs into left double quotes `“` with a trailing lone backtick as a left
///   single quote `‘`; a run of apostrophes pairs into right double quotes `”` with a trailing lone
///   apostrophe as a right single quote `’`;
/// - a run of hyphens is consumed greedily, three at a time as an em dash `—`, then two as an en
///   dash `–`, then one as a literal hyphen.
///
/// Other characters pass through unchanged, so a run with no quote or dash is returned as written.
fn apply_text_ligatures(run: &str) -> String {
    let chars: Vec<char> = run.chars().collect();
    let mut out = String::with_capacity(run.len());
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        match c {
            '`' | '\'' => {
                let (open, single) = if c == '`' {
                    ('\u{201C}', '\u{2018}')
                } else {
                    ('\u{201D}', '\u{2019}')
                };
                let mut run_len = 1;
                while chars.get(i + run_len) == Some(&c) {
                    run_len += 1;
                }
                for _ in 0..run_len / 2 {
                    out.push(open);
                }
                if run_len % 2 == 1 {
                    out.push(single);
                }
                i += run_len;
            }
            '-' => {
                let mut run_len = 1;
                while chars.get(i + run_len) == Some(&'-') {
                    run_len += 1;
                }
                let mut remaining = run_len;
                while remaining >= 3 {
                    out.push('\u{2014}');
                    remaining -= 3;
                }
                if remaining == 2 {
                    out.push('\u{2013}');
                } else if remaining == 1 {
                    out.push('-');
                }
                i += run_len;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    out
}

/// The literal character a backslash-escape inside text mode unescapes to (`\%`→`%`, `\_`→`_`, …),
/// or `None` if the command is not a recognized text-mode escape.
fn text_escape_char(name: &str) -> Option<char> {
    let c = match name {
        "%" => '%',
        "&" => '&',
        "_" => '_',
        "$" => '$',
        "{" => '{',
        "}" => '}',
        "#" => '#',
        " " => ' ',
        _ => return None,
    };
    Some(c)
}

/// The spacing a backslash-escape inside text mode introduces (`\,`, `\;`, `\:`, `\!`), or `None`.
fn text_space(name: &str) -> Option<TextSpace> {
    let s = match name {
        "," => TextSpace::Thin,
        ";" => TextSpace::Thick,
        ":" => TextSpace::Medium,
        "!" => TextSpace::NegThin,
        _ => return None,
    };
    Some(s)
}

/// Whether `name` is a text-mode accent command that places a diacritic over the following character.
/// The accents recognized are those that compose with a Latin base to a single Unicode letter (acute,
/// grave, circumflex, diaeresis, tilde, macron, dot-above, breve, caron, cedilla).
fn is_text_accent_command(name: &str) -> bool {
    matches!(
        name,
        "'" | "`" | "^" | "\"" | "~" | "=" | "." | "u" | "v" | "c"
    )
}

/// The text-mode accent command applied to a base character, resolving to the composed Latin letter.
/// A base with no composed form for the accent returns the bare base character; an accent/base pair
/// outside the recognized set returns `None`, leaving the wrapper to fall back to verbatim.
// A flat accent-to-composed-letter lookup; the per-accent arms are one cohesive table with no
// shared logic to factor out, so splitting them into helpers would only scatter it.
#[allow(clippy::too_many_lines)]
fn text_accent(name: &str, base: char) -> Option<&'static str> {
    if !is_text_accent_command(name) {
        return None;
    }
    let table: &[(char, &str)] = match name {
        "'" => &[
            ('a', "\u{e1}"),
            ('c', "\u{107}"),
            ('e', "\u{e9}"),
            ('i', "\u{ed}"),
            ('l', "\u{13a}"),
            ('n', "\u{144}"),
            ('o', "\u{f3}"),
            ('r', "\u{155}"),
            ('s', "\u{15b}"),
            ('u', "\u{fa}"),
            ('y', "\u{fd}"),
            ('z', "\u{17a}"),
            ('A', "\u{c1}"),
            ('C', "\u{106}"),
            ('E', "\u{c9}"),
            ('I', "\u{cd}"),
            ('L', "\u{139}"),
            ('N', "\u{143}"),
            ('O', "\u{d3}"),
            ('R', "\u{154}"),
            ('S', "\u{15a}"),
            ('U', "\u{da}"),
            ('Y', "\u{dd}"),
            ('Z', "\u{179}"),
        ],
        "`" => &[
            ('a', "\u{e0}"),
            ('e', "\u{e8}"),
            ('i', "\u{ec}"),
            ('o', "\u{f2}"),
            ('u', "\u{f9}"),
            ('A', "\u{c0}"),
            ('E', "\u{c8}"),
            ('I', "\u{cc}"),
            ('O', "\u{d2}"),
            ('U', "\u{d9}"),
        ],
        "^" => &[
            ('a', "\u{e2}"),
            ('c', "\u{109}"),
            ('e', "\u{ea}"),
            ('g', "\u{11d}"),
            ('h', "\u{125}"),
            ('i', "\u{ee}"),
            ('j', "\u{135}"),
            ('o', "\u{f4}"),
            ('s', "\u{15d}"),
            ('u', "\u{fb}"),
            ('w', "\u{175}"),
            ('y', "\u{177}"),
            ('A', "\u{c2}"),
            ('C', "\u{108}"),
            ('E', "\u{ca}"),
            ('G', "\u{11c}"),
            ('H', "\u{124}"),
            ('I', "\u{ce}"),
            ('J', "\u{134}"),
            ('O', "\u{d4}"),
            ('S', "\u{15c}"),
            ('U', "\u{db}"),
            ('W', "\u{174}"),
            ('Y', "\u{176}"),
        ],
        "\"" => &[
            ('a', "\u{e4}"),
            ('e', "\u{eb}"),
            ('i', "\u{ef}"),
            ('o', "\u{f6}"),
            ('u', "\u{fc}"),
            ('A', "\u{c4}"),
            ('E', "\u{cb}"),
            ('I', "\u{cf}"),
            ('O', "\u{d6}"),
            ('U', "\u{dc}"),
        ],
        "~" => &[
            ('a', "\u{e3}"),
            ('i', "\u{129}"),
            ('n', "\u{f1}"),
            ('o', "\u{f5}"),
            ('u', "\u{169}"),
            ('A', "\u{c3}"),
            ('I', "\u{128}"),
            ('N', "\u{d1}"),
            ('O', "\u{d5}"),
            ('U', "\u{168}"),
        ],
        "=" => &[
            ('a', "\u{101}"),
            ('e', "\u{113}"),
            ('i', "\u{12b}"),
            ('o', "\u{14d}"),
            ('u', "\u{16b}"),
            ('A', "\u{100}"),
            ('E', "\u{112}"),
            ('I', "\u{12a}"),
            ('O', "\u{14c}"),
            ('U', "\u{16a}"),
        ],
        "." => &[
            ('c', "\u{10b}"),
            ('e', "\u{117}"),
            ('g', "\u{121}"),
            ('z', "\u{17c}"),
            ('C', "\u{10a}"),
            ('E', "\u{116}"),
            ('G', "\u{120}"),
            ('I', "\u{130}"),
            ('Z', "\u{17b}"),
        ],
        "u" => &[
            ('a', "\u{103}"),
            ('e', "\u{115}"),
            ('g', "\u{11f}"),
            ('i', "\u{12d}"),
            ('o', "\u{14f}"),
            ('u', "\u{16d}"),
            ('A', "\u{102}"),
            ('E', "\u{114}"),
            ('G', "\u{11e}"),
            ('I', "\u{12c}"),
            ('O', "\u{14e}"),
            ('U', "\u{16c}"),
        ],
        "v" => &[
            ('a', "\u{1ce}"),
            ('c', "\u{10d}"),
            ('d', "\u{10f}"),
            ('e', "\u{11b}"),
            ('g', "\u{1e7}"),
            ('h', "\u{21f}"),
            ('i', "\u{1d0}"),
            ('j', "\u{1f0}"),
            ('k', "\u{1e9}"),
            ('l', "\u{13e}"),
            ('n', "\u{148}"),
            ('o', "\u{1d2}"),
            ('r', "\u{159}"),
            ('s', "\u{161}"),
            ('t', "\u{165}"),
            ('u', "\u{1d4}"),
            ('z', "\u{17e}"),
            ('A', "\u{1cd}"),
            ('C', "\u{10c}"),
            ('D', "\u{10e}"),
            ('E', "\u{11a}"),
            ('G', "\u{1e6}"),
            ('H', "\u{21e}"),
            ('I', "\u{1cf}"),
            ('K', "\u{1e8}"),
            ('L', "\u{13d}"),
            ('N', "\u{147}"),
            ('O', "\u{1d1}"),
            ('R', "\u{158}"),
            ('S', "\u{160}"),
            ('T', "\u{164}"),
            ('U', "\u{1d3}"),
            ('Z', "\u{17d}"),
        ],
        "c" => &[
            ('c', "\u{e7}"),
            ('e', "\u{229}"),
            ('h', "\u{1e29}"),
            ('o', "o\u{327}"),
            ('s', "\u{15f}"),
            ('t', "\u{163}"),
            ('C', "\u{c7}"),
            ('E', "\u{228}"),
            ('H', "\u{1e28}"),
            ('O', "O\u{327}"),
            ('S', "\u{15e}"),
            ('T', "\u{162}"),
        ],
        _ => return None,
    };
    for &(b, composed) in table {
        if b == base {
            return Some(composed);
        }
    }
    // An accent over a base with no composed Latin letter drops the accent and keeps the base.
    BARE_BASE.get(&base).copied()
}

/// A single-character string for each ASCII letter that an accent may fall back to bare. Holding the
/// `&'static str` views here lets `text_accent` return a `'static` slice for the no-composition case.
static BARE_BASE: std::sync::LazyLock<std::collections::BTreeMap<char, &'static str>> =
    std::sync::LazyLock::new(|| {
        const LETTERS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
        LETTERS
            .char_indices()
            .filter_map(|(i, c)| Some((c, LETTERS.get(i..=i)?)))
            .collect()
    });

/// A foreign letter or ligature command resolved in a text wrapper (`\oe`→œ, `\ss`→ß, …), or `None`.
/// Only the commands that render to a single precomposed glyph are recognized; the rest fall back to
/// verbatim.
fn foreign_letter(name: &str) -> Option<char> {
    let c = match name {
        "oe" => '\u{153}',
        "OE" => '\u{152}',
        "ae" => '\u{e6}',
        "AE" => '\u{c6}',
        "o" => '\u{f8}',
        "O" => '\u{d8}',
        "ss" => '\u{df}',
        "l" => '\u{142}',
        "L" => '\u{141}',
        "aa" => '\u{e5}',
        "AA" => '\u{c5}',
        _ => return None,
    };
    Some(c)
}

/// A text-symbol command resolved in a text wrapper to its literal glyph, or `None`. Only the
/// commands that lower to an ordinary ASCII glyph are recognized.
fn text_symbol(name: &str) -> Option<char> {
    let c = match name {
        "textbackslash" => '\\',
        "textasciitilde" => '~',
        "textasciicircum" => '^',
        _ => return None,
    };
    Some(c)
}

/// The base character an accent command applies to, read from the tokens at `from`: a single bare
/// character, or a brace-wrapped single character, optionally preceded by one space. Returns the base
/// and the index just past it, or `None` if the following token is not a lone character.
fn accent_base(tokens: &[Token], from: usize) -> Option<(char, usize)> {
    let mut i = from;
    if matches!(tokens.get(i), Some(Token::Space)) {
        i += 1;
    }
    match tokens.get(i)? {
        Token::Char(c) => Some((*c, i + 1)),
        Token::GroupOpen => {
            let inner = tokens.get(i + 1)?;
            let base = match inner {
                Token::Char(c) => *c,
                _ => return None,
            };
            if matches!(tokens.get(i + 2), Some(Token::GroupClose)) {
                Some((base, i + 3))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// The literal source of a dotless-letter control word (`\i`, `\j`) following an accent, read from the
/// tokens at `from` (after an optional leading space), with the index just past it. These bases have no
/// composed accented form, so the accent is dropped and the control word's source is kept verbatim.
fn accent_dotless_base(tokens: &[Token], from: usize) -> Option<(&'static str, usize)> {
    let mut i = from;
    if matches!(tokens.get(i), Some(Token::Space)) {
        i += 1;
    }
    match tokens.get(i)? {
        Token::Command(name) if name == "i" => Some(("\\i", i + 1)),
        Token::Command(name) if name == "j" => Some(("\\j", i + 1)),
        _ => None,
    }
}

/// Read a `{...}` group as text-mode content. Recognized backslash-escapes unescape to their literal
/// character; `~` becomes a non-breaking space; the spacing escapes (`\,`, `\;`, `\:`, `\!`) become
/// spacing pieces that break the run (unless `fold_spacing`, in which case they collapse into the run
/// as their spacing codepoint). Returns `None` if the group contains a math-mode switch, an
/// unrecognized control sequence, or a nested script/group, which we do not attempt to render.
// The `$` arm must precede the general-character arm, so it cannot fold into the unhandled-token arm.
#[allow(clippy::match_same_arms, clippy::too_many_lines)]
fn parse_verbatim_group(
    tokens: &[Token],
    pos: &mut usize,
    mode: TextMode,
) -> Option<Vec<TextPiece>> {
    skip_spaces(tokens, pos);
    if !matches!(tokens.get(*pos), Some(Token::GroupOpen)) {
        return None;
    }
    let math = mode == TextMode::Math;
    let mut probe = *pos + 1;
    let mut pieces: Vec<TextPiece> = Vec::new();
    let mut run = String::new();
    while let Some(tok) = tokens.get(probe) {
        // Whether a control word or active `~` at this position should swallow a following space.
        let following_space = matches!(tokens.get(probe + 1), Some(Token::Space));
        match tok {
            Token::GroupClose => {
                *pos = probe + 1;
                if !run.is_empty() {
                    pieces.push(TextPiece::Run(finish_text_run(run, mode)));
                }
                return Some(pieces);
            }
            // A `$…$` inside a text wrapper switches back to math mode for its content. An
            // operator-name group is already math, so a `$` there has no meaning and is unhandled.
            Token::Char('$') if math => return None,
            Token::Char('$') => {
                let inner_start = probe + 1;
                let inner_end = (inner_start..tokens.len())
                    .find(|&i| matches!(tokens.get(i), Some(Token::Char('$'))))?;
                let inner = tokens.get(inner_start..inner_end)?;
                let mut inner_pos = 0;
                let atoms = parse_atoms(inner, &mut inner_pos, 0, false)?;
                if inner_pos != inner.len() {
                    return None;
                }
                if !run.is_empty() {
                    pieces.push(TextPiece::Run(finish_text_run(
                        std::mem::take(&mut run),
                        mode,
                    )));
                }
                pieces.push(TextPiece::Math(atoms));
                probe = inner_end;
            }
            // `~` is an inter-word non-breaking space in both modes; in math mode it swallows a
            // following space the way a control word does.
            Token::Char('~') => {
                run.push('\u{00A0}');
                if math && following_space {
                    probe += 1;
                }
            }
            // A bare double quote or backtick is unparsable in math mode (here, `\operatorname`
            // content), so the whole expression falls back to verbatim. In a text wrapper both are
            // ordinary literal characters.
            Token::Char('"' | '`') if math => return None,
            Token::Char(c) => run.push(*c),
            Token::Number(digits) => run.push_str(digits),
            // Math-mode inter-token space is not significant; a text wrapper keeps it literal.
            Token::Space => {
                if !math {
                    run.push(' ');
                }
            }
            // Subscript/superscript markers are literal characters in text mode.
            Token::Sub => run.push('_'),
            Token::Sup => run.push('^'),
            // `\(…\)` and `\[…\]` inside a text wrapper switch back to math mode for their content,
            // the same as an inline `$…$`. The content renders as math and splices in among the
            // literal runs. An operator-name group is already math, so the markers have no special
            // meaning there.
            Token::Command(name) if !math && (name == "(" || name == "[") => {
                let close = if name == "(" { ")" } else { "]" };
                let inner_start = probe + 1;
                let inner_end = (inner_start..tokens.len())
                    .find(|&i| matches!(tokens.get(i), Some(Token::Command(c)) if c == close))?;
                let inner = tokens.get(inner_start..inner_end)?;
                let mut inner_pos = 0;
                let atoms = parse_atoms(inner, &mut inner_pos, 0, false)?;
                if inner_pos != inner.len() {
                    return None;
                }
                if !run.is_empty() {
                    pieces.push(TextPiece::Run(finish_text_run(
                        std::mem::take(&mut run),
                        mode,
                    )));
                }
                pieces.push(TextPiece::Math(atoms));
                probe = inner_end;
            }
            Token::Command(name) => {
                probe = apply_text_command(
                    TextCommand {
                        name,
                        mode,
                        math,
                        following_space,
                    },
                    tokens,
                    probe,
                    &mut run,
                    &mut pieces,
                )?;
            }
            // In a text wrapper an inner brace group is transparent: its content joins the surrounding
            // run, with ligatures kept within each group. In math mode a nested group is unhandled.
            Token::GroupOpen if !math => {
                if !run.is_empty() {
                    pieces.push(TextPiece::Run(finish_text_run(
                        std::mem::take(&mut run),
                        mode,
                    )));
                }
                let mut nested = probe;
                let inner = parse_verbatim_group(tokens, &mut nested, mode)?;
                if inner.is_empty() {
                    // An empty inner group is still a distinct (empty) text segment.
                    pieces.push(TextPiece::Run(String::new()));
                } else {
                    pieces.extend(inner);
                }
                probe = nested - 1;
            }
            // A nested group inside a math-mode wrapper is not handled.
            Token::GroupOpen => return None,
        }
        probe += 1;
    }
    None
}

/// The context a control word inside a verbatim group is resolved in.
#[derive(Clone, Copy)]
struct TextCommand<'a> {
    name: &'a str,
    mode: TextMode,
    /// Whether the wrapper is math mode (`\operatorname`) rather than a text wrapper.
    math: bool,
    /// Whether a space immediately follows, which a control word swallows as in TeX.
    following_space: bool,
}

/// Resolve a control word inside a verbatim group, appending its rendering to `run` (or flushing a
/// space piece to `pieces`), and return the probe position the scan should continue from — the
/// caller's loop advances one further past it. Returns `None` for a command the wrapper does not
/// resolve, leaving the whole expression to fall back to verbatim.
fn apply_text_command(
    ctx: TextCommand<'_>,
    tokens: &[Token],
    probe: usize,
    run: &mut String,
    pieces: &mut Vec<TextPiece>,
) -> Option<usize> {
    let TextCommand {
        name,
        mode,
        math,
        following_space,
    } = ctx;
    // A control symbol or word that resolves to a single glyph swallows one following run of spaces.
    let swallow = |probe: usize| if following_space { probe + 1 } else { probe };
    if let Some(c) = text_escape_char(name) {
        // The escaped space is a non-breaking space in math mode, an ordinary one in text.
        let pushed = if c == ' ' && math { '\u{00A0}' } else { c };
        run.push(pushed);
        Some(swallow(probe))
    } else if name == "ldots" {
        // `\ldots` is the one ellipsis command a text-mode wrapper resolves: it folds the ellipsis
        // glyph into the run. Other ellipsis commands (`\dots`, `\cdots`, `\textellipsis`) are not
        // recognized here and leave the expression verbatim.
        run.push('\u{2026}');
        Some(swallow(probe))
    } else if let Some(space) = text_space(name) {
        if math {
            run.push(space.codepoint());
            Some(swallow(probe))
        } else {
            if !run.is_empty() {
                pieces.push(TextPiece::Run(finish_text_run(std::mem::take(run), mode)));
            }
            pieces.push(TextPiece::Space(space));
            Some(probe)
        }
    } else if !math && is_text_accent_command(name) {
        // An accent over a dotless-letter control word (`\i`, `\j`) has no composed form: the accent
        // is dropped and the control word is emitted as its literal source.
        if let Some((literal, next)) = accent_dotless_base(tokens, probe + 1) {
            run.push_str(literal);
            return Some(next - 1);
        }
        // A text-mode accent composes with its following base into a single Latin letter; an
        // unparsable base leaves the whole wrapper verbatim.
        let (base, next) = accent_base(tokens, probe + 1)?;
        run.push_str(text_accent(name, base)?);
        Some(next - 1)
    } else if !math && let Some(c) = foreign_letter(name) {
        run.push(c);
        Some(swallow(probe))
    } else if !math && let Some(c) = text_symbol(name) {
        run.push(c);
        Some(swallow(probe))
    } else if !math && name == "quad" {
        // A text-mode `\quad` folds into the run as a single en quad.
        run.push('\u{2000}');
        Some(swallow(probe))
    } else {
        None
    }
}

/// Parse a required `{...}` group, advancing past optional leading spaces. An unbraced argument is a
/// single token: a multi-digit number gives up only its first digit and leaves the rest in `tail` for
/// the command's next argument or the enclosing run to place.
fn parse_required_group(
    tokens: &[Token],
    pos: &mut usize,
    depth: usize,
    tail: &mut Option<String>,
) -> Option<Vec<Atom>> {
    if depth > MAX_DEPTH {
        return None;
    }
    // A previous argument of the same command took one digit of an unbraced multi-digit number and
    // left the rest here; this argument takes the next digit of it. (`\frac12` → `1` over `2`.)
    if let Some(rest) = tail.take() {
        let mut chars = rest.chars();
        let first = chars.next()?;
        let remainder: String = chars.collect();
        if !remainder.is_empty() {
            *tail = Some(remainder);
        }
        return Some(vec![Atom::new(Body::Number(first.to_string()))]);
    }
    skip_spaces(tokens, pos);
    match tokens.get(*pos)? {
        Token::GroupOpen => {
            *pos += 1;
            parse_atoms(tokens, pos, depth + 1, true)
        }
        // An unbraced number gives up only its first digit; any further digits stay in `tail` for the
        // next argument or the enclosing run. (`\sqrt12` → `\sqrt1` then a loose `2`.)
        Token::Number(digits) => {
            let mut chars = digits.chars();
            let first = chars.next()?;
            let remainder: String = chars.collect();
            *pos += 1;
            if !remainder.is_empty() {
                *tail = Some(remainder);
            }
            Some(vec![Atom::new(Body::Number(first.to_string()))])
        }
        // A single token also serves as the argument (e.g. `\bar x`).
        Token::Char(_) | Token::Command(_) => {
            let mut group = Vec::new();
            parse_atom_into(tokens, pos, depth + 1, &mut group)?;
            Some(group)
        }
        _ => None,
    }
}

/// Parse an optional `[...]` bracket argument (used by `\sqrt[n]{..}`); the bracketed content is
/// read as raw atoms.
fn parse_optional_bracket(tokens: &[Token], pos: &mut usize, depth: usize) -> Option<Vec<Atom>> {
    skip_spaces(tokens, pos);
    if !matches!(tokens.get(*pos), Some(Token::Char('['))) {
        return None;
    }
    let mut probe = *pos + 1;
    let mut inner: Vec<Atom> = Vec::new();
    while let Some(tok) = tokens.get(probe) {
        if matches!(tok, Token::Char(']')) {
            *pos = probe + 1;
            return Some(inner);
        }
        let mut local = probe;
        // A bracketed optional argument keeps its numbers whole (`\sqrt[12]{x}`), so any leftover
        // digits a nested command produces stay with this argument rather than escaping the bracket.
        let mut bracket_tail = None;
        let atom = parse_atom(tokens, &mut local, depth + 1, &mut bracket_tail)?;
        if local == probe {
            return None;
        }
        probe = local;
        inner.push(atom);
        if let Some(rest) = bracket_tail {
            inner.push(Atom::new(Body::Number(rest)));
        }
    }
    None
}

fn skip_spaces(tokens: &[Token], pos: &mut usize) {
    while matches!(tokens.get(*pos), Some(Token::Space)) {
        *pos += 1;
    }
}

/// Read a `\label{name}` argument as a flat verbatim run, returning its reconstructed source text and
/// advancing past the closing brace. The argument must be a single flat group: a nested `{…}` (e.g.
/// `\label{\sqrt{x}}`) leaves the whole expression unhandled and is reported as `None`. The name keeps
/// its source spelling — a control word is rebuilt with its backslash (`\alpha`), an escaped character
/// keeps the escape — so an inner `$`, command, or space round-trips into the label verbatim. A
/// missing opening brace yields `Some(None)`: the `\label` is consumed but carries no name.
//
// The nested option is load-bearing: the outer layer drives `?`-propagation of the
// unhandled-expression fallback shared across the parser (a flattened type would forfeit that), while
// the inner layer reports whether a name was present.
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

/// Consume and discard a row break's optional `[<dim>]` extra-space argument. The bracket binds only
/// when it immediately follows the `\\` with no intervening space; a `[` after a space is ordinary
/// content (the start of the next row) and is left untouched.
fn skip_optional_break_dim(tokens: &[Token], pos: &mut usize) {
    if !matches!(tokens.get(*pos), Some(Token::Char('['))) {
        return;
    }
    let mut probe = *pos + 1;
    while let Some(tok) = tokens.get(probe) {
        if matches!(tok, Token::Char(']')) {
            *pos = probe + 1;
            return;
        }
        probe += 1;
    }
}

/// Consume a balanced `{…}` group without interpreting its contents, advancing past leading spaces.
/// Returns `None` if no group is present or it is unbalanced.
fn skip_balanced_group(tokens: &[Token], pos: &mut usize) -> Option<()> {
    skip_spaces(tokens, pos);
    if !matches!(tokens.get(*pos), Some(Token::GroupOpen)) {
        return None;
    }
    *pos += 1;
    let mut depth = 1u32;
    while let Some(tok) = tokens.get(*pos) {
        *pos += 1;
        match tok {
            Token::GroupOpen => depth += 1,
            Token::GroupClose => {
                depth -= 1;
                if depth == 0 {
                    return Some(());
                }
            }
            _ => {}
        }
    }
    None
}
