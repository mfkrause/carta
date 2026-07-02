//! HTML reader.
//!
//! Parsing runs in three stages: a tokenizer (`tokenize`) turns the source into a flat stream of
//! start tags, end tags and text; a tree builder (`tree::build_tree`) assembles that stream into a
//! node tree, applying void-element and implied-end-tag rules; and a `convert::Converter` walks the
//! tree into a [`Document`]. Document metadata is read from a `<head>` element when present.

mod classify;
mod convert;
mod notes;
mod table;
mod tokenize;
mod tree;

use std::borrow::Cow;

use carta_ast::Document;
use carta_core::{Extensions, Reader, ReaderOptions, Result};

#[cfg(feature = "opml")]
use carta_ast::Inline;

#[cfg(feature = "opml")]
use convert::inlines_from_nodes;
use convert::{Converter, extract_meta};
use tokenize::tokenize;
use tree::{build_tree, locate};

/// Parses HTML text into the document model.
#[derive(Debug, Default, Clone, Copy)]
pub struct HtmlReader;

impl Reader for HtmlReader {
    fn read(&self, input: &str, options: &ReaderOptions) -> Result<Document> {
        Ok(parse(input, options.extensions))
    }
}

fn parse(input: &str, ext: Extensions) -> Document {
    let normalized = normalize(input);
    let chars: Vec<char> = normalized.chars().collect();
    let tokens = tokenize(&chars);
    let roots = build_tree(tokens);
    let (head, body) = locate(&roots);

    let mut converter = Converter::new(ext);
    converter.index_notes(notes::collect_note_defs(&body));
    let meta = head.map(extract_meta).unwrap_or_default();
    let blocks = converter.blocks(&body, false);
    Document {
        meta,
        blocks,
        ..Document::default()
    }
}

/// Parse a string of HTML inline markup into inlines, with no surrounding block. Recognized inline
/// tags (`<em>`, `<strong>`, `<code>`, `<a>`, …) become their corresponding constructs, character
/// references are resolved, and leading and trailing whitespace is trimmed. Intended for callers
/// that carry inline content in a single string, such as an outline heading.
#[cfg(feature = "opml")]
pub(crate) fn parse_inline_fragment(input: &str) -> Vec<Inline> {
    let normalized = normalize(input);
    let chars: Vec<char> = normalized.chars().collect();
    let tokens = tokenize(&chars);
    let roots = build_tree(tokens);
    inlines_from_nodes(&roots)
}

/// Normalize line endings to `\n` and strip a leading byte-order mark.
fn normalize(input: &str) -> Cow<'_, str> {
    let without_bom = input.strip_prefix('\u{feff}').unwrap_or(input);
    if !without_bom.contains('\r') {
        return Cow::Borrowed(without_bom);
    }
    let mut out = String::with_capacity(without_bom.len());
    let mut chars = without_bom.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            other => out.push(other),
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::HtmlReader;
    use carta_ast::{Block, Inline, MathType};
    use carta_core::{Extension, Extensions, Reader, ReaderOptions};

    /// The structural extensions enabled by default for the `html` format. The unit tests exercise
    /// this default dialect; `+`/`-` toggle behavior is covered by the golden corpus.
    fn html_defaults() -> Extensions {
        Extensions::from_list(&[
            Extension::AutoIdentifiers,
            Extension::LineBlocks,
            Extension::NativeDivs,
            Extension::NativeSpans,
        ])
    }

    fn read_with(input: &str, extensions: Extensions) -> Vec<Block> {
        let mut options = ReaderOptions::default();
        options.extensions = extensions;
        HtmlReader
            .read(input, &options)
            .expect("reader should not fail")
            .blocks
    }

    fn blocks(input: &str) -> Vec<Block> {
        read_with(input, html_defaults())
    }

    #[test]
    fn paragraph_with_emphasis() {
        let result = blocks("<p>a <em>b</em></p>");
        assert!(matches!(result.as_slice(), [Block::Para(_)]));
    }

    #[test]
    fn loose_text_is_plain() {
        assert!(matches!(blocks("hello").as_slice(), [Block::Plain(_)]));
    }

    #[test]
    fn paragraph_sibling_promotes_loose_text() {
        let result = blocks("loose<p>para</p>");
        assert!(matches!(
            result.as_slice(),
            [Block::Para(_), Block::Para(_)]
        ));
    }

    #[test]
    fn horizontal_rule_does_not_promote() {
        let result = blocks("loose<hr>");
        assert!(matches!(
            result.as_slice(),
            [Block::Plain(_), Block::HorizontalRule]
        ));
    }

    #[test]
    fn nested_list_inside_item_stays_tight() {
        let result = blocks("<ul><li>a<ul><li>b</li></ul></li></ul>");
        let Some(Block::BulletList(items)) = result.first() else {
            panic!("expected bullet list");
        };
        let Some(item) = items.first() else {
            panic!("expected one item");
        };
        assert!(matches!(item.first(), Some(Block::Plain(_))));
    }

    #[test]
    fn heading_generates_identifier() {
        let result = blocks("<h1>Hello World</h1>");
        let Some(Block::Header(level, attr, _)) = result.first() else {
            panic!("expected header");
        };
        assert_eq!(*level, 1);
        assert_eq!(attr.id, "hello-world");
    }

    #[test]
    fn duplicate_identifiers_are_disambiguated() {
        let result = blocks("<h1>Sec</h1><h2>Sec</h2>");
        let ids: Vec<&str> = result
            .iter()
            .filter_map(|block| match block {
                Block::Header(_, attr, _) => Some(attr.id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec!["sec", "sec-1"]);
    }

    #[test]
    fn entities_are_decoded() {
        let result = blocks("<p>a &amp; b &copy; c</p>");
        let Some(Block::Para(inlines)) = result.first() else {
            panic!("expected paragraph");
        };
        assert!(inlines.contains(&Inline::Str("&".to_string())));
        assert!(inlines.contains(&Inline::Str("\u{a9}".to_string())));
    }

    #[test]
    fn comment_joins_surrounding_text() {
        let result = blocks("<p>a<!-- c -->b</p>");
        let Some(Block::Para(inlines)) = result.first() else {
            panic!("expected paragraph");
        };
        assert_eq!(inlines.as_slice(), [Inline::Str("ab".to_string())]);
    }

    #[test]
    fn script_content_is_dropped() {
        assert!(blocks("<script>var x = 1;</script><p>p</p>").len() == 1);
    }

    #[test]
    fn head_metadata_is_extracted() {
        let document = HtmlReader
            .read(
                "<head><title>T</title><meta name=\"author\" content=\"A\"></head><body><p>b</p></body>",
                &ReaderOptions::default(),
            )
            .expect("reader should not fail");
        assert!(document.meta.contains_key("title"));
        assert!(document.meta.contains_key("author"));
    }

    use carta_ast::{Alignment, ColWidth, ListNumberStyle, Target};

    fn first_block(input: &str) -> Block {
        blocks(input).into_iter().next().expect("a block")
    }

    fn para_inlines(input: &str) -> Vec<Inline> {
        match first_block(input) {
            Block::Para(inlines) | Block::Plain(inlines) => inlines,
            other => panic!("expected a paragraph, got {other:?}"),
        }
    }

    #[test]
    fn normalizes_crlf_and_strips_bom() {
        let inlines = para_inlines("\u{feff}<p>a\r\nb</p>");
        assert_eq!(
            inlines.as_slice(),
            [
                Inline::Str("a".to_string()),
                Inline::SoftBreak,
                Inline::Str("b".to_string())
            ]
        );
    }

    #[test]
    fn ordered_list_reads_type_and_start() {
        let Block::OrderedList(attrs, items) =
            first_block(r#"<ol type="A" start="3"><li>x</li><li>y</li></ol>"#)
        else {
            panic!("expected ordered list");
        };
        assert_eq!(attrs.start, 3);
        assert_eq!(attrs.style, ListNumberStyle::UpperAlpha);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn menu_is_a_bullet_list() {
        assert!(matches!(
            first_block("<menu><li>a</li></menu>"),
            Block::BulletList(_)
        ));
    }

    #[test]
    fn implied_li_close_splits_items() {
        let Block::BulletList(items) = first_block("<ul><li>a<li>b</ul>") else {
            panic!("expected bullet list");
        };
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn pre_with_code_language_class_becomes_code_block() {
        let Block::CodeBlock(attr, text) = first_block(
            r#"<pre><code class="language-rust">let x = 1;
</code></pre>"#,
        ) else {
            panic!("expected code block");
        };
        assert_eq!(attr.classes, vec!["rust".to_string()]);
        assert_eq!(text, "let x = 1;");
    }

    #[test]
    fn definition_list_pairs_terms_and_definitions() {
        let Block::DefinitionList(items) =
            first_block("<dl><dt>term</dt><dd>one</dd><dd>two</dd></dl>")
        else {
            panic!("expected definition list");
        };
        let (term, defs) = items.into_iter().next().expect("an item");
        assert_eq!(term, vec![Inline::Str("term".to_string())]);
        assert_eq!(defs.len(), 2);
    }

    #[test]
    fn blockquote_wraps_child_blocks() {
        assert!(matches!(
            first_block("<blockquote><p>q</p></blockquote>"),
            Block::BlockQuote(_)
        ));
    }

    #[test]
    fn sectioning_div_gets_a_class() {
        let Block::Div(attr, _) = first_block("<section><p>x</p></section>") else {
            panic!("expected div");
        };
        assert!(attr.classes.contains(&"section".to_string()));
    }

    #[test]
    fn figure_separates_caption_from_content() {
        let Block::Figure(_, caption, content) =
            first_block("<figure><img src=\"a.png\"><figcaption>cap</figcaption></figure>")
        else {
            panic!("expected figure");
        };
        assert_eq!(caption.short, None);
        assert!(!caption.long.is_empty());
        assert!(!content.is_empty());
    }

    #[test]
    fn table_reads_sections_alignment_and_spans() {
        let input = r#"<table>
            <caption>cap</caption>
            <colgroup><col style="width: 25%"><col></colgroup>
            <thead><tr><th align="right">H1</th><th>H2</th></tr></thead>
            <tbody><tr><td colspan="2">wide</td></tr></tbody>
            <tfoot><tr><td>f1</td><td>f2</td></tr></tfoot>
        </table>"#;
        let Block::Table(table) = first_block(input) else {
            panic!("expected table");
        };
        assert_eq!(table.col_specs.len(), 2);
        assert_eq!(
            table.col_specs.first().map(|spec| spec.width.clone()),
            Some(ColWidth::ColWidth(0.25))
        );
        assert_eq!(
            table
                .head
                .rows
                .first()
                .and_then(|row| row.cells.first())
                .map(|cell| cell.align.clone()),
            Some(Alignment::AlignRight)
        );
        let body_cell_span = table
            .bodies
            .first()
            .and_then(|body| body.body.first())
            .and_then(|row| row.cells.first())
            .map(|cell| cell.col_span);
        assert_eq!(body_cell_span, Some(2));
        assert_eq!(table.foot.rows.len(), 1);
    }

    #[test]
    fn oversized_cell_spans_are_clamped() {
        // A cell span materialises one grid slot per spanned column (and a carry per spanned row),
        // so an unbounded `colspan="90000000"` once forced a multi-gigabyte allocation that a
        // nightly fuzz run hit as an out-of-memory crash. Spans are now clamped to the HTML spec's
        // limits, keeping the input parseable in bounded memory.
        let input = r#"<table><tr><td colspan="90000000" rowspan="2">x</td></tr><tr><td>y</td></tr></table>"#;
        let Block::Table(table) = first_block(input) else {
            panic!("expected table");
        };
        let cell_span = table
            .bodies
            .first()
            .and_then(|body| body.body.first())
            .and_then(|row| row.cells.first())
            .map(|cell| (cell.col_span, cell.row_span));
        assert_eq!(cell_span, Some((1000, 2)));
    }

    #[test]
    fn cell_alignment_reads_text_align_style() {
        let Block::Table(table) =
            first_block(r#"<table><tr><td style="text-align: center">c</td></tr></table>"#)
        else {
            panic!("expected table");
        };
        let align = table
            .bodies
            .first()
            .and_then(|body| body.body.first())
            .and_then(|row| row.cells.first())
            .map(|cell| cell.align.clone());
        assert_eq!(align, Some(Alignment::AlignCenter));
    }

    #[test]
    fn every_inline_emphasis_kind_is_mapped() {
        let inlines = para_inlines(
            "<p><em>a</em><b>b</b><del>c</del><u>d</u><sup>e</sup><sub>f</sub><q>g</q></p>",
        );
        assert!(matches!(
            inlines.as_slice(),
            [
                Inline::Emph(_),
                Inline::Strong(_),
                Inline::Strikeout(_),
                Inline::Underline(_),
                Inline::Superscript(_),
                Inline::Subscript(_),
                Inline::Quoted(_, _),
            ]
        ));
    }

    #[test]
    fn class_carrying_inlines_become_spans() {
        let inlines = para_inlines("<p><mark>m</mark><kbd>k</kbd></p>");
        let classes: Vec<&str> = inlines
            .iter()
            .filter_map(|inline| match inline {
                Inline::Span(attr, _) => attr.classes.first().map(String::as_str),
                _ => None,
            })
            .collect();
        assert_eq!(classes, vec!["mark", "kbd"]);
    }

    #[test]
    fn code_variants_force_classes() {
        let inlines = para_inlines("<p><code>c</code><samp>s</samp><var>v</var></p>");
        let classes: Vec<Vec<String>> = inlines
            .iter()
            .filter_map(|inline| match inline {
                Inline::Code(attr, _) => Some(attr.classes.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            classes,
            vec![
                Vec::<String>::new(),
                vec!["sample".to_string()],
                vec!["variable".to_string()],
            ]
        );
    }

    #[test]
    fn line_break_element_becomes_line_break() {
        let inlines = para_inlines("<p>a<br>b</p>");
        assert!(inlines.contains(&Inline::LineBreak));
    }

    #[test]
    fn anchor_with_href_is_a_link() {
        let inlines = para_inlines(r#"<p><a href="/u" title="T" class="x">t</a></p>"#);
        let Some(Inline::Link(attr, _, target)) = inlines.first() else {
            panic!("expected link");
        };
        assert_eq!(
            *target,
            Box::new(Target {
                url: "/u".to_string(),
                title: "T".to_string()
            })
        );
        assert!(attr.classes.contains(&"x".to_string()));
    }

    #[test]
    fn anchor_with_name_is_a_span_with_id() {
        let inlines = para_inlines(r#"<p><a name="anchor">t</a></p>"#);
        let Some(Inline::Span(attr, _)) = inlines.first() else {
            panic!("expected span");
        };
        assert_eq!(attr.id, "anchor");
    }

    #[test]
    fn image_reads_src_title_and_alt() {
        let inlines = para_inlines(r#"<p><img src="a.png" title="T" alt="alt text"></p>"#);
        let Some(Inline::Image(_, alt, target)) = inlines.first() else {
            panic!("expected image");
        };
        assert_eq!(target.url, "a.png");
        assert_eq!(target.title, "T");
        assert_eq!(
            alt.as_slice(),
            [
                Inline::Str("alt".to_string()),
                Inline::Space,
                Inline::Str("text".to_string())
            ]
        );
    }

    #[test]
    fn unknown_inline_element_is_transparent() {
        let inlines = para_inlines("<p>a<bogus>b</bogus>c</p>");
        assert_eq!(inlines.as_slice(), [Inline::Str("abc".to_string())]);
    }

    #[test]
    fn data_attributes_drop_their_prefix() {
        let Block::Div(attr, _) = first_block(r#"<div id="d" data-role="note">x</div>"#) else {
            panic!("expected div");
        };
        assert_eq!(attr.id, "d");
        assert!(
            attr.attributes
                .contains(&("role".to_string(), "note".to_string()))
        );
    }

    #[test]
    fn boolean_and_unquoted_attributes_parse() {
        let Block::OrderedList(attrs, _) = first_block("<ol reversed start=5><li>a</li></ol>")
        else {
            panic!("expected ordered list");
        };
        assert_eq!(attrs.start, 5);
    }

    #[test]
    fn numeric_and_named_references_decode() {
        let inlines = para_inlines("<p>&#65;&#x42;&#X43;&copy</p>");
        assert_eq!(inlines.as_slice(), [Inline::Str("ABC\u{a9}".to_string())]);
    }

    #[test]
    fn unknown_entity_is_left_verbatim() {
        let inlines = para_inlines("<p>&notreal;</p>");
        assert_eq!(inlines.as_slice(), [Inline::Str("&notreal;".to_string())]);
    }

    #[test]
    fn style_block_is_dropped() {
        assert!(blocks("<style>p { color: red }</style><p>x</p>").len() == 1);
    }

    #[test]
    fn textarea_content_is_read_as_text() {
        let inlines = para_inlines("<p><textarea>typed &amp; ok</textarea></p>");
        assert!(
            inlines
                .iter()
                .any(|inline| matches!(inline, Inline::Str(s) if s.contains('&')))
        );
    }

    #[test]
    fn cdata_and_processing_instructions_are_skipped() {
        let inlines = para_inlines("<p>a<![CDATA[ junk ]]><?pi here?>b</p>");
        assert_eq!(inlines.as_slice(), [Inline::Str("ab".to_string())]);
    }

    #[test]
    fn doctype_declaration_is_skipped() {
        assert!(matches!(
            first_block("<!DOCTYPE html><p>x</p>"),
            Block::Para(_)
        ));
    }

    #[test]
    fn stray_less_than_is_literal_text() {
        let inlines = para_inlines("<p>a < b</p>");
        assert!(
            inlines
                .iter()
                .any(|inline| matches!(inline, Inline::Str(s) if s.contains('<')))
        );
    }

    #[test]
    fn self_closing_span_has_no_children() {
        let inlines = para_inlines("<p>a<span/>b</p>");
        assert!(
            inlines
                .iter()
                .any(|inline| matches!(inline, Inline::Span(_, children) if children.is_empty()))
        );
    }

    #[test]
    fn explicit_id_on_heading_is_preserved() {
        let Block::Header(_, attr, _) = first_block(r#"<h2 id="custom">Title</h2>"#) else {
            panic!("expected header");
        };
        assert_eq!(attr.id, "custom");
    }

    #[test]
    fn line_block_div_becomes_line_block() {
        let Block::LineBlock(lines) = first_block(r#"<div class="line-block">a<br>b</div>"#) else {
            panic!("expected line block");
        };
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn line_block_div_with_id_stays_div() {
        assert!(matches!(
            first_block(r#"<div class="line-block" id="x">a</div>"#),
            Block::Div(..)
        ));
    }

    #[test]
    fn inline_style_becomes_raw_html() {
        let inlines = para_inlines("<p>a<style>.x{}</style>b</p>");
        assert!(inlines.iter().any(|inline| matches!(
            inline,
            Inline::RawInline(format, text)
                if format.0 == "html" && text == "<style>.x{}</style>"
        )));
    }

    #[test]
    fn leading_style_block_is_dropped() {
        assert!(matches!(
            blocks("<style>.x{}</style><p>x</p>").as_slice(),
            [Block::Para(_)]
        ));
    }

    #[test]
    fn style_after_a_block_is_kept_as_a_raw_paragraph() {
        let result = blocks("<p>a</p>\n<style>.x{}</style>\n<p>b</p>");
        let [Block::Para(_), Block::Para(mid), Block::Para(_)] = result.as_slice() else {
            panic!("expected three paragraphs");
        };
        assert!(matches!(
            mid.as_slice(),
            [Inline::RawInline(format, text)]
                if format.0 == "html" && text == "<style>.x{}</style>"
        ));
    }

    #[test]
    fn style_directly_adjacent_to_a_block_is_dropped() {
        assert!(matches!(
            blocks("<p>a</p><style>.x{}</style><p>b</p>").as_slice(),
            [Block::Para(_), Block::Para(_)]
        ));
    }

    #[test]
    fn adjacent_styles_share_one_raw_paragraph() {
        let result = blocks("<p>a</p>\n<style>s1{}</style>\n<style>s2{}</style>\n<p>b</p>");
        let [_, Block::Para(mid), _] = result.as_slice() else {
            panic!("expected three paragraphs");
        };
        assert!(matches!(
            mid.as_slice(),
            [
                Inline::RawInline(f1, t1),
                Inline::SoftBreak,
                Inline::RawInline(f2, t2),
            ] if f1.0 == "html" && t1 == "<style>s1{}</style>"
                && f2.0 == "html" && t2 == "<style>s2{}</style>"
        ));
    }

    #[test]
    fn math_script_becomes_inline_math() {
        let inlines = para_inlines(r#"<p><script type="math/tex">\D</script></p>"#);
        assert!(matches!(
            inlines.as_slice(),
            [Inline::Math(MathType::InlineMath, text)] if text == "\\D"
        ));
    }

    #[test]
    fn display_math_script_becomes_display_math() {
        let inlines = para_inlines(r#"<p><script type="math/tex; mode=display">\D</script></p>"#);
        assert!(matches!(
            inlines.as_slice(),
            [Inline::Math(MathType::DisplayMath, _)]
        ));
    }

    #[test]
    fn non_math_script_is_dropped() {
        assert!(blocks("<p><script>run()</script></p>").is_empty());
    }

    #[test]
    fn checkbox_in_item_renders_ballot_box() {
        let Block::BulletList(items) =
            first_block(r#"<ul><li><input type="checkbox" checked/>do it</li></ul>"#)
        else {
            panic!("expected bullet list");
        };
        let Some([Block::Plain(inlines)]) = items.first().map(Vec::as_slice) else {
            panic!("expected one plain block");
        };
        assert!(matches!(inlines.first(), Some(Inline::Str(s)) if s == "\u{2612}"));
    }

    #[test]
    fn checkbox_outside_item_is_dropped() {
        let inlines = para_inlines(r#"<p><input type="checkbox"/>text</p>"#);
        assert_eq!(inlines.as_slice(), [Inline::Str("text".to_string())]);
    }

    #[test]
    fn paragraph_with_checkbox_demotes_to_plain() {
        assert!(matches!(
            first_block(r#"<p><input type="checkbox"/>x</p>"#),
            Block::Plain(_)
        ));
    }

    #[test]
    fn empty_paragraph_is_dropped() {
        assert!(blocks("<p>hi</p><p></p><p>lo</p>").len() == 2);
    }

    #[test]
    fn consecutive_terms_merge_with_line_break() {
        let Block::DefinitionList(items) = first_block("<dl><dt>a</dt><dt>b</dt><dd>x</dd></dl>")
        else {
            panic!("expected definition list");
        };
        let Some((term, _)) = items.first() else {
            panic!("expected one item");
        };
        assert!(term.contains(&Inline::LineBreak));
    }

    #[test]
    fn stray_paragraph_in_list_attaches_to_item() {
        let Block::BulletList(items) = first_block("<ul><li>a</li><p>b</p></ul>") else {
            panic!("expected bullet list");
        };
        assert_eq!(items.len(), 1);
        assert_eq!(items.first().map(Vec::len), Some(2));
    }

    #[test]
    fn native_divs_off_splices_div_children() {
        let result = read_with("<div class=\"c\"><p>x</p></div>", Extensions::empty());
        assert!(matches!(result.as_slice(), [Block::Para(_)]));
    }

    #[test]
    fn native_divs_off_drops_sectioning_wrapper() {
        let result = read_with("<section><p>x</p></section>", Extensions::empty());
        assert!(matches!(result.as_slice(), [Block::Para(_)]));
    }

    #[test]
    fn native_spans_off_unwraps_span_and_small_caps() {
        let plain = read_with("<p><span class=\"c\">x</span></p>", Extensions::empty());
        let Some(Block::Para(inlines)) = plain.first() else {
            panic!("expected paragraph");
        };
        assert_eq!(inlines.as_slice(), [Inline::Str("x".to_string())]);

        let caps = read_with(
            "<p><span style=\"font-variant: small-caps\">x</span></p>",
            Extensions::empty(),
        );
        let Some(Block::Para(inlines)) = caps.first() else {
            panic!("expected paragraph");
        };
        assert_eq!(inlines.as_slice(), [Inline::Str("x".to_string())]);
    }

    #[test]
    fn native_spans_off_keeps_class_carrying_inlines() {
        // `<mark>`/`<kbd>` and friends are their own constructs, not `<span>` elements, so the
        // toggle leaves them as spans.
        let result = read_with("<p><mark>m</mark></p>", Extensions::empty());
        let Some(Block::Para(inlines)) = result.first() else {
            panic!("expected paragraph");
        };
        assert!(matches!(inlines.first(), Some(Inline::Span(_, _))));
    }

    #[test]
    fn auto_identifiers_off_leaves_id_empty_but_keeps_explicit() {
        let generated = read_with("<h1>Hello World</h1>", Extensions::empty());
        let Some(Block::Header(_, attr, _)) = generated.first() else {
            panic!("expected header");
        };
        assert_eq!(attr.id, "");

        let explicit = read_with("<h2 id=\"keep\">T</h2>", Extensions::empty());
        let Some(Block::Header(_, attr, _)) = explicit.first() else {
            panic!("expected header");
        };
        assert_eq!(attr.id, "keep");
    }

    #[test]
    fn line_blocks_off_keeps_a_plain_div() {
        let result = read_with(
            "<div class=\"line-block\">a<br>b</div>",
            Extensions::from_list(&[Extension::NativeDivs]),
        );
        let Some(Block::Div(attr, children)) = result.first() else {
            panic!("expected div");
        };
        assert_eq!(attr.classes, vec!["line-block".to_string()]);
        assert!(matches!(children.as_slice(), [Block::Plain(_)]));
    }

    /// Read with the `html` default set plus the given text extensions, which is what `html+smart`
    /// and the `html+tex_math_*` corpus specs resolve to.
    fn read_with_text_ext(input: &str, added: &[Extension]) -> Vec<Block> {
        read_with(input, html_defaults().union(Extensions::from_list(added)))
    }

    fn para_inlines_ext(input: &str, added: &[Extension]) -> Vec<Inline> {
        match read_with_text_ext(input, added).into_iter().next() {
            Some(Block::Para(inlines) | Block::Plain(inlines)) => inlines,
            other => panic!("expected a paragraph, got {other:?}"),
        }
    }

    #[test]
    fn smart_off_keeps_literal_punctuation() {
        let inlines = para_inlines("<p>\"a\" -- ... ---</p>");
        assert_eq!(
            inlines.as_slice(),
            [
                Inline::Str("\"a\"".to_string()),
                Inline::Space,
                Inline::Str("--".to_string()),
                Inline::Space,
                Inline::Str("...".to_string()),
                Inline::Space,
                Inline::Str("---".to_string()),
            ]
        );
    }

    #[test]
    fn smart_on_curls_quotes_and_folds_dashes() {
        let inlines = para_inlines_ext("<p>\"a\" -- ... ---</p>", &[Extension::Smart]);
        assert_eq!(
            inlines.as_slice(),
            [
                Inline::Quoted(
                    carta_ast::QuoteType::DoubleQuote,
                    vec![Inline::Str("a".to_string())]
                ),
                Inline::Space,
                Inline::Str("\u{2013}".to_string()),
                Inline::Space,
                Inline::Str("\u{2026}".to_string()),
                Inline::Space,
                Inline::Str("\u{2014}".to_string()),
            ]
        );
    }

    #[test]
    fn tex_math_dollars_off_keeps_literal_text() {
        let inlines = para_inlines("<p>$x^2$ and $$y$$</p>");
        assert_eq!(
            inlines.as_slice(),
            [
                Inline::Str("$x^2$".to_string()),
                Inline::Space,
                Inline::Str("and".to_string()),
                Inline::Space,
                Inline::Str("$$y$$".to_string()),
            ]
        );
    }

    #[test]
    fn tex_math_dollars_on_splits_inline_and_display() {
        let inlines = para_inlines_ext("<p>$x^2$ and $$y$$</p>", &[Extension::TexMathDollars]);
        assert_eq!(
            inlines.as_slice(),
            [
                Inline::Math(MathType::InlineMath, "x^2".to_string()),
                Inline::Space,
                Inline::Str("and".to_string()),
                Inline::Space,
                Inline::Math(MathType::DisplayMath, "y".to_string()),
            ]
        );
    }

    #[test]
    fn tex_math_single_backslash_on_splits_inline_and_display() {
        let inlines = para_inlines_ext(
            "<p>\\(x\\) and \\[y\\]</p>",
            &[Extension::TexMathSingleBackslash],
        );
        assert_eq!(
            inlines.as_slice(),
            [
                Inline::Math(MathType::InlineMath, "x".to_string()),
                Inline::Space,
                Inline::Str("and".to_string()),
                Inline::Space,
                Inline::Math(MathType::DisplayMath, "y".to_string()),
            ]
        );
    }

    #[test]
    fn tex_math_double_backslash_on_splits_inline_and_display() {
        let inlines = para_inlines_ext(
            "<p>\\\\(x\\\\) and \\\\[y\\\\]</p>",
            &[Extension::TexMathDoubleBackslash],
        );
        assert_eq!(
            inlines.as_slice(),
            [
                Inline::Math(MathType::InlineMath, "x".to_string()),
                Inline::Space,
                Inline::Str("and".to_string()),
                Inline::Space,
                Inline::Math(MathType::DisplayMath, "y".to_string()),
            ]
        );
    }

    #[test]
    fn note_reference_reconstructs_body_and_drops_container() {
        let result = blocks(concat!(
            "text<a href=\"#fn1\" class=\"footnote-ref\" role=\"doc-noteref\"><sup>1</sup></a>\n",
            "<section class=\"footnotes\" role=\"doc-endnotes\"><hr /><ol>",
            "<li id=\"fn1\"><p>the note",
            "<a href=\"#fnref1\" class=\"footnote-back\" role=\"doc-backlink\">\u{21a9}</a></p></li>",
            "</ol></section>",
        ));
        assert_eq!(
            result.as_slice(),
            [Block::Plain(vec![
                Inline::Str("text".to_string()),
                Inline::Note(vec![Block::Para(vec![
                    Inline::Str("the".to_string()),
                    Inline::Space,
                    Inline::Str("note".to_string()),
                ])]),
            ])]
        );
    }

    #[test]
    fn unmatched_note_reference_becomes_an_empty_note() {
        let result = blocks("text<a href=\"#missing\" role=\"doc-noteref\"><sup>1</sup></a>");
        assert_eq!(
            result.as_slice(),
            [Block::Plain(vec![
                Inline::Str("text".to_string()),
                Inline::Note(Vec::new()),
            ])]
        );
    }

    fn header_ids(input: &str, added: &[Extension]) -> Vec<String> {
        read_with_text_ext(input, added)
            .into_iter()
            .filter_map(|block| match block {
                Block::Header(_, attr, _) => Some(attr.id),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn gfm_auto_identifiers_drops_dots_keeps_digits_and_does_not_collapse() {
        // The `gfm_auto_identifiers` slug differs from the default: dots are dropped, leading digits
        // survive, and removed punctuation leaves its surrounding separators (no run collapsing).
        let ids = header_ids(
            "<h2>1.2 Section A.B</h2><h2>Tools &amp; Tips</h2>",
            &[Extension::GfmAutoIdentifiers],
        );
        assert_eq!(ids, vec!["12-section-ab", "tools--tips"]);
    }

    #[test]
    fn gfm_auto_identifiers_keep_the_section_fallback_and_increment_on_collision() {
        let ids = header_ids(
            "<h2>Repeat</h2><h2>Repeat</h2><h3>!!!</h3>",
            &[Extension::GfmAutoIdentifiers],
        );
        assert_eq!(ids, vec!["repeat", "repeat-1", "section"]);
    }

    #[test]
    fn gfm_auto_identifiers_need_auto_identifiers_to_take_effect() {
        let ids = read_with(
            "<h2>1.2 Section A.B</h2>",
            Extensions::from_list(&[Extension::GfmAutoIdentifiers]),
        )
        .into_iter()
        .filter_map(|block| match block {
            Block::Header(_, attr, _) => Some(attr.id),
            _ => None,
        })
        .collect::<Vec<_>>();
        assert_eq!(ids, vec![String::new()]);
    }

    #[cfg(feature = "opml")]
    #[test]
    fn inline_fragment_parses_markup_and_trims_edges() {
        let inlines = super::parse_inline_fragment("  <strong>a</strong> b <code>c</code>  ");
        assert_eq!(
            inlines,
            vec![
                Inline::Strong(vec![Inline::Str("a".to_string())]),
                Inline::Space,
                Inline::Str("b".to_string()),
                Inline::Space,
                Inline::Code(Box::default(), "c".to_string()),
            ]
        );
    }

    #[cfg(feature = "opml")]
    #[test]
    fn inline_fragment_resolves_character_references() {
        let inlines = super::parse_inline_fragment("a &amp; b");
        assert_eq!(
            inlines,
            vec![
                Inline::Str("a".to_string()),
                Inline::Space,
                Inline::Str("&".to_string()),
                Inline::Space,
                Inline::Str("b".to_string()),
            ]
        );
    }

    #[cfg(feature = "opml")]
    fn raw(tag: &str) -> Inline {
        Inline::RawInline(carta_ast::Format("html".to_string()), tag.to_string())
    }

    #[cfg(feature = "opml")]
    #[test]
    fn inline_fragment_preserves_an_unrecognized_tag_verbatim() {
        let inlines = super::parse_inline_fragment("<cite>Book</cite>");
        assert_eq!(
            inlines,
            vec![
                raw("<cite>"),
                Inline::Str("Book".to_string()),
                raw("</cite>")
            ]
        );
    }

    #[cfg(feature = "opml")]
    #[test]
    fn inline_fragment_keeps_unknown_tag_attributes() {
        let inlines = super::parse_inline_fragment("<time datetime=\"2020\">y</time>");
        assert_eq!(
            inlines,
            vec![
                raw("<time datetime=\"2020\">"),
                Inline::Str("y".to_string()),
                raw("</time>"),
            ]
        );
    }

    #[cfg(feature = "opml")]
    #[test]
    fn inline_fragment_escapes_attribute_values_and_emits_bare_boolean() {
        let inlines = super::parse_inline_fragment("<x-foo a=\"1<2&3\" hidden>z</x-foo>");
        assert_eq!(
            inlines,
            vec![
                raw("<x-foo a=\"1&lt;2&amp;3\" hidden>"),
                Inline::Str("z".to_string()),
                raw("</x-foo>"),
            ]
        );
    }

    #[cfg(feature = "opml")]
    #[test]
    fn inline_fragment_lowercases_an_unknown_tag_name() {
        let inlines = super::parse_inline_fragment("<CITE>b</CITE>");
        assert_eq!(
            inlines,
            vec![raw("<cite>"), Inline::Str("b".to_string()), raw("</cite>")]
        );
    }

    #[cfg(feature = "opml")]
    #[test]
    fn inline_fragment_void_unknown_tag_is_a_single_raw_inline() {
        let inlines = super::parse_inline_fragment("a <wbr> b");
        assert_eq!(
            inlines,
            vec![
                Inline::Str("a".to_string()),
                Inline::Space,
                raw("<wbr>"),
                Inline::Space,
                Inline::Str("b".to_string()),
            ]
        );
    }

    #[cfg(feature = "opml")]
    #[test]
    fn inline_fragment_self_closing_unknown_tag_pairs_open_and_close() {
        let inlines = super::parse_inline_fragment("<custom-tag/>");
        assert_eq!(inlines, vec![raw("<custom-tag>"), raw("</custom-tag>")]);
    }

    #[cfg(feature = "opml")]
    #[test]
    fn inline_fragment_unclosed_unknown_tag_omits_the_close() {
        let inlines = super::parse_inline_fragment("a <cite>open-only");
        assert_eq!(
            inlines,
            vec![
                Inline::Str("a".to_string()),
                Inline::Space,
                raw("<cite>"),
                Inline::Str("open-only".to_string()),
            ]
        );
    }

    #[cfg(feature = "opml")]
    #[test]
    fn inline_fragment_stray_unknown_end_tag_is_preserved() {
        let inlines = super::parse_inline_fragment("</cite> tail");
        assert_eq!(
            inlines,
            vec![
                raw("</cite>"),
                Inline::Space,
                Inline::Str("tail".to_string()),
            ]
        );
    }

    #[cfg(feature = "opml")]
    #[test]
    fn inline_fragment_unknown_tag_wraps_recognized_inner_markup() {
        let inlines = super::parse_inline_fragment("<cite><em>x</em></cite>");
        assert_eq!(
            inlines,
            vec![
                raw("<cite>"),
                Inline::Emph(vec![Inline::Str("x".to_string())]),
                raw("</cite>"),
            ]
        );
    }

    #[cfg(feature = "opml")]
    #[test]
    fn inline_fragment_recognized_tags_keep_structural_mapping() {
        let inlines = super::parse_inline_fragment("<em>e</em> <strong>s</strong> <sup>2</sup>");
        assert_eq!(
            inlines,
            vec![
                Inline::Emph(vec![Inline::Str("e".to_string())]),
                Inline::Space,
                Inline::Strong(vec![Inline::Str("s".to_string())]),
                Inline::Space,
                Inline::Superscript(vec![Inline::Str("2".to_string())]),
            ]
        );
    }
}
