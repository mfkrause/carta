use super::helpers::custom_style_attr;
use super::*;
use carta_core::container::zip::ZipArchive;

/// Packages a bare `word/document.xml` body into a minimal archive the reader accepts, so a
/// hand-built story can be fed through the full byte-input path.
fn docx_from_body(body: &str) -> Vec<u8> {
    docx_from_parts(body, None, None)
}

/// Packages a `word/document.xml` body alongside optional `styles.xml` and `numbering.xml`
/// parts. The parts sit at their conventional names, which the reader resolves without any
/// relationship entries.
fn docx_from_parts(body: &str, styles: Option<&str>, numbering: Option<&str>) -> Vec<u8> {
    const NS: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
         xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\"";
    let document =
        format!("<?xml version=\"1.0\"?><w:document {NS}><w:body>{body}</w:body></w:document>");
    let mut archive = ZipArchive::new();
    archive
        .deflate("word/document.xml", document.as_bytes())
        .expect("store document part");
    if let Some(styles) = styles {
        let xml = format!("<?xml version=\"1.0\"?><w:styles {NS}>{styles}</w:styles>");
        archive
            .deflate("word/styles.xml", xml.as_bytes())
            .expect("store styles part");
    }
    if let Some(numbering) = numbering {
        let xml = format!("<?xml version=\"1.0\"?><w:numbering {NS}>{numbering}</w:numbering>");
        archive
            .deflate("word/numbering.xml", xml.as_bytes())
            .expect("store numbering part");
    }
    archive.finish().expect("finish archive")
}

#[test]
fn deeply_nested_hyperlinks_do_not_overflow_the_stack() {
    // Exceed the inline depth limit within the XML limit to check stack safety.
    let depth = 2_000;
    let body = format!(
        "<w:p>{}<w:r><w:t>x</w:t></w:r>{}</w:p>",
        "<w:hyperlink w:anchor=\"a\">".repeat(depth),
        "</w:hyperlink>".repeat(depth)
    );
    let archive = docx_from_body(&body);
    assert!(DocxReader.read(&archive, &ReaderOptions::default()).is_ok());
}

/// The `word/document.xml` body of `depth` tables nested one inside another, innermost cell
/// holding a single marker paragraph.
fn nested_tables(depth: usize) -> String {
    let mut body = String::from("<w:p><w:r><w:t>core</w:t></w:r></w:p>");
    for _ in 0..depth {
        body = format!(
            "<w:tbl><w:tblGrid><w:gridCol w:w=\"5000\"/></w:tblGrid>\
             <w:tr><w:tc>{body}</w:tc></w:tr></w:tbl>"
        );
    }
    body
}

/// The deepest cell content of a chain of singly-nested tables, and how many tables were
/// descended to reach it.
fn descend_tables(blocks: &[Block]) -> (usize, &[Block]) {
    match blocks.first() {
        Some(Block::Table(table)) => {
            match table
                .bodies
                .first()
                .and_then(|section| section.body.first())
                .and_then(|row| row.cells.first())
            {
                Some(cell) => {
                    let (deeper, content) = descend_tables(&cell.content);
                    (deeper + 1, content)
                }
                None => (0, blocks),
            }
        }
        _ => (0, blocks),
    }
}

#[test]
fn deeply_nested_tables_are_preserved_to_the_scanner_ceiling() {
    // Use a large stack to drop 1000 nested tables below the scanner limit.
    let outcome = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            let archive = docx_from_body(&nested_tables(1000));
            let document = DocxReader
                .read(&archive, &ReaderOptions::default())
                .expect("read deeply nested tables");
            let (depth, innermost) = descend_tables(&document.blocks);
            (
                depth,
                innermost == [Block::Plain(vec![Inline::Str("core".into())])],
            )
        })
        .expect("spawn worker")
        .join()
        .expect("join worker");
    assert_eq!(outcome, (1000, true));
}

#[test]
fn custom_styled_list_items_carry_a_style_container_only_under_the_styles_extension() {
    let styles = "<w:style w:type=\"paragraph\" w:styleId=\"ListParagraph\">\
         <w:name w:val=\"List Paragraph\"/></w:style>";
    let numbering = "<w:abstractNum w:abstractNumId=\"0\"><w:lvl w:ilvl=\"0\">\
         <w:numFmt w:val=\"bullet\"/><w:lvlText w:val=\"o\"/></w:lvl></w:abstractNum>\
         <w:num w:numId=\"1\"><w:abstractNumId w:val=\"0\"/></w:num>";
    let item = |text: &str| {
        format!(
            "<w:p><w:pPr><w:pStyle w:val=\"ListParagraph\"/>\
             <w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr></w:pPr>\
             <w:r><w:t>{text}</w:t></w:r></w:p>"
        )
    };
    let body = format!("{}{}", item("alpha"), item("beta"));
    let archive = docx_from_parts(&body, Some(styles), Some(numbering));

    let plain = |text: &str| Block::Para(vec![Inline::Str(text.into())]);
    let default = DocxReader
        .read(&archive, &ReaderOptions::default())
        .expect("read without styles");
    assert_eq!(
        default.blocks,
        vec![Block::BulletList(vec![
            vec![plain("alpha")],
            vec![plain("beta")],
        ])]
    );

    let mut options = ReaderOptions::default();
    options.extensions.insert(Extension::Styles);
    let with_styles = DocxReader
        .read(&archive, &options)
        .expect("read with styles");
    let wrapped = |text: &str| {
        Block::Div(
            Box::new(custom_style_attr("List Paragraph")),
            vec![plain(text)],
        )
    };
    assert_eq!(
        with_styles.blocks,
        vec![Block::BulletList(vec![
            vec![wrapped("alpha")],
            vec![wrapped("beta")],
        ])]
    );
}

#[test]
fn run_toggle_off_value_disables_the_property() {
    let run = |toggle: &str, text: &str| {
        format!("<w:p><w:r><w:rPr>{toggle}</w:rPr><w:t>{text}</w:t></w:r></w:p>")
    };
    let body = format!(
        "{}{}{}",
        run("<w:b w:val=\"off\"/>", "off"),
        run("<w:b w:val=\"false\"/>", "false"),
        run("<w:b/>", "on"),
    );
    let document = DocxReader
        .read(&docx_from_body(&body), &ReaderOptions::default())
        .expect("read toggle runs");
    assert_eq!(
        document.blocks,
        vec![
            Block::Para(vec![Inline::Str("off".into())]),
            Block::Para(vec![Inline::Str("false".into())]),
            Block::Para(vec![Inline::Strong(vec![Inline::Str("on".into())])]),
        ]
    );
}

#[test]
fn tbl_look_hex_val_first_row_bit_promotes_the_header() {
    let table = |look: &str| {
        format!(
            "<w:tbl><w:tblPr><w:tblLook w:val=\"{look}\"/></w:tblPr>\
             <w:tblGrid><w:gridCol w:w=\"100\"/></w:tblGrid>\
             <w:tr><w:tc><w:p><w:r><w:t>H</w:t></w:r></w:p></w:tc></w:tr>\
             <w:tr><w:tc><w:p><w:r><w:t>B</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"
        )
    };
    let head_rows = |look: &str| {
        let body = table(look);
        let document = DocxReader
            .read(&docx_from_body(&body), &ReaderOptions::default())
            .expect("read table");
        match document.blocks.first() {
            Some(Block::Table(t)) => t.head.rows.len(),
            other => panic!("expected a table, found {other:?}"),
        }
    };
    // Bit 0x0020 of the packed look bitmask selects the first row for header promotion.
    assert_eq!(head_rows("04A0"), 1);
    assert_eq!(head_rows("0480"), 0);
}

#[test]
fn tbl_header_row_marker_honors_an_explicit_off_value() {
    let head_rows = |marker: &str| {
        let body = format!(
            "<w:tbl><w:tblGrid><w:gridCol w:w=\"100\"/></w:tblGrid>\
             <w:tr><w:trPr>{marker}</w:trPr><w:tc><w:p><w:r><w:t>H</w:t></w:r></w:p></w:tc></w:tr>\
             <w:tr><w:tc><w:p><w:r><w:t>B</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"
        );
        let document = DocxReader
            .read(&docx_from_body(&body), &ReaderOptions::default())
            .expect("read table");
        match document.blocks.first() {
            Some(Block::Table(t)) => t.head.rows.len(),
            other => panic!("expected a table, found {other:?}"),
        }
    };
    assert_eq!(head_rows("<w:tblHeader/>"), 1);
    assert_eq!(head_rows("<w:tblHeader w:val=\"0\"/>"), 0);
    // Only the literal `0` disables the marker; it lacks the run toggles' broader off spellings.
    assert_eq!(head_rows("<w:tblHeader w:val=\"false\"/>"), 1);
    assert_eq!(head_rows("<w:tblHeader w:val=\"1\"/>"), 1);
}

fn text_box(body: &str) -> String {
    format!(
        "<w:pict xmlns:v=\"urn:schemas-microsoft-com:vml\"><v:shape><v:textbox>\
         <w:txbxContent>{body}</w:txbxContent></v:textbox></v:shape></w:pict>"
    )
}

#[test]
fn page_and_column_breaks_keep_words_separate() {
    for kind in ["textWrapping", "page", "column"] {
        let body = format!(
            "<w:p><w:r><w:t>before</w:t><w:br w:type=\"{kind}\"/>\
             <w:t>after</w:t></w:r></w:p>"
        );
        let document = DocxReader
            .read(&docx_from_body(&body), &ReaderOptions::default())
            .unwrap();
        assert_eq!(
            document.blocks,
            vec![Block::Para(vec![
                Inline::Str("before".into()),
                Inline::LineBreak,
                Inline::Str("after".into()),
            ])],
            "{kind}"
        );
        let root = xml::parse_tolerant(body.as_bytes(), MAX_XML_DEPTH);
        let paragraph = root.child("p").unwrap();
        assert_eq!(
            inline::code_paragraph_text(paragraph),
            "before\nafter",
            "{kind}"
        );
    }
}

#[test]
fn text_boxes_keep_blocks_formatting_and_surrounding_text() {
    let boxed = text_box(
        "<w:p><w:r><w:t>Box </w:t></w:r><w:r><w:rPr><w:i/></w:rPr>\
         <w:t>text</w:t></w:r></w:p><w:p><w:r><w:t>next</w:t></w:r></w:p>",
    );
    let body = format!("<w:p><w:r><w:t>before </w:t>{boxed}<w:t> after</w:t></w:r></w:p>");
    let document = DocxReader
        .read(&docx_from_body(&body), &ReaderOptions::default())
        .unwrap();
    let root = xml::parse_tolerant(body.as_bytes(), MAX_XML_DEPTH);
    assert_eq!(
        inline::code_paragraph_text(root.child("p").unwrap()),
        "before  after"
    );
    assert_eq!(
        document.blocks,
        vec![
            Block::Para(vec![
                Inline::Str("before".into()),
                Inline::Space,
                Inline::Str("after".into())
            ]),
            Block::Para(vec![
                Inline::Str("Box".into()),
                Inline::Space,
                Inline::Emph(vec![Inline::Str("text".into())])
            ]),
            Block::Para(vec![Inline::Str("next".into())]),
        ]
    );
}

#[test]
fn text_boxes_select_one_compatibility_branch() {
    let choice = text_box("<w:p><w:r><w:t>choice</w:t></w:r></w:p>");
    let fallback = text_box("<w:p><w:r><w:t>fallback</w:t></w:r></w:p>");
    for has_fallback in [false, true] {
        let fallback = if has_fallback {
            format!("<mc:Fallback>{fallback}</mc:Fallback>")
        } else {
            String::new()
        };
        let body = format!(
            "<w:p><w:r><mc:AlternateContent \
             xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" \
             xmlns:v=\"urn:schemas-microsoft-com:vml\">\
             <mc:Choice Requires=\"v\">{choice}</mc:Choice>{fallback}\
             </mc:AlternateContent></w:r></w:p>"
        );
        let document = DocxReader
            .read(&docx_from_body(&body), &ReaderOptions::default())
            .unwrap();
        let expected = if has_fallback { "fallback" } else { "choice" };
        assert_eq!(
            document.blocks,
            vec![Block::Para(vec![Inline::Str(expected.into())])]
        );
    }
}

#[test]
fn text_box_images_stay_inside_their_own_paragraphs() {
    let boxed = text_box(
        "<w:p><w:r><w:t>Box</w:t></w:r></w:p><w:p><w:r><w:pict>\
         <v:shape><v:imagedata xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
         r:id=\"image\"/></v:shape></w:pict></w:r></w:p>",
    );
    let body = format!("<w:p><w:r>{boxed}</w:r></w:p>");
    let mut parts = zip::read_map(&docx_from_body(&body)).unwrap();
    parts.insert("word/_rels/document.xml.rels".into(),
        b"<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
          <Relationship Id=\"image\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" \
          Target=\"https://example.test/image.png\" TargetMode=\"External\"/></Relationships>".to_vec());
    let mut archive = ZipArchive::new();
    for (path, contents) in parts {
        archive.deflate(&path, &contents).unwrap();
    }
    let document = DocxReader
        .read(&archive.finish().unwrap(), &ReaderOptions::default())
        .unwrap();
    assert_eq!(document.blocks.len(), 2);
    assert_eq!(
        document.blocks.first(),
        Some(&Block::Para(vec![Inline::Str("Box".into())]))
    );
    assert!(matches!(document.blocks.get(1), Some(Block::Para(inlines))
        if matches!(inlines.as_slice(), [Inline::Image(_, _, target)]
            if target.url == "https://example.test/image.png")));
}

#[test]
fn nested_text_boxes_keep_the_innermost_text() {
    let mut body = "<w:p><w:r><w:t>core</w:t></w:r></w:p>".to_owned();
    for _ in 0..128 {
        body = format!("<w:p><w:r>{}</w:r></w:p>", text_box(&body));
    }
    let document = DocxReader
        .read(&docx_from_body(&body), &ReaderOptions::default())
        .unwrap();
    assert_eq!(
        document.blocks,
        vec![Block::Para(vec![Inline::Str("core".into())])]
    );
}

#[test]
fn text_box_traversal_keeps_siblings_and_skips_deletions() {
    let first = text_box("<w:p><w:r><w:t>first</w:t></w:r></w:p>");
    let deleted = text_box("<w:p><w:r><w:t>deleted</w:t></w:r></w:p>");
    let second = text_box("<w:p><w:r><w:t>second</w:t></w:r></w:p>");
    for deletion in ["del", "moveFrom"] {
        let body = format!(
            "<w:p><w:r>{first}</w:r><w:{deletion}><w:r>{deleted}</w:r></w:{deletion}>\
             <w:r>{second}</w:r></w:p>"
        );
        let document = DocxReader
            .read(&docx_from_body(&body), &ReaderOptions::default())
            .unwrap();
        assert_eq!(
            document.blocks,
            vec![
                Block::Para(vec![Inline::Str("first".into())]),
                Block::Para(vec![Inline::Str("second".into())]),
            ]
        );
    }
}

#[test]
fn math_symbols_use_canonical_commands_without_trailing_spaces() {
    let archive = docx_from_body("<w:p><m:oMath><m:r><m:t>→∅·α</m:t></m:r></m:oMath></w:p>");
    let document = DocxReader
        .read(&archive, &ReaderOptions::default())
        .unwrap();
    assert_eq!(
        document.blocks,
        vec![Block::Para(vec![Inline::Math(
            carta_ast::MathType::InlineMath,
            "\\rightarrow \\varnothing \\cdot \\alpha".into(),
        )])]
    );
}
