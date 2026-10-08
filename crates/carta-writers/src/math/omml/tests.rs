//! Golden checks for the OMML backend. Each expected string is a fixed golden: the exact OMML
//! element tree the backend emits for one construct.

use super::to_omml;

/// `(source, expected `<m:oMath>` fragment)` pairs. The zero-width space (`\u{200B}`) fills empty
/// slots; `\u{2009}` is a thin inter-atom space.
const INLINE: &[(&str, &str)] = &[
    ("x", "<m:oMath><m:r><m:t>x</m:t></m:r></m:oMath>"),
    ("1", "<m:oMath><m:r><m:t>1</m:t></m:r></m:oMath>"),
    (
        "a+b",
        "<m:oMath><m:r><m:t>a</m:t></m:r><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>+</m:t></m:r><m:r><m:t>b</m:t></m:r></m:oMath>",
    ),
    ("\\alpha", "<m:oMath><m:r><m:t>α</m:t></m:r></m:oMath>"),
    (
        "\\Gamma",
        "<m:oMath><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>Γ</m:t></m:r></m:oMath>",
    ),
    (
        "x^2",
        "<m:oMath><m:sSup><m:e><m:r><m:t>x</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup></m:oMath>",
    ),
    (
        "x_i",
        "<m:oMath><m:sSub><m:e><m:r><m:t>x</m:t></m:r></m:e><m:sub><m:r><m:t>i</m:t></m:r></m:sub></m:sSub></m:oMath>",
    ),
    (
        "x^2_3",
        "<m:oMath><m:sSubSup><m:e><m:r><m:t>x</m:t></m:r></m:e><m:sub><m:r><m:t>3</m:t></m:r></m:sub><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSubSup></m:oMath>",
    ),
    (
        "f'",
        "<m:oMath><m:sSup><m:e><m:r><m:t>f</m:t></m:r></m:e><m:sup><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>′</m:t></m:r></m:sup></m:sSup></m:oMath>",
    ),
    (
        "x'^2",
        "<m:oMath><m:sSup><m:e><m:r><m:t>x</m:t></m:r></m:e><m:sup><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>′</m:t></m:r></m:sup></m:sSup><m:sSup><m:e><m:r><m:t>\u{200B}</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup></m:oMath>",
    ),
    (
        "{x^2}^3",
        "<m:oMath><m:sSup><m:e><m:sSup><m:e><m:r><m:t>x</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup></m:e><m:sup><m:r><m:t>3</m:t></m:r></m:sup></m:sSup></m:oMath>",
    ),
    (
        "_x",
        "<m:oMath><m:sSub><m:e><m:r><m:t>\u{200B}</m:t></m:r></m:e><m:sub><m:r><m:t>x</m:t></m:r></m:sub></m:sSub></m:oMath>",
    ),
    (
        "\\frac{a}{b}",
        "<m:oMath><m:f><m:fPr><m:type m:val=\"bar\" /></m:fPr><m:num><m:r><m:t>a</m:t></m:r></m:num><m:den><m:r><m:t>b</m:t></m:r></m:den></m:f></m:oMath>",
    ),
    (
        "\\sqrt{x+1}",
        "<m:oMath><m:rad><m:radPr><m:degHide m:val=\"on\" /></m:radPr><m:deg /><m:e><m:r><m:t>x</m:t></m:r><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>+</m:t></m:r><m:r><m:t>1</m:t></m:r></m:e></m:rad></m:oMath>",
    ),
    (
        "\\sqrt[3]{c}",
        "<m:oMath><m:rad><m:deg><m:r><m:t>3</m:t></m:r></m:deg><m:e><m:r><m:t>c</m:t></m:r></m:e></m:rad></m:oMath>",
    ),
    (
        "\\sum_a^b x",
        "<m:oMath><m:nary><m:naryPr><m:chr m:val=\"∑\" /><m:limLoc m:val=\"undOvr\" /><m:subHide m:val=\"off\" /><m:supHide m:val=\"off\" /></m:naryPr><m:sub><m:r><m:t>a</m:t></m:r></m:sub><m:sup><m:r><m:t>b</m:t></m:r></m:sup><m:e><m:r><m:t>x</m:t></m:r></m:e></m:nary></m:oMath>",
    ),
    (
        "\\int_a^b f",
        "<m:oMath><m:nary><m:naryPr><m:chr m:val=\"∫\" /><m:limLoc m:val=\"subSup\" /><m:subHide m:val=\"off\" /><m:supHide m:val=\"off\" /></m:naryPr><m:sub><m:r><m:t>a</m:t></m:r></m:sub><m:sup><m:r><m:t>b</m:t></m:r></m:sup><m:e><m:r><m:t>f</m:t></m:r></m:e></m:nary></m:oMath>",
    ),
    (
        "\\bigcup_a^b",
        "<m:oMath><m:sSubSup><m:e><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>⋃</m:t></m:r></m:e><m:sub><m:r><m:t>a</m:t></m:r></m:sub><m:sup><m:r><m:t>b</m:t></m:r></m:sup></m:sSubSup></m:oMath>",
    ),
    (
        "\\lim_x",
        "<m:oMath><m:sSub><m:e><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>lim</m:t></m:r></m:e><m:sub><m:r><m:t>x</m:t></m:r></m:sub></m:sSub></m:oMath>",
    ),
    (
        "\\bar{x}",
        "<m:oMath><m:acc><m:accPr><m:chr m:val=\"‾\" /></m:accPr><m:e><m:r><m:t>x</m:t></m:r></m:e></m:acc></m:oMath>",
    ),
    (
        "\\vec{v}",
        "<m:oMath><m:acc><m:accPr><m:chr m:val=\"\u{20D7}\" /></m:accPr><m:e><m:r><m:t>v</m:t></m:r></m:e></m:acc></m:oMath>",
    ),
    (
        "\\overline{x}",
        "<m:oMath><m:bar><m:barPr><m:pos m:val=\"top\" /></m:barPr><m:e><m:r><m:t>x</m:t></m:r></m:e></m:bar></m:oMath>",
    ),
    (
        "\\left(a\\right)",
        "<m:oMath><m:d><m:dPr><m:begChr m:val=\"(\" /><m:sepChr m:val=\"\" /><m:endChr m:val=\")\" /><m:grow /></m:dPr><m:e><m:r><m:t>a</m:t></m:r></m:e></m:d></m:oMath>",
    ),
    (
        "\\begin{pmatrix}a&b\\\\c&d\\end{pmatrix}",
        "<m:oMath><m:d><m:dPr><m:begChr m:val=\"(\" /><m:sepChr m:val=\"\" /><m:endChr m:val=\")\" /><m:grow /></m:dPr><m:e><m:m><m:mPr><m:baseJc m:val=\"center\" /><m:plcHide m:val=\"on\" /><m:mcs><m:mc><m:mcPr><m:mcJc m:val=\"center\" /><m:count m:val=\"1\" /></m:mcPr></m:mc><m:mc><m:mcPr><m:mcJc m:val=\"center\" /><m:count m:val=\"1\" /></m:mcPr></m:mc></m:mcs></m:mPr><m:mr><m:e><m:r><m:t>a</m:t></m:r></m:e><m:e><m:r><m:t>b</m:t></m:r></m:e></m:mr><m:mr><m:e><m:r><m:t>c</m:t></m:r></m:e><m:e><m:r><m:t>d</m:t></m:r></m:e></m:mr></m:m></m:e></m:d></m:oMath>",
    ),
    (
        "\\begin{cases}a&x>0\\\\b&x<0\\end{cases}",
        "<m:oMath><m:d><m:dPr><m:begChr m:val=\"{\" /><m:sepChr m:val=\"\" /><m:endChr m:val=\"\" /><m:grow /></m:dPr><m:e><m:m><m:mPr><m:baseJc m:val=\"center\" /><m:plcHide m:val=\"on\" /><m:mcs><m:mc><m:mcPr><m:mcJc m:val=\"left\" /><m:count m:val=\"1\" /></m:mcPr></m:mc><m:mc><m:mcPr><m:mcJc m:val=\"left\" /><m:count m:val=\"1\" /></m:mcPr></m:mc></m:mcs></m:mPr><m:mr><m:e><m:r><m:t>a</m:t></m:r></m:e><m:e><m:r><m:t>x</m:t></m:r><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>&gt;</m:t></m:r><m:r><m:t>0</m:t></m:r></m:e></m:mr><m:mr><m:e><m:r><m:t>b</m:t></m:r></m:e><m:e><m:r><m:t>x</m:t></m:r><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>&lt;</m:t></m:r><m:r><m:t>0</m:t></m:r></m:e></m:mr></m:m></m:e></m:d></m:oMath>",
    ),
    (
        "\\binom{n}{k}",
        "<m:oMath><m:d><m:dPr><m:begChr m:val=\"(\" /><m:sepChr m:val=\"\" /><m:endChr m:val=\")\" /><m:grow /></m:dPr><m:e><m:f><m:fPr><m:type m:val=\"noBar\" /></m:fPr><m:num><m:r><m:t>n</m:t></m:r></m:num><m:den><m:r><m:t>k</m:t></m:r></m:den></m:f></m:e></m:d></m:oMath>",
    ),
    (
        "\\mathbb{R}",
        "<m:oMath><m:r><m:rPr><m:scr m:val=\"double-struck\" /><m:sty m:val=\"p\" /></m:rPr><m:t>R</m:t></m:r></m:oMath>",
    ),
    (
        "\\mathbf{R}",
        "<m:oMath><m:r><m:rPr><m:sty m:val=\"bi\" /></m:rPr><m:t>R</m:t></m:r></m:oMath>",
    ),
    (
        "\\mathbf{\\alpha}",
        "<m:oMath><m:r><m:rPr><m:sty m:val=\"bi\" /></m:rPr><m:t>α</m:t></m:r></m:oMath>",
    ),
    (
        "\\mathit{1}",
        "<m:oMath><m:r><m:rPr><m:sty m:val=\"i\" /></m:rPr><m:t>1</m:t></m:r></m:oMath>",
    ),
    (
        "\\mathrm{d}",
        "<m:oMath><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>d</m:t></m:r></m:oMath>",
    ),
    (
        "\\mathcal{A}",
        "<m:oMath><m:r><m:rPr><m:scr m:val=\"script\" /><m:sty m:val=\"p\" /></m:rPr><m:t>A</m:t></m:r></m:oMath>",
    ),
    (
        "\\text{if}",
        "<m:oMath><m:r><m:rPr><m:nor /><m:sty m:val=\"p\" /></m:rPr><m:t>if</m:t></m:r></m:oMath>",
    ),
    (
        "\\operatorname{sn}",
        "<m:oMath><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>sn</m:t></m:r></m:oMath>",
    ),
    (
        "a\\,b",
        "<m:oMath><m:r><m:t>a</m:t></m:r><m:r><m:t>\u{2009}</m:t></m:r><m:r><m:t>b</m:t></m:r></m:oMath>",
    ),
    (
        "a:=b",
        "<m:oMath><m:r><m:t>a</m:t></m:r><m:box><m:boxPr><m:opEmu m:val=\"on\" /></m:boxPr><m:e><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>:=</m:t></m:r></m:e></m:box><m:r><m:t>b</m:t></m:r></m:oMath>",
    ),
    (
        "\\tfrac{a}{b}",
        "<m:oMath><m:f><m:fPr><m:type m:val=\"lin\" /></m:fPr><m:num><m:r><m:t>a</m:t></m:r></m:num><m:den><m:r><m:t>b</m:t></m:r></m:den></m:f></m:oMath>",
    ),
    (
        "\\overbrace{a+b}",
        "<m:oMath><m:groupChr><m:groupChrPr><m:chr m:val=\"⏞\" /><m:pos m:val=\"top\" /><m:vertJc m:val=\"bot\" /></m:groupChrPr><m:e><m:r><m:t>a</m:t></m:r><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>+</m:t></m:r><m:r><m:t>b</m:t></m:r></m:e></m:groupChr></m:oMath>",
    ),
    (
        "\\underbrace{a+b}^{n}",
        "<m:oMath><m:sSup><m:e><m:limLow><m:e><m:r><m:t>a</m:t></m:r><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>+</m:t></m:r><m:r><m:t>b</m:t></m:r></m:e><m:lim><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>⏟</m:t></m:r></m:lim></m:limLow></m:e><m:sup><m:r><m:t>n</m:t></m:r></m:sup></m:sSup></m:oMath>",
    ),
    (
        "\\overset{a}{b}",
        "<m:oMath><m:limUpp><m:e><m:r><m:t>b</m:t></m:r></m:e><m:lim><m:r><m:t>a</m:t></m:r></m:lim></m:limUpp></m:oMath>",
    ),
    (
        "a \\pmod{n}",
        "<m:oMath><m:r><m:t>a</m:t></m:r><m:r><m:t>\u{2005}</m:t></m:r><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>(</m:t></m:r><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>mod</m:t></m:r><m:r><m:t>\u{2005}</m:t></m:r><m:r><m:t>n</m:t></m:r><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>)</m:t></m:r></m:oMath>",
    ),
    (
        "\\not= b",
        "<m:oMath><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>≠</m:t></m:r><m:r><m:t>b</m:t></m:r></m:oMath>",
    ),
    (
        "\\not\\vdash",
        "<m:oMath><m:box><m:boxPr><m:opEmu m:val=\"on\" /></m:boxPr><m:e><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>\u{22A2}\u{338}</m:t></m:r></m:e></m:box></m:oMath>",
    ),
    (
        "\\xrightarrow{f}",
        "<m:oMath><m:limUpp><m:e><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>→</m:t></m:r></m:e><m:lim><m:r><m:t>f</m:t></m:r></m:lim></m:limUpp></m:oMath>",
    ),
    (
        "\\cancel{x}",
        "<m:oMath><m:borderBox><m:borderBoxPr><m:hideTop m:val=\"1\" /><m:hideBot m:val=\"1\" /><m:hideLeft m:val=\"1\" /><m:hideRight m:val=\"1\" /><m:strikeBLTR m:val=\"1\" /></m:borderBoxPr><m:e><m:r><m:t>x</m:t></m:r></m:e></m:borderBox></m:oMath>",
    ),
    (
        "\\boxed{x}",
        "<m:oMath><m:borderBox><m:e><m:r><m:t>x</m:t></m:r></m:e></m:borderBox></m:oMath>",
    ),
    (
        "\\left\\{x \\middle| \\mathrm{pred}\\right\\}",
        "<m:oMath><m:d><m:dPr><m:begChr m:val=\"{\" /><m:sepChr m:val=\"|\" /><m:endChr m:val=\"}\" /><m:grow /></m:dPr><m:e><m:r><m:t>x</m:t></m:r></m:e><m:e><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>p</m:t></m:r><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>r</m:t></m:r><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>e</m:t></m:r><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>d</m:t></m:r></m:e></m:d></m:oMath>",
    ),
    (
        "\\begin{eqnarray}a &= b &c\\end{eqnarray}",
        "<m:oMath><m:m><m:mPr><m:baseJc m:val=\"center\" /><m:plcHide m:val=\"on\" /><m:mcs><m:mc><m:mcPr><m:mcJc m:val=\"right\" /><m:count m:val=\"1\" /></m:mcPr></m:mc><m:mc><m:mcPr><m:mcJc m:val=\"center\" /><m:count m:val=\"1\" /></m:mcPr></m:mc><m:mc><m:mcPr><m:mcJc m:val=\"left\" /><m:count m:val=\"1\" /></m:mcPr></m:mc></m:mcs></m:mPr><m:mr><m:e><m:r><m:t>a</m:t></m:r></m:e><m:e><m:r><m:rPr><m:sty m:val=\"p\" /></m:rPr><m:t>=</m:t></m:r><m:r><m:t>b</m:t></m:r></m:e><m:e><m:r><m:t>c</m:t></m:r></m:e></m:mr></m:m></m:oMath>",
    ),
    (
        "\\sqrt12",
        "<m:oMath><m:rad><m:radPr><m:degHide m:val=\"on\" /></m:radPr><m:deg /><m:e><m:r><m:t>1</m:t></m:r></m:e></m:rad><m:r><m:t>2</m:t></m:r></m:oMath>",
    ),
    (
        "\\begin{array}{cc}a & b \\\\ c & d\\end{array}",
        "<m:oMath><m:m><m:mPr><m:baseJc m:val=\"center\" /><m:plcHide m:val=\"on\" /><m:mcs><m:mc><m:mcPr><m:mcJc m:val=\"center\" /><m:count m:val=\"1\" /></m:mcPr></m:mc><m:mc><m:mcPr><m:mcJc m:val=\"center\" /><m:count m:val=\"1\" /></m:mcPr></m:mc></m:mcs></m:mPr><m:mr><m:e><m:r><m:t>a</m:t></m:r></m:e><m:e><m:r><m:t>b</m:t></m:r></m:e></m:mr><m:mr><m:e><m:r><m:t>c</m:t></m:r></m:e><m:e><m:r><m:t>d</m:t></m:r></m:e></m:mr></m:m></m:oMath>",
    ),
];

#[test]
fn inline_goldens() {
    for (source, expected) in INLINE {
        assert_eq!(
            to_omml(source, false).as_deref(),
            Some(*expected),
            "source: {source:?}"
        );
    }
}

#[test]
fn display_wraps_in_para() {
    assert_eq!(
        to_omml("x^2", true).as_deref(),
        Some(
            "<m:oMathPara><m:oMathParaPr><m:jc m:val=\"center\" /></m:oMathParaPr><m:oMath><m:sSup><m:e><m:r><m:t>x</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup></m:oMath></m:oMathPara>"
        )
    );
}

#[test]
fn unconvertible_constructs_degrade_to_none() {
    assert_eq!(to_omml("\\thiscommanddoesnotexist", false), None);
    assert_eq!(to_omml("\\phantom{x}", false), None);
    assert_eq!(to_omml("\\not\\mid", false), None);
}

#[test]
fn xml_special_characters_are_escaped() {
    let rendered = to_omml("a<b", false).unwrap_or_default();
    assert!(rendered.contains("&lt;"));
    assert!(!rendered.contains("<m:t>a<b"));
}

#[test]
fn deeply_nested_input_does_not_panic() {
    // A pathological brace nest is bounded: it returns some result without overflowing the stack.
    let source = format!("{}x{}", "{".repeat(200), "}".repeat(200));
    let _ = to_omml(&source, false);
}

#[test]
fn empty_input_renders_empty_math() {
    assert_eq!(to_omml("", false).as_deref(), Some("<m:oMath />"));
}
