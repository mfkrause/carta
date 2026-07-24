//! Golden fixtures for the MathML backend: atoms, scripts, fractions, accents, text, and font styles.

pub(super) const GOLDENS: &[(&str, bool, Option<&str>)] = &[
    // digits `<mn>`, letters `<mi>`, operator classes `<mo>`; hyphen prints minus; `:=` is one relation
    (
        "x",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mi>x</mi></math>",
        ),
    ),
    (
        "1",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mn>1</mn></math>",
        ),
    ),
    (
        "12",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mn>12</mn></math>",
        ),
    ),
    (
        "a+b",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mi>a</mi><mo>+</mo><mi>b</mi></mrow></math>",
        ),
    ),
    (
        "a-b",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mi>a</mi><mo>−</mo><mi>b</mi></mrow></math>",
        ),
    ),
    (
        "a*b",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mi>a</mi><mo>*</mo><mi>b</mi></mrow></math>",
        ),
    ),
    (
        "a=b",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mi>a</mi><mo>=</mo><mi>b</mi></mrow></math>",
        ),
    ),
    (
        "a<b",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mi>a</mi><mo>&lt;</mo><mi>b</mi></mrow></math>",
        ),
    ),
    (
        "a>b",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mi>a</mi><mo>&gt;</mo><mi>b</mi></mrow></math>",
        ),
    ),
    (
        "a,b",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mi>a</mi><mo>,</mo><mi>b</mi></mrow></math>",
        ),
    ),
    (
        "a;b",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mi>a</mi><mo>;</mo><mi>b</mi></mrow></math>",
        ),
    ),
    (
        "(a)",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mo>(</mo><mi>a</mi><mo>)</mo></mrow></math>",
        ),
    ),
    (
        "[a]",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mo>[</mo><mi>a</mi><mo>]</mo></mrow></math>",
        ),
    ),
    (
        "|a|",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mo>|</mo><mi>a</mi><mo>|</mo></mrow></math>",
        ),
    ),
    (
        "a:=b",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mi>a</mi><mo>:=</mo><mi>b</mi></mrow></math>",
        ),
    ),
    // Greek, symbols, named functions, spacings, and an unknown command as a literal `<mi>`
    (
        "\\alpha",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mi>α</mi></math>",
        ),
    ),
    (
        "\\infty",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mi>∞</mi></math>",
        ),
    ),
    (
        "\\sin",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mi>sin</mi></math>",
        ),
    ),
    (
        "\\lim",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mi>lim</mi></math>",
        ),
    ),
    (
        "\\,",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mspace width=\"0.167em\"></mspace></math>",
        ),
    ),
    (
        "\\:",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mspace width=\"0.222em\"></mspace></math>",
        ),
    ),
    (
        "\\;",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mspace width=\"0.278em\"></mspace></math>",
        ),
    ),
    (
        "\\!",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mspace width=\"-0.167em\"></mspace></math>",
        ),
    ),
    (
        "\\quad",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mspace width=\"1em\"></mspace></math>",
        ),
    ),
    (
        "\\qquad",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mspace width=\"2em\"></mspace></math>",
        ),
    ),
    (
        "\\enspace",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mspace width=\"0.5em\"></mspace></math>",
        ),
    ),
    (
        "\\thinspace",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mspace width=\"0.167em\"></mspace></math>",
        ),
    ),
    (
        "\\medspace",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mspace width=\"0.222em\"></mspace></math>",
        ),
    ),
    (
        "\\thickspace",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mspace width=\"0.278em\"></mspace></math>",
        ),
    ),
    (
        "\\negthinspace",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mspace width=\"-0.167em\"></mspace></math>",
        ),
    ),
    (
        "\\thiscommanddoesnotexist",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mi>thiscommanddoesnotexist</mi></math>",
        ),
    ),
    // Primes: the precomposed run up to four marks, then repeated single primes past it.
    (
        "f'",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><msup><mi>f</mi><mo>′</mo></msup></math>",
        ),
    ),
    (
        "f''",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><msup><mi>f</mi><mo>″</mo></msup></math>",
        ),
    ),
    (
        "f'''",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><msup><mi>f</mi><mo>‴</mo></msup></math>",
        ),
    ),
    (
        "f''''",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><msup><mi>f</mi><mo>⁗</mo></msup></math>",
        ),
    ),
    (
        "f'''''",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><msup><mi>f</mi><mo>′′′′′</mo></msup></math>",
        ),
    ),
    // Groups and scripts, beside the base.
    (
        "{ab}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mi>a</mi><mi>b</mi></mrow></math>",
        ),
    ),
    (
        "x^2",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><msup><mi>x</mi><mn>2</mn></msup></math>",
        ),
    ),
    (
        "x_i",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><msub><mi>x</mi><mi>i</mi></msub></math>",
        ),
    ),
    (
        "x^2_3",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><msubsup><mi>x</mi><mn>3</mn><mn>2</mn></msubsup></math>",
        ),
    ),
    (
        "x_a^b",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><msubsup><mi>x</mi><mi>a</mi><mi>b</mi></msubsup></math>",
        ),
    ),
    // Fractions (bar and bar-less) and radicals; an unbraced radicand takes only its first digit.
    (
        "\\frac{a}{b}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mfrac><mi>a</mi><mi>b</mi></mfrac></math>",
        ),
    ),
    (
        "\\tfrac{a}{b}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mfrac linethickness=\"0\"><mi>a</mi><mi>b</mi></mfrac></math>",
        ),
    ),
    (
        "\\dfrac{a}{b}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mfrac><mi>a</mi><mi>b</mi></mfrac></math>",
        ),
    ),
    (
        "\\sqrt{x}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><msqrt><mi>x</mi></msqrt></math>",
        ),
    ),
    (
        "\\sqrt[3]{x}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mroot><mi>x</mi><mn>3</mn></mroot></math>",
        ),
    ),
    (
        "\\sqrt12",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><msqrt><mn>1</mn></msqrt><mn>2</mn></mrow></math>",
        ),
    ),
    // accents over their base, unmapped falls back to macron; `\underline` sets a combining low line
    (
        "\\hat{a}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mover accent=\"true\"><mi>a</mi><mo accent=\"true\">\u{302}</mo></mover></math>",
        ),
    ),
    (
        "\\widehat{a}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mover accent=\"true\"><mi>a</mi><mo accent=\"true\">\u{302}</mo></mover></math>",
        ),
    ),
    (
        "\\tilde{a}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mover accent=\"true\"><mi>a</mi><mo accent=\"true\">\u{303}</mo></mover></math>",
        ),
    ),
    (
        "\\widetilde{a}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mover accent=\"true\"><mi>a</mi><mo accent=\"true\">\u{303}</mo></mover></math>",
        ),
    ),
    (
        "\\vec{a}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mover accent=\"true\"><mi>a</mi><mo accent=\"true\">\u{20d7}</mo></mover></math>",
        ),
    ),
    (
        "\\overrightarrow{a}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mover accent=\"true\"><mi>a</mi><mo accent=\"true\">\u{20d7}</mo></mover></math>",
        ),
    ),
    (
        "\\overleftarrow{a}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mover accent=\"true\"><mi>a</mi><mo accent=\"true\">\u{20d6}</mo></mover></math>",
        ),
    ),
    (
        "\\dot{a}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mover accent=\"true\"><mi>a</mi><mo accent=\"true\">\u{307}</mo></mover></math>",
        ),
    ),
    (
        "\\ddot{a}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mover accent=\"true\"><mi>a</mi><mo accent=\"true\">\u{308}</mo></mover></math>",
        ),
    ),
    (
        "\\dddot{a}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mover accent=\"true\"><mi>a</mi><mo accent=\"true\">\u{20db}</mo></mover></math>",
        ),
    ),
    (
        "\\check{a}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mover accent=\"true\"><mi>a</mi><mo accent=\"true\">\u{30c}</mo></mover></math>",
        ),
    ),
    (
        "\\breve{a}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mover accent=\"true\"><mi>a</mi><mo accent=\"true\">\u{306}</mo></mover></math>",
        ),
    ),
    (
        "\\acute{a}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mover accent=\"true\"><mi>a</mi><mo accent=\"true\">\u{301}</mo></mover></math>",
        ),
    ),
    (
        "\\grave{a}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mover accent=\"true\"><mi>a</mi><mo accent=\"true\">\u{300}</mo></mover></math>",
        ),
    ),
    (
        "\\mathring{a}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mover accent=\"true\"><mi>a</mi><mo accent=\"true\">\u{30a}</mo></mover></math>",
        ),
    ),
    (
        "\\bar{a}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mover accent=\"true\"><mi>a</mi><mo accent=\"true\">‾</mo></mover></math>",
        ),
    ),
    (
        "\\overline{a}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mover accent=\"true\"><mi>a</mi><mo accent=\"true\">‾</mo></mover></math>",
        ),
    ),
    (
        "\\underline{a}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><munder accentunder=\"true\"><mi>a</mi><mo>\u{332}</mo></munder></math>",
        ),
    ),
    // Text wrappers: an upright run tagged `mathvariant=normal`, an italic run left untagged.
    (
        "\\text{if}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mtext mathvariant=\"normal\">if</mtext></math>",
        ),
    ),
    (
        "\\textit{x}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mtext>x</mtext></math>",
        ),
    ),
    (
        "\\emph{x}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mi>emph</mi><mi>x</mi></mrow></math>",
        ),
    ),
    // `\math…` styles map identifier/number leaves into the font-variant block; operator leaves
    // and unstyled symbols are left as they are
    (
        "\\mathbb{R}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mi mathvariant=\"double-struck\">ℝ</mi></math>",
        ),
    ),
    (
        "\\mathds{R}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mi mathvariant=\"double-struck\">ℝ</mi></math>",
        ),
    ),
    (
        "\\mathcal{A}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mi mathvariant=\"script\">𝒜</mi></math>",
        ),
    ),
    (
        "\\mathscr{A}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mi mathvariant=\"script\">𝒜</mi></math>",
        ),
    ),
    (
        "\\mathfrak{g}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mi mathvariant=\"fraktur\">𝔤</mi></math>",
        ),
    ),
    (
        "\\mathbf{R}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mi mathvariant=\"bold-italic\">𝑹</mi></math>",
        ),
    ),
    (
        "\\mathbf{1}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mn mathvariant=\"bold\">𝟏</mn></math>",
        ),
    ),
    (
        "\\mathbf{\\alpha}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mi mathvariant=\"bold\">𝛂</mi></math>",
        ),
    ),
    (
        "\\mathsf{R}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mi mathvariant=\"sans-serif\">𝖱</mi></math>",
        ),
    ),
    (
        "\\mathsf{1}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mn mathvariant=\"sans-serif\">𝟣</mn></math>",
        ),
    ),
    (
        "\\mathtt{R}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mi mathvariant=\"monospace\">𝚁</mi></math>",
        ),
    ),
    (
        "\\mathtt{1}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mn mathvariant=\"monospace\">𝟷</mn></math>",
        ),
    ),
    (
        "\\mathit{x}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mi mathvariant=\"italic\">𝑥</mi></math>",
        ),
    ),
    (
        "\\mathrm{d}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mi mathvariant=\"normal\">d</mi></math>",
        ),
    ),
    (
        "\\mathup{d}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mi mathvariant=\"normal\">d</mi></math>",
        ),
    ),
    (
        "\\mathbf{+}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mo>+</mo></math>",
        ),
    ),
    (
        "\\mathbf{ab}",
        false,
        Some(
            "<math xmlns=\"http://www.w3.org/1998/Math/MathML\" display=\"inline\"><mrow><mi mathvariant=\"bold-italic\">𝒂</mi><mi mathvariant=\"bold-italic\">𝒃</mi></mrow></math>",
        ),
    ),
];
