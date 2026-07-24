use super::*;
use carta_ast::Document;

fn render(blocks: Vec<Block>) -> String {
    let document = Document {
        blocks,
        ..Document::default()
    };
    JiraWriter
        .write(&document, &WriterOptions::default())
        .expect("jira writer is infallible over these inputs")
}

fn para(inlines: Vec<Inline>) -> Block {
    Block::Para(inlines)
}

fn math(kind: MathType, tex: &str) -> Inline {
    Inline::Math(kind, tex.to_owned().into())
}

fn inline(tex: &str) -> String {
    render(vec![para(vec![math(MathType::InlineMath, tex)])])
}

fn display(tex: &str) -> String {
    render(vec![para(vec![math(MathType::DisplayMath, tex)])])
}

fn str_inline(text: &str) -> Inline {
    Inline::Str(text.to_owned().into())
}

#[test]
fn superscript_lowers_to_caret_markup() {
    assert_eq!(inline("a^2"), "_a_^2^");
}

#[test]
fn subscript_lowers_to_tilde_markup() {
    assert_eq!(inline("a_n"), "_a_~_n_~");
}

#[test]
fn binary_operator_and_relation_carry_typographic_spacing() {
    // Math spacing around `+` and `=` is word-like, so following variables get braced markers.
    assert_eq!(
        inline("a^2 + b^2 = c^2"),
        "_a_^2^\u{2005}+\u{2005}{_}b{_}^2^\u{2004}=\u{2004}{_}c{_}^2^"
    );
}

#[test]
fn greek_letters_render_as_unicode() {
    assert_eq!(inline("\\alpha+\\beta"), "_α_\u{2005}+\u{2005}{_}β{_}");
}

#[test]
fn blackboard_alphabet_renders_as_unicode_symbol() {
    assert_eq!(inline("\\mathbb{R}"), "ℝ");
}

#[test]
fn bold_alphabet_lowers_to_strong_markup() {
    assert_eq!(inline("\\mathbf{v}"), "*v*");
}

#[test]
fn accent_renders_as_combining_mark_inside_emphasis() {
    assert_eq!(inline("\\bar{x}"), "_x\u{304}_");
}

#[test]
fn integral_lowers_to_symbol_with_scripts_and_thin_space() {
    // The thin space (`\,`) is word-like, so the differential's variable is brace-guarded.
    assert_eq!(
        inline("\\int_0^1 x \\, dx"),
        "∫{~}0{~}^1^_x_\u{2006}{_}d{_}_x_"
    );
}

#[test]
fn inline_math_threads_surrounding_text_into_edge_guards() {
    // Flanking word text forces braced emphasis markers.
    let out = render(vec![para(vec![
        str_inline("x"),
        math(MathType::InlineMath, "a^2"),
        str_inline("y"),
    ])]);
    assert_eq!(out, "x{_}a{_}{^}2{^}y");
}

#[test]
fn display_math_stands_on_its_own_line() {
    // Framed by a newline on each side; the document writer adds the trailing one.
    assert_eq!(display("a^2"), "\n_a_^2^\n");
}

#[test]
fn display_math_breaks_surrounding_inline_content() {
    let out = render(vec![para(vec![
        str_inline("before"),
        Inline::Space,
        math(MathType::DisplayMath, "a^2"),
        Inline::Space,
        str_inline("after"),
    ])]);
    assert_eq!(out, "before \n_a_^2^\n after");
}

#[test]
fn inline_fallback_wraps_source_in_single_dollars() {
    // No single-line form: verbatim in `$…$`, braces escaped, content-flanked backslash literal.
    assert_eq!(inline("\\frac{1}{2}"), "$\\frac\\{1\\}\\{2\\}$");
}

#[test]
fn display_fallback_wraps_source_in_double_dollars_on_its_own_line() {
    assert_eq!(display("\\sqrt{x}"), "\n$$\\sqrt\\{x\\}$$\n");
}

#[test]
fn empty_inline_math_contributes_nothing() {
    let out = render(vec![para(vec![
        str_inline("x"),
        math(MathType::InlineMath, "  "),
        str_inline("y"),
    ])]);
    assert_eq!(out, "xy");
}

#[test]
fn empty_display_math_emits_an_empty_framed_line() {
    let out = render(vec![para(vec![
        str_inline("x"),
        math(MathType::DisplayMath, ""),
        str_inline("y"),
    ])]);
    assert_eq!(out, "x\n\ny");
}

#[test]
fn preceding_emphasis_guards_against_convertible_math_starting_with_a_letter() {
    // Math opening with an alphanumeric (`ℝ`) forces braces on the preceding closing marker.
    let out = render(vec![para(vec![
        Inline::Emph(vec![str_inline("word")]),
        math(MathType::InlineMath, "\\mathbb{R}"),
    ])]);
    assert_eq!(out, "{_}word{_}ℝ");
}

#[test]
fn only_plain_space_and_newline_are_word_boundaries() {
    assert!(is_word_boundary(' '));
    assert!(is_word_boundary('\n'));
    // Typographic and non-breaking spaces are word-like, so a marker resting on one is braced.
    assert!(!is_word_boundary('\u{00a0}'));
    assert!(!is_word_boundary('\u{2004}'));
    assert!(!is_word_boundary('\u{2005}'));
    assert!(!is_word_boundary('\u{2006}'));
    assert!(!is_word_boundary('\t'));
}

#[test]
fn deep_nesting_falls_back_without_panicking() {
    // Past the depth limit conversion must degrade to verbatim, not overflow the stack.
    let tex = format!("{}x{}", "{".repeat(5000), "}".repeat(5000));
    let out = inline(&tex);
    assert!(out.starts_with('$') && out.ends_with('$'));
}

/// Render a single plain-text token as the writer would, with no surrounding inlines.
fn text(value: &str) -> String {
    render(vec![para(vec![str_inline(value)])])
}

#[test]
fn lone_backslash_between_words_stays_literal() {
    assert_eq!(text("a\\b"), "a\\b");
    assert_eq!(text("path\\to\\file"), "path\\to\\file");
}

#[test]
fn backslash_at_an_edge_becomes_the_entity() {
    // At the string edge a backslash would read as an escape, so the `&bsol;` entity.
    assert_eq!(text("\\start"), "&bsol;start");
    assert_eq!(text("end\\"), "end&bsol;");
}

#[test]
fn backslash_between_spaces_stays_literal() {
    // Both neighbors are plain spaces, the same category, so the backslash is kept literal.
    assert_eq!(text("a \\ b"), "a \\ b");
}

#[test]
fn consecutive_backslashes_become_entities() {
    // Each backslash neighbors another backslash, a differing category, so both are entities.
    assert_eq!(text("x\\\\y"), "x&bsol;&bsol;y");
}

#[test]
fn backslash_before_a_marker_becomes_the_entity() {
    // Word-vs-marker is a category mismatch, so entity; the entity's `&` then makes the
    // following marker markup-significant.
    assert_eq!(text("a\\*b"), "a&bsol;\\*b");
    assert_eq!(text("\\*end"), "&bsol;\\*end");
}

#[test]
fn open_paren_inside_a_word_stays_bare() {
    assert_eq!(text("a(b)c"), "a(b)c");
    assert_eq!(text("a (b) c"), "a \\(b) c");
}

#[test]
fn open_paren_before_an_emoticon_body_is_escaped() {
    // An icon body escapes the opening `(`; the matching `)` stays bare.
    assert_eq!(text("(x)"), "\\(x)");
    assert_eq!(text("(y)"), "\\(y)");
    assert_eq!(text("f(x)"), "f\\(x)");
    assert_eq!(text("(!)"), "\\(!)");
}

#[test]
fn space_flanked_open_paren_escapes_only_at_the_trailing_edge() {
    // Space-flanked `(` stays bare while content follows; on the trailing edge it escapes.
    assert_eq!(text("a ( b"), "a ( b");
    assert_eq!(text("a ( ( b"), "a ( ( b");
    assert_eq!(text("a ( )"), "a ( )");
    assert_eq!(text("a ( "), "a \\(");
}

#[test]
fn space_flanked_open_paren_consults_the_inline_stream_for_the_trailing_edge() {
    // The trailing-edge test must consult the inline stream, not just this token's own tail.
    let prose = render(vec![para(vec![
        str_inline("a"),
        Inline::Space,
        str_inline("("),
        Inline::Space,
        str_inline("b"),
    ])]);
    assert_eq!(prose, "a ( b");
    let trailing_space = render(vec![para(vec![
        str_inline("a"),
        Inline::Space,
        str_inline("("),
        Inline::Space,
    ])]);
    assert_eq!(trailing_space, "a ( ");
    let document_edge = render(vec![para(vec![
        str_inline("a"),
        Inline::Space,
        str_inline("("),
    ])]);
    assert_eq!(document_edge, "a \\(");
    // A markup inline directly after puts the `(` against a boundary: escaped.
    let before_markup = render(vec![para(vec![
        str_inline("a"),
        Inline::Space,
        str_inline("("),
        Inline::Emph(vec![str_inline("b")]),
    ])]);
    assert_eq!(before_markup, "a \\(_b_");
}

#[test]
fn open_paren_at_an_edge_is_escaped() {
    assert_eq!(text("(start"), "\\(start");
    assert_eq!(text("("), "\\(");
}

#[test]
fn open_paren_does_not_over_escape_a_following_marker() {
    // The `!` after the escaped `(` is content here and must not itself be escaped.
    assert_eq!(text("z(!)"), "z\\(!)");
}

#[test]
fn text_emoticons_escape_their_leading_punctuation() {
    // The leading `:`/`;` is escaped so the literal characters render, not an icon.
    assert_eq!(text(":)"), "\\:)");
    assert_eq!(text(":("), "\\:(");
    assert_eq!(text(":P"), "\\:P");
    assert_eq!(text(":D"), "\\:D");
    assert_eq!(text(";)"), "\\;)");
    assert_eq!(text(";P"), "\\;P");
    assert_eq!(text(";D"), "\\;D");
}

#[test]
fn colon_emoticon_escapes_when_followed_by_a_word() {
    // The `:`-family reads as an emoticon whenever a markup boundary follows, even mid-word.
    assert_eq!(text("a:)"), "a\\:)");
    // A following word character keeps the sequence from being an emoticon.
    assert_eq!(text(":)x"), ":)x");
}

#[test]
fn wink_letter_emoticon_needs_a_boundary_on_both_sides() {
    // `;P`/`;D` need a boundary on both sides; a word char or `)` suppresses the escape.
    assert_eq!(text("a;P"), "a;P");
    assert_eq!(text(";P)"), ";P)");
    assert_eq!(text(" ;P "), " \\;P");
}

#[test]
fn colon_open_paren_emoticon_escapes_the_colon_only() {
    // `:(` at a boundary is a sad-face emoticon: the `:` is escaped and the parenthesis is bare.
    assert_eq!(text(":("), "\\:(");
    // Followed by a word it is no emoticon: `:` stays bare, `(` escapes against the significant `:`.
    assert_eq!(text(":(x"), ":\\(x");
}

#[test]
fn slash_adjacent_open_paren_stays_bare() {
    // A slash is ordinary content for the parenthesis rule.
    assert_eq!(text("a(/"), "a(/");
    assert_eq!(text("/(x"), "/(x");
}

#[test]
fn run_of_markers_escapes_every_member() {
    // Adjoining markers each open or close markup, so every member escapes, not just the edges.
    assert_eq!(text("a -- b"), "a \\-\\- b");
    assert_eq!(text("a---b"), "a\\-\\-\\-b");
    assert_eq!(text("__x"), "\\_\\_x");
    assert_eq!(text("++plus++"), "\\+\\+plus\\+\\+");
    assert_eq!(text("a !! b"), "a \\!\\! b");
}

#[test]
fn interior_marker_of_a_long_run_is_escaped() {
    // A marker flanked by markers escapes even without a content transition across it.
    assert_eq!(text("*_*"), "\\*\\_\\*");
    assert_eq!(text("-+-"), "\\-\\+\\-");
    assert_eq!(text("a*-+b"), "a\\*\\-\\+b");
}

#[test]
fn marker_against_punctuation_is_escaped() {
    // `:` is markup-significant punctuation; `)` is ordinary content and leaves the marker bare.
    assert_eq!(text("a:*:b"), "a:\\*:b");
    assert_eq!(text("a)*)b"), "a)*)b");
}

#[test]
fn marker_against_an_entity_backslash_is_escaped() {
    // The `&bsol;` entity's `&` is a markup boundary, so the marker escapes on that side.
    assert_eq!(text("a*\\"), "a\\*&bsol;");
    assert_eq!(text("x*\\y"), "x\\*&bsol;y");
    assert_eq!(text("\\*a"), "&bsol;\\*a");
}

#[test]
fn marker_against_a_neutralized_emoticon_stays_bare() {
    // An escaped emoticon opener reads as content, so the abutting marker stays bare.
    assert_eq!(text("a_:)"), "a_\\:)");
    assert_eq!(text(":(*<>:"), "\\:(*<>:");
    assert_eq!(text("a_(x)"), "a_\\(x)");
    // When the colon does not open an emoticon it is markup-significant and the marker is escaped.
    assert_eq!(text("a_:x"), "a\\_:x");
}

#[test]
fn lone_marker_keeps_the_boundary_rule() {
    assert_eq!(text("a*b"), "a*b");
    assert_eq!(text("a - b"), "a - b");
    assert_eq!(text("a_b"), "a_b");
}

/// Render a single inline-code token as the writer would, with no surrounding inlines.
fn code(value: &str) -> String {
    render(vec![para(vec![Inline::Code(
        Box::default(),
        value.to_owned().into(),
    )])])
}

#[test]
fn inline_code_escapes_its_boundary_markers() {
    // Boundary markup characters inside a monospaced span are still escaped.
    assert_eq!(code("(x)"), "{{\\(x)}}");
    assert_eq!(code("[y]"), "{{\\[y\\]}}");
    assert_eq!(code("*x*"), "{{\\*x\\*}}");
    assert_eq!(code("|x|"), "{{\\|x\\|}}");
}

#[test]
fn inline_code_leaves_interior_content_markers_bare() {
    // Content-flanked markers stay bare; an abutting run escapes throughout.
    assert_eq!(code("a*b"), "{{a*b}}");
    assert_eq!(code("a|b"), "{{a|b}}");
    assert_eq!(code("a--b"), "{{a\\-\\-b}}");
}

#[test]
fn inline_code_handles_backslashes_like_running_text() {
    // Same as running text: literal between content, the `&bsol;` entity at the edge.
    assert_eq!(code("a\\b"), "{{a\\b}}");
    assert_eq!(code("a\\"), "{{a&bsol;}}");
}
