//! Golden fixtures for the MathML backend: enclosures, matrices, fences, modulo, negation, and display forms.

pub(super) const GOLDENS: &[(&str, bool, Option<&str>)] = &[
    // the cancel family and `\boxed` use `<menclose>` with the matching notation
    (
        "\\cancel{x}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><menclose notation=\"updiagonalstrike\"><mi>x</mi></menclose></math>",
        ),
    ),
    (
        "\\bcancel{x}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><menclose notation=\"downdiagonalstrike\"><mi>x</mi></menclose></math>",
        ),
    ),
    (
        "\\xcancel{x}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><menclose notation=\"updiagonalstrike downdiagonalstrike\"><mi>x</mi></menclose></math>",
        ),
    ),
    (
        "\\boxed{x}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><menclose notation=\"box\"><mi>x</mi></menclose></math>",
        ),
    ),
    // binomials, matrices with every delimiter, arrays, cases, and each alignment grid's
    // column-alignment cycle
    (
        "\\binom{n}{k}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mo stretchy=\"true\" form=\"prefix\">(</mo><mfrac linethickness=\"0\"><mi>n</mi><mi>k</mi></mfrac><mo stretchy=\"true\" form=\"postfix\">)</mo></mrow></math>",
        ),
    ),
    (
        "\\begin{matrix}a&b\\\\c&d\\end{matrix}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mtable><mtr><mtd columnalign=\"center\" style=\"text-align: center\"><mi>a</mi></mtd><mtd columnalign=\"center\" style=\"text-align: center\"><mi>b</mi></mtd></mtr><mtr><mtd columnalign=\"center\" style=\"text-align: center\"><mi>c</mi></mtd><mtd columnalign=\"center\" style=\"text-align: center\"><mi>d</mi></mtd></mtr></mtable></math>",
        ),
    ),
    (
        "\\begin{pmatrix}a&b\\\\c&d\\end{pmatrix}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mo stretchy=\"true\" form=\"prefix\">(</mo><mtable><mtr><mtd columnalign=\"center\" style=\"text-align: center\"><mi>a</mi></mtd><mtd columnalign=\"center\" style=\"text-align: center\"><mi>b</mi></mtd></mtr><mtr><mtd columnalign=\"center\" style=\"text-align: center\"><mi>c</mi></mtd><mtd columnalign=\"center\" style=\"text-align: center\"><mi>d</mi></mtd></mtr></mtable><mo stretchy=\"true\" form=\"postfix\">)</mo></mrow></math>",
        ),
    ),
    (
        "\\begin{bmatrix}a&b\\end{bmatrix}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mo stretchy=\"true\" form=\"prefix\">[</mo><mtable><mtr><mtd columnalign=\"center\" style=\"text-align: center\"><mi>a</mi></mtd><mtd columnalign=\"center\" style=\"text-align: center\"><mi>b</mi></mtd></mtr></mtable><mo stretchy=\"true\" form=\"postfix\">]</mo></mrow></math>",
        ),
    ),
    (
        "\\begin{Bmatrix}a&b\\end{Bmatrix}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mo stretchy=\"true\" form=\"prefix\">{</mo><mtable><mtr><mtd columnalign=\"center\" style=\"text-align: center\"><mi>a</mi></mtd><mtd columnalign=\"center\" style=\"text-align: center\"><mi>b</mi></mtd></mtr></mtable><mo stretchy=\"true\" form=\"postfix\">}</mo></mrow></math>",
        ),
    ),
    (
        "\\begin{vmatrix}a&b\\end{vmatrix}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mo stretchy=\"true\" form=\"prefix\">|</mo><mtable><mtr><mtd columnalign=\"center\" style=\"text-align: center\"><mi>a</mi></mtd><mtd columnalign=\"center\" style=\"text-align: center\"><mi>b</mi></mtd></mtr></mtable><mo stretchy=\"true\" form=\"postfix\">|</mo></mrow></math>",
        ),
    ),
    (
        "\\begin{Vmatrix}a&b\\end{Vmatrix}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mo stretchy=\"true\" form=\"prefix\">‖</mo><mtable><mtr><mtd columnalign=\"center\" style=\"text-align: center\"><mi>a</mi></mtd><mtd columnalign=\"center\" style=\"text-align: center\"><mi>b</mi></mtd></mtr></mtable><mo stretchy=\"true\" form=\"postfix\">‖</mo></mrow></math>",
        ),
    ),
    (
        "\\begin{array}{lcr}a&b&c\\\\d&e&f\\end{array}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mtable><mtr><mtd columnalign=\"left\" style=\"text-align: left\"><mi>a</mi></mtd><mtd columnalign=\"center\" style=\"text-align: center\"><mi>b</mi></mtd><mtd columnalign=\"right\" style=\"text-align: right\"><mi>c</mi></mtd></mtr><mtr><mtd columnalign=\"left\" style=\"text-align: left\"><mi>d</mi></mtd><mtd columnalign=\"center\" style=\"text-align: center\"><mi>e</mi></mtd><mtd columnalign=\"right\" style=\"text-align: right\"><mi>f</mi></mtd></mtr></mtable></math>",
        ),
    ),
    (
        "\\begin{cases}a&x>0\\\\b&x<0\\end{cases}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mo stretchy=\"true\" form=\"prefix\">{</mo><mtable><mtr><mtd columnalign=\"left\" style=\"text-align: left\"><mi>a</mi></mtd><mtd columnalign=\"left\" style=\"text-align: left\"><mi>x</mi><mo>&gt;</mo><mn>0</mn></mtd></mtr><mtr><mtd columnalign=\"left\" style=\"text-align: left\"><mi>b</mi></mtd><mtd columnalign=\"left\" style=\"text-align: left\"><mi>x</mi><mo>&lt;</mo><mn>0</mn></mtd></mtr></mtable></mrow></math>",
        ),
    ),
    (
        "\\begin{aligned}a&=b\\\\c&=d\\end{aligned}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mtable><mtr><mtd columnalign=\"right\" style=\"text-align: right; padding-right: 0\"><mi>a</mi></mtd><mtd columnalign=\"left\" style=\"text-align: left; padding-left: 0\"><mo>=</mo><mi>b</mi></mtd></mtr><mtr><mtd columnalign=\"right\" style=\"text-align: right; padding-right: 0\"><mi>c</mi></mtd><mtd columnalign=\"left\" style=\"text-align: left; padding-left: 0\"><mo>=</mo><mi>d</mi></mtd></mtr></mtable></math>",
        ),
    ),
    (
        "\\begin{eqnarray}a&=&b\\end{eqnarray}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mtable><mtr><mtd columnalign=\"right\" style=\"text-align: right\"><mi>a</mi></mtd><mtd columnalign=\"center\" style=\"text-align: center\"><mo>=</mo></mtd><mtd columnalign=\"left\" style=\"text-align: left\"><mi>b</mi></mtd></mtr></mtable></math>",
        ),
    ),
    (
        "\\begin{flalign}a&=b\\end{flalign}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mtable><mtr><mtd columnalign=\"left\" style=\"text-align: left\"><mi>a</mi></mtd><mtd columnalign=\"right\" style=\"text-align: right\"><mo>=</mo><mi>b</mi></mtd></mtr></mtable></math>",
        ),
    ),
    (
        "\\begin{gathered}a\\\\b\\end{gathered}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mtable><mtr><mtd columnalign=\"center\" style=\"text-align: center\"><mi>a</mi></mtd></mtr><mtr><mtd columnalign=\"center\" style=\"text-align: center\"><mi>b</mi></mtd></mtr></mtable></math>",
        ),
    ),
    (
        "x_{\\substack{a\\\\b}}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><msub><mi>x</mi><mtable><mtr><mtd columnalign=\"center\" style=\"text-align: center\"><mi>a</mi></mtd></mtr><mtr><mtd columnalign=\"center\" style=\"text-align: center\"><mi>b</mi></mtd></mtr></mtable></msub></math>",
        ),
    ),
    // explicit fences (stretchy sides, dropped `.`, `\middle`) and the plain-glyph bracket macros
    (
        "\\left(a\\right)",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mo stretchy=\"true\" form=\"prefix\">(</mo><mi>a</mi><mo stretchy=\"true\" form=\"postfix\">)</mo></mrow></math>",
        ),
    ),
    (
        "\\left[a\\right]",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mo stretchy=\"true\" form=\"prefix\">[</mo><mi>a</mi><mo stretchy=\"true\" form=\"postfix\">]</mo></mrow></math>",
        ),
    ),
    (
        "\\left\\{a\\right\\}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mo stretchy=\"true\" form=\"prefix\">{</mo><mi>a</mi><mo stretchy=\"true\" form=\"postfix\">}</mo></mrow></math>",
        ),
    ),
    (
        "\\left|a\\right|",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mo stretchy=\"true\" form=\"prefix\">|</mo><mi>a</mi><mo stretchy=\"true\" form=\"postfix\">|</mo></mrow></math>",
        ),
    ),
    (
        "\\left.a\\right)",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mi>a</mi><mo stretchy=\"true\" form=\"postfix\">)</mo></mrow></math>",
        ),
    ),
    (
        "\\langle a\\rangle",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mo>⟨</mo><mi>a</mi><mo>⟩</mo></mrow></math>",
        ),
    ),
    (
        "\\lfloor a\\rfloor",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mo>⌊</mo><mi>a</mi><mo>⌋</mo></mrow></math>",
        ),
    ),
    (
        "\\lceil a\\rceil",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mo>⌈</mo><mi>a</mi><mo>⌉</mo></mrow></math>",
        ),
    ),
    (
        "\\|a\\|",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mi>∥</mi><mi>a</mi><mi>∥</mi></mrow></math>",
        ),
    ),
    (
        "\\left(a\\middle|b\\right)",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mo stretchy=\"true\" form=\"prefix\">(</mo><mi>a</mi><mo>|</mo><mi>b</mi><mo stretchy=\"true\" form=\"postfix\">)</mo></mrow></math>",
        ),
    ),
    // sized delimiters carry percentage min/max; opener is prefix, relation glyphs take no fence
    // form, ordinary glyphs stay `<mi>`
    (
        "\\big(",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mo minsize=\"120%\" maxsize=\"120%\" stretchy=\"true\" form=\"prefix\">(</mo></math>",
        ),
    ),
    (
        "\\bigg[",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mo minsize=\"240%\" maxsize=\"240%\" stretchy=\"true\" form=\"prefix\">[</mo></math>",
        ),
    ),
    (
        "\\big\\|",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mi minsize=\"120%\" maxsize=\"120%\" stretchy=\"true\">∥</mi></math>",
        ),
    ),
    (
        "\\big=",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mo minsize=\"120%\" maxsize=\"120%\" stretchy=\"true\">=</mo></math>",
        ),
    ),
    (
        "\\big/",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mi minsize=\"120%\" maxsize=\"120%\" stretchy=\"true\">/</mi></math>",
        ),
    ),
    // Modulo forms: `\bmod` and `\mod` set the operator inline, `\pmod`/`\pod` parenthesise it.
    (
        "a\\bmod b",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mi>a</mi><mrow><mspace width=\"0.222em\"></mspace><mrow><mi mathvariant=\"normal\">mod</mi><mo>\u{2061}</mo></mrow><mspace width=\"0.222em\"></mspace></mrow><mi>b</mi></mrow></math>",
        ),
    ),
    (
        "a\\mod b",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mi>a</mi><mrow><mspace width=\"0.444em\"></mspace><mrow><mi mathvariant=\"normal\">mod</mi><mo>\u{2061}</mo></mrow><mspace width=\"0.222em\"></mspace></mrow><mi>b</mi></mrow></math>",
        ),
    ),
    (
        "a\\pmod{n}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mi>a</mi><mrow><mspace width=\"0.222em\"></mspace><mo stretchy=\"true\" form=\"prefix\">(</mo><mrow><mi mathvariant=\"normal\">mod</mi><mo>\u{2061}</mo></mrow><mspace width=\"0.222em\"></mspace><mi>n</mi><mo stretchy=\"true\" form=\"postfix\">)</mo></mrow></mrow></math>",
        ),
    ),
    (
        "a\\pod{n}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mi>a</mi><mrow><mspace width=\"0.222em\"></mspace><mo stretchy=\"true\" form=\"prefix\">(</mo><mi>n</mi><mo stretchy=\"true\" form=\"postfix\">)</mo></mrow></mrow></math>",
        ),
    ),
    // negation: precomposed relation, combining solidus over relation/letter/digit, struck Greek
    (
        "\\not= b",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mo>≠</mo><mi>b</mi></mrow></math>",
        ),
    ),
    (
        "\\not\\vdash",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mo>⊢\u{338}</mo></math>",
        ),
    ),
    (
        "\\not a",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mi>a\u{338}</mi></math>",
        ),
    ),
    (
        "\\not 1",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mn>1\u{338}</mn></math>",
        ),
    ),
    (
        "\\not\\alpha",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mi>α\u{338}</mi></math>",
        ),
    ),
    // stacks, horizontal braces (bare, labelled, ordinary script pair), and extensible arrows
    (
        "\\overset{a}{b}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mover><mi>b</mi><mi>a</mi></mover></math>",
        ),
    ),
    (
        "\\underset{a}{b}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><munder><mi>b</mi><mi>a</mi></munder></math>",
        ),
    ),
    (
        "\\overbrace{a+b}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mover><mrow><mi>a</mi><mo>+</mo><mi>b</mi></mrow><mo accent=\"true\">⏞</mo></mover></math>",
        ),
    ),
    (
        "\\underbrace{a+b}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><munder><mrow><mi>a</mi><mo>+</mo><mi>b</mi></mrow><mo accent=\"true\">⏟</mo></munder></math>",
        ),
    ),
    (
        "\\overbrace{a+b}^{n}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mover><mover><mrow><mi>a</mi><mo>+</mo><mi>b</mi></mrow><mo accent=\"true\">⏞</mo></mover><mi>n</mi></mover></math>",
        ),
    ),
    (
        "\\underbrace{a+b}_{n}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><munder><munder><mrow><mi>a</mi><mo>+</mo><mi>b</mi></mrow><mo accent=\"true\">⏟</mo></munder><mi>n</mi></munder></math>",
        ),
    ),
    (
        "\\overbrace{a}_c^d",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><msub><mover><mover><mi>a</mi><mo accent=\"true\">⏞</mo></mover><mi>d</mi></mover><mi>c</mi></msub></math>",
        ),
    ),
    (
        "\\xrightarrow{f}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mover><mo>→</mo><mi>f</mi></mover></math>",
        ),
    ),
    (
        "\\xleftarrow{f}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mover><mo>←</mo><mi>f</mi></mover></math>",
        ),
    ),
    (
        "\\xrightarrow[b]{a}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><munderover><mo>→</mo><mi>b</mi><mi>a</mi></munderover></math>",
        ),
    ),
    // Whitespace-only and empty input lower to an empty `<math>`, a successful (empty) conversion.
    (
        "",
        false,
        Some("<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"></math>"),
    ),
    (
        "   ",
        false,
        Some("<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"></math>"),
    ),
    // display mode: root carries `display=\"block\"`; limit operators stack their scripts,
    // integrals keep them beside
    (
        "x^2",
        true,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"block\"><msup><mi>x</mi><mn>2</mn></msup></math>",
        ),
    ),
    (
        "\\sum_a^b x",
        true,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"block\"><mrow><munderover><mo>∑</mo><mi>a</mi><mi>b</mi></munderover><mi>x</mi></mrow></math>",
        ),
    ),
    (
        "\\prod_a^b",
        true,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"block\"><munderover><mo>∏</mo><mi>a</mi><mi>b</mi></munderover></math>",
        ),
    ),
    (
        "\\lim_x",
        true,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"block\"><munder><mi>lim</mi><mi>x</mi></munder></math>",
        ),
    ),
    (
        "\\int_a^b f",
        true,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"block\"><mrow><msubsup><mo>∫</mo><mi>a</mi><mi>b</mi></msubsup><mi>f</mi></mrow></math>",
        ),
    ),
    (
        "\\bigcup_a^b",
        true,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"block\"><munderover><mo>⋃</mo><mi>a</mi><mi>b</mi></munderover></math>",
        ),
    ),
    (
        "\\frac{a}{b}",
        true,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"block\"><mfrac><mi>a</mi><mi>b</mi></mfrac></math>",
        ),
    ),
    (
        "∑_i^n",
        true,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"block\"><munderover><mi>∑</mi><mi>i</mi><mi>n</mi></munderover></math>",
        ),
    ),
];
