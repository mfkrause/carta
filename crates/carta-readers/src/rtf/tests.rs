use super::*;

use carta_ast::{
    Attr, Block, Inline, ListAttributes, ListNumberDelim, ListNumberStyle, MetaValue, Target, Text,
};
use carta_core::media::content_addressed_name;

fn read(input: &str) -> Document {
    read_bytes(input.as_bytes())
}

fn read_bytes(input: &[u8]) -> Document {
    RtfReader
        .read(input, &ReaderOptions::default())
        .expect("read")
}

fn read_media(input: &str) -> (Document, MediaBag) {
    read_media_bytes(input.as_bytes())
}

fn read_media_bytes(input: &[u8]) -> (Document, MediaBag) {
    RtfReader
        .read_media(input, &ReaderOptions::default())
        .expect("read")
}

fn para(inlines: Vec<Inline>) -> Block {
    Block::Para(inlines)
}

fn s(text: &str) -> Inline {
    Inline::Str(text.into())
}

#[test]
fn plain_paragraph_splits_words() {
    let doc = read(r"{\rtf1\ansi Hello world.\par}");
    assert_eq!(
        doc.blocks,
        vec![para(vec![s("Hello"), Inline::Space, s("world.")])]
    );
}

#[test]
fn collapses_runs_of_spaces() {
    let doc = read(r"{\rtf1\ansi a  b   c\par}");
    assert_eq!(
        doc.blocks,
        vec![para(vec![
            s("a"),
            Inline::Space,
            s("b"),
            Inline::Space,
            s("c"),
        ])]
    );
}

#[test]
fn bold_and_italic_map_to_strong_and_emph() {
    let doc = read(r"{\rtf1\ansi \b bold\b0  normal\par}");
    assert_eq!(
        doc.blocks,
        vec![para(vec![
            Inline::Strong(vec![s("bold")]),
            Inline::Space,
            s("normal"),
        ])]
    );
    let doc = read(r"{\rtf1\ansi \i italic\i0\par}");
    assert_eq!(
        doc.blocks,
        vec![para(vec![Inline::Emph(vec![s("italic")])])]
    );
}

#[test]
fn style_reference_inherits_character_formatting() {
    let doc = read(r"{\rtf1\ansi{\stylesheet{\s1\i Emphasis;}}\pard\s1 italic text\par}");
    assert_eq!(
        doc.blocks,
        vec![para(vec![Inline::Emph(vec![
            s("italic"),
            Inline::Space,
            s("text"),
        ])])]
    );
}

#[test]
fn default_style_applies_to_unstyled_paragraphs() {
    let doc = read(r"{\rtf1\ansi{\stylesheet{\s0\b Normal;}}\pard first\par\pard second\par}");
    assert_eq!(
        doc.blocks,
        vec![
            para(vec![Inline::Strong(vec![s("first")])]),
            para(vec![Inline::Strong(vec![s("second")])]),
        ]
    );
}

#[test]
fn style_formatting_overlays_default_style() {
    // `\s0` sets bold for every paragraph and `\s1` adds italic, so the run is both.
    let doc = read(r"{\rtf1\ansi{\stylesheet{\s0\b Normal;}{\s1\i Emphasis;}}\pard\s1 word\par}");
    assert_eq!(
        doc.blocks,
        vec![para(vec![Inline::Strong(vec![Inline::Emph(vec![s(
            "word"
        )])])])]
    );
}

#[test]
fn whole_heading_emphasis_is_stripped() {
    // A fully bold heading drops the emphasis; the level already conveys prominence.
    let doc =
        read(r"{\rtf1\ansi{\stylesheet{\s1\outlinelevel0\b Heading;}}\pard\s1 the title\par}");
    assert_eq!(
        doc.blocks,
        vec![Block::Header(
            1,
            Box::default(),
            vec![s("the"), Inline::Space, s("title")],
        )]
    );
}

#[test]
fn nesting_order_is_fixed() {
    let doc = read(r"{\rtf1\ansi \i\b x\par}");
    assert_eq!(
        doc.blocks,
        vec![para(vec![Inline::Strong(vec![Inline::Emph(vec![s("x")])])])]
    );
}

#[test]
fn shared_formatting_coalesces_across_inner_group() {
    let doc = read(r"{\rtf1\ansi \b bold {\i both} stillbold\b0 normal\par}");
    assert_eq!(
        doc.blocks,
        vec![para(vec![
            Inline::Strong(vec![
                s("bold"),
                Inline::Space,
                Inline::Emph(vec![s("both")]),
                Inline::Space,
                s("stillbold"),
            ]),
            s("normal"),
        ])]
    );
}

#[test]
fn formatting_persists_across_par_but_pard_resets() {
    let doc = read(r"{\rtf1\ansi \b bold\par next\par}");
    assert_eq!(
        doc.blocks,
        vec![
            para(vec![Inline::Strong(vec![s("bold")])]),
            para(vec![Inline::Strong(vec![s("next")])]),
        ]
    );
    let doc = read(r"{\rtf1\ansi \b bold\par\pard normal\par}");
    assert_eq!(
        doc.blocks,
        vec![
            para(vec![Inline::Strong(vec![s("bold")])]),
            para(vec![s("normal")]),
        ]
    );
}

#[test]
fn line_break_and_tab() {
    let doc = read(r"{\rtf1\ansi one\line two\par}");
    assert_eq!(
        doc.blocks,
        vec![para(vec![s("one"), Inline::LineBreak, s("two")])]
    );
    let doc = read(r"{\rtf1\ansi a\tab b\par}");
    assert_eq!(doc.blocks, vec![para(vec![s("a"), Inline::Space, s("b")])]);
}

#[test]
fn trailing_line_break_at_paragraph_boundary_is_dropped() {
    // A single break just before the paragraph mark carries no line and is removed.
    let doc = read(r"{\rtf1\ansi a\line\par}");
    assert_eq!(doc.blocks, vec![para(vec![s("a")])]);
    // Only one trailing break is removed; the earlier break stays.
    let doc = read(r"{\rtf1\ansi a\line\line\par}");
    assert_eq!(doc.blocks, vec![para(vec![s("a"), Inline::LineBreak])]);
    // A paragraph holding only a break becomes empty and is emitted as nothing.
    let doc = read(r"{\rtf1\ansi first\par\line\par second\par}");
    assert_eq!(
        doc.blocks,
        vec![para(vec![s("first")]), para(vec![s("second")])]
    );
    // The same trimming applies where a paragraph is closed by a cell or footnote boundary.
    let doc = read(r"{\rtf1\ansi x{\footnote note\line}\par}");
    assert_eq!(
        doc.blocks,
        vec![para(vec![
            s("x"),
            Inline::Note(vec![para(vec![s("note")])])
        ])]
    );
}

#[test]
fn escapes_and_special_characters() {
    let doc = read(r"{\rtf1\ansi a\{b\}c\\d\par}");
    assert_eq!(doc.blocks, vec![para(vec![s(r"a{b}c\d")])]);
    let doc = read(r"{\rtf1\ansi em\emdash dash\par}");
    assert_eq!(doc.blocks, vec![para(vec![s("em\u{2014}dash")])]);
    let doc = read(r"{\rtf1\ansi non\~breaking\par}");
    assert_eq!(doc.blocks, vec![para(vec![s("non\u{00A0}breaking")])]);
}

#[test]
fn hex_escape_uses_code_page() {
    let doc = read("{\\rtf1\\ansi caf\\'e9\\par}");
    assert_eq!(doc.blocks, vec![para(vec![s("caf\u{00E9}")])]);
    let doc = read("{\\rtf1\\ansi \\'80\\par}");
    assert_eq!(doc.blocks, vec![para(vec![s("\u{20AC}")])]);
}

#[test]
fn unicode_with_fallback_skip() {
    let doc = read(r"{\rtf1\ansi \u233 e\par}");
    assert_eq!(doc.blocks, vec![para(vec![s("\u{00E9}")])]);
    let doc = read(r"{\rtf1\ansi \uc2\u233 xx after\par}");
    assert_eq!(
        doc.blocks,
        vec![para(vec![s("\u{00E9}"), Inline::Space, s("after")])]
    );
    let doc = read(r"{\rtf1\ansi \uc0\u233 x\par}");
    assert_eq!(doc.blocks, vec![para(vec![s("\u{00E9}x")])]);
}

#[test]
fn negative_unicode_wraps() {
    let doc = read(r"{\rtf1\ansi \u-3647 ?after\par}");
    assert_eq!(doc.blocks, vec![para(vec![s("\u{F1C1}after")])]);
}

#[test]
fn outline_level_becomes_header() {
    let doc = read(r"{\rtf1\ansi \outlinelevel0 Chapter\par}");
    assert_eq!(
        doc.blocks,
        vec![Block::Header(1, Box::default(), vec![s("Chapter")])]
    );
    let doc = read(r"{\rtf1\ansi \outlinelevel2 Sub\par}");
    assert_eq!(
        doc.blocks,
        vec![Block::Header(3, Box::default(), vec![s("Sub")])]
    );
}

#[test]
fn extreme_outline_level_saturates_without_overflow() {
    let doc = read(r"{\rtf1\ansi \outlinelevel2147483647 Edge\par}");
    assert_eq!(
        doc.blocks,
        vec![Block::Header(i32::MAX, Box::default(), vec![s("Edge")])]
    );
}

#[test]
fn info_group_populates_metadata() {
    let doc = read(r"{\rtf1{\info{\title My Title}{\author Jane Doe}}\ansi Body\par}");
    assert_eq!(
        doc.meta.get("title"),
        Some(&MetaValue::MetaInlines(vec![
            s("My"),
            Inline::Space,
            s("Title")
        ]))
    );
    assert_eq!(
        doc.meta.get("author"),
        Some(&MetaValue::MetaInlines(vec![
            s("Jane"),
            Inline::Space,
            s("Doe")
        ]))
    );
    assert_eq!(doc.blocks, vec![para(vec![s("Body")])]);
}

#[test]
fn destinations_are_skipped() {
    let doc = read(r"{\rtf1{\fonttbl{\f0 Times;}}\ansi text {\*\generator X;}more\par}");
    assert_eq!(
        doc.blocks,
        vec![para(vec![s("text"), Inline::Space, s("more")])]
    );
}

#[test]
fn unknown_group_word_keeps_text() {
    let doc = read(r"{\rtf1\ansi text {\madeupword hidden} more\par}");
    assert_eq!(
        doc.blocks,
        vec![para(vec![
            s("text"),
            Inline::Space,
            s("hidden"),
            Inline::Space,
            s("more"),
        ])]
    );
}

#[test]
fn hyperlink_field_becomes_link() {
    let doc =
        read(r#"{\rtf1\ansi {\field{\*\fldinst HYPERLINK "http://x.com"}{\fldrslt click}}\par}"#);
    assert_eq!(
        doc.blocks,
        vec![para(vec![Inline::Link(
            Box::default(),
            vec![s("click")],
            Box::new(Target {
                url: "http://x.com".into(),
                title: Text::default(),
            }),
        )])]
    );
}

#[test]
fn hyperlink_field_without_quotes_becomes_link() {
    let doc = read(
        r#"{\rtf1\ansi {\field{\*\fldinst HYPERLINK http://x.com \o "tip"}{\fldrslt click}}\par}"#,
    );
    assert_eq!(
        doc.blocks,
        vec![para(vec![Inline::Link(
            Box::default(),
            vec![s("click")],
            Box::new(Target {
                url: "http://x.com".into(),
                title: Text::default(),
            }),
        )])]
    );
}

#[test]
fn list_table_numbering_becomes_ordered() {
    let doc = read(
        r"{\rtf1\ansi{\listtable{\list{\listlevel\levelnfc4\levelstartat3{\leveltext\'02\'00.;}}\listid1}}{\listoverridetable{\listoverride\listid1\ls1}}\pard\ls1\ilvl0 First\par\pard\ls1\ilvl0 Second\par}",
    );
    assert_eq!(
        doc.blocks,
        vec![Block::OrderedList(
            ListAttributes {
                start: 3,
                style: ListNumberStyle::LowerAlpha,
                delim: ListNumberDelim::Period,
            },
            vec![vec![para(vec![s("First")])], vec![para(vec![s("Second")])],],
        )]
    );
}

#[test]
fn list_without_a_table_stays_a_bullet() {
    let doc = read(
        r"{\rtf1\ansi {\listtext\'B7}\ls1\ilvl0 First\par {\listtext\'B7}\ls1\ilvl0 Second\par}",
    );
    assert_eq!(
        doc.blocks,
        vec![Block::BulletList(vec![
            vec![para(vec![s("First")])],
            vec![para(vec![s("Second")])],
        ])]
    );
}

#[test]
fn footnote_becomes_note() {
    let doc = read(r"{\rtf1\ansi text{\footnote note body}more\par}");
    assert_eq!(
        doc.blocks,
        vec![para(vec![
            s("text"),
            Inline::Note(vec![para(vec![s("note"), Inline::Space, s("body")])]),
            s("more"),
        ])]
    );
}

#[test]
fn bookmark_becomes_span() {
    let doc = read(r"{\rtf1\ansi {\*\bkmkstart mark}anchored{\*\bkmkend mark}\par}");
    assert_eq!(
        doc.blocks,
        vec![para(vec![Inline::Span(
            Box::new(Attr {
                id: "mark".into(),
                classes: Vec::new(),
                attributes: Vec::new(),
            }),
            vec![s("anchored")],
        )])]
    );
}

#[test]
fn table_is_reconstructed() {
    let doc = read(
        r"{\rtf1\ansi \trowd\cellx3000\cellx6000\pard\intbl A\cell\pard\intbl B\cell\row\pard after\par}",
    );
    let Some(Block::Table(table)) = doc.blocks.first() else {
        panic!("expected a leading table, got {:?}", doc.blocks);
    };
    assert_eq!(table.col_specs.len(), 2);
    assert_eq!(table.head.rows.len(), 0);
    let body = table.bodies.first().expect("body");
    assert_eq!(body.row_head_columns, 0);
    assert_eq!(body.body.len(), 1);
    let row = body.body.first().expect("row");
    assert_eq!(row.cells.len(), 2);
    assert_eq!(
        row.cells.first().expect("cell").content,
        vec![para(vec![s("A")])]
    );
    assert_eq!(
        row.cells.get(1).expect("cell").content,
        vec![para(vec![s("B")])]
    );
    assert_eq!(doc.blocks.get(1), Some(&para(vec![s("after")])));
}

#[test]
fn picture_decodes_into_media() {
    let (doc, media) = read_media(r"{\rtf1\ansi {\pict\pngblip 89504e47}\par}");
    let name = content_addressed_name("image/png", &[0x89, 0x50, 0x4e, 0x47]);
    assert_eq!(
        doc.blocks,
        vec![para(vec![Inline::Image(
            Box::default(),
            vec![s("image")],
            Box::new(Target {
                url: name.clone().into(),
                title: Text::default(),
            }),
        )])]
    );
    assert!(media.contains(&name));
}

#[test]
fn shape_picture_becomes_inline_image() {
    let (doc, media) = read_media(
        r"{\rtf1\ansi A{\shp{\*\shpinst{\sp{\sn pib}{\sv {\pict\pngblip 89504e470d0a1a0a}}}}}B\par}",
    );
    let name = content_addressed_name(
        "image/png",
        &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a],
    );
    assert_eq!(
        doc.blocks,
        vec![para(vec![
            s("A"),
            Inline::Image(
                Box::default(),
                vec![s("image")],
                Box::new(Target {
                    url: name.clone().into(),
                    title: Text::default(),
                }),
            ),
            s("B"),
        ])]
    );
    assert!(media.contains(&name));
}

#[test]
fn shape_text_box_becomes_paragraph() {
    let doc = read(
        r"{\rtf1\ansi Para one.\par {\shp{\*\shpinst{\shptxt \pard Callout text here.\par}}}Para two.\par}",
    );
    assert_eq!(
        doc.blocks,
        vec![
            para(vec![s("Para"), Inline::Space, s("one.")]),
            para(vec![
                s("Callout"),
                Inline::Space,
                s("text"),
                Inline::Space,
                s("here."),
            ]),
            para(vec![s("Para"), Inline::Space, s("two.")]),
        ]
    );
}

#[test]
fn raw_high_bytes_fall_back_to_latin1() {
    // Not valid UTF-8, so the whole document reads as Latin-1: 0xE9 -> U+00E9 (é).
    let doc = read_bytes(b"{\\rtf1\\ansi caf\xe9 here\\par}");
    assert_eq!(
        doc.blocks,
        vec![para(vec![s("caf\u{00E9}"), Inline::Space, s("here")])]
    );
    let doc = read_bytes(b"{\\rtf1\\ansi A\x93B\xa0C\\par}");
    assert_eq!(doc.blocks, vec![para(vec![s("A\u{0093}B\u{00A0}C")])]);
}

#[test]
fn empty_paragraphs_are_dropped() {
    let doc = read(r"{\rtf1\ansi text\par\par\par more\par}");
    assert_eq!(
        doc.blocks,
        vec![para(vec![s("text")]), para(vec![s("more")])]
    );
}

#[test]
fn trailing_content_without_par_flushes() {
    let doc = read(r"{\rtf1\ansi Hello}");
    assert_eq!(doc.blocks, vec![para(vec![s("Hello")])]);
}

#[test]
fn allcaps_uppercases_text() {
    let doc = read(r"{\rtf1\ansi \caps upper\par}");
    assert_eq!(doc.blocks, vec![para(vec![s("UPPER")])]);
}

#[test]
fn small_caps_wraps() {
    let doc = read(r"{\rtf1\ansi \scaps x\par}");
    assert_eq!(
        doc.blocks,
        vec![para(vec![Inline::SmallCaps(vec![s("x")])])]
    );
}

#[test]
fn down_level_numbering_placeholder_is_skipped() {
    // The down-level rendering placeholders drop out; the words on either side join directly.
    let doc = read(r"{\rtf1\ansi before{\pntxtb X}{\pntxta Y}after\par}");
    assert_eq!(doc.blocks, vec![para(vec![s("beforeafter")])]);

    let doc = read(r"{\rtf1\ansi {\pntext\pnlvlblt\pnf1{\pntxtb\'B7}}Item\par}");
    assert_eq!(doc.blocks, vec![para(vec![s("Item")])]);
}

#[test]
fn binary_data_is_consumed_by_byte_count() {
    // `\binN`'s N raw bytes are not text and must not desync the parse.
    let doc = read(r"{\rtf1\ansi price\bin3ABCtag\par}");
    assert_eq!(doc.blocks, vec![para(vec![s("pricetag")])]);
}

#[test]
fn binary_picture_decodes_into_media() {
    // The payload includes 0x7d (`}`): captured as data at the lexer, it must not end the
    // picture group early.
    let (doc, media) = read_media_bytes(
        b"{\\rtf1\\ansi before {\\pict\\pngblip\\bin4 \x89\x7d\x50\x47}after\\par}",
    );
    let name = content_addressed_name("image/png", &[0x89, 0x7d, 0x50, 0x47]);
    assert_eq!(
        doc.blocks,
        vec![para(vec![
            s("before"),
            Inline::Space,
            Inline::Image(
                Box::default(),
                vec![s("image")],
                Box::new(Target {
                    url: name.clone().into(),
                    title: Text::default(),
                }),
            ),
            s("after"),
        ])]
    );
    assert!(media.contains(&name));
}

#[test]
fn hyperlink_display_keeps_edge_space() {
    // Edge spaces in display text are preserved so the link does not fuse with its neighbor.
    let doc = read(
        r#"{\rtf1\ansi {\field{\*\fldinst{HYPERLINK "http://x.com"}}{\fldrslt link }}after\par}"#,
    );
    assert_eq!(
        doc.blocks,
        vec![para(vec![
            Inline::Link(
                Box::default(),
                vec![s("link"), Inline::Space],
                Box::new(Target {
                    url: "http://x.com".into(),
                    title: Text::default(),
                }),
            ),
            s("after"),
        ])]
    );

    let doc = read(
        r#"{\rtf1\ansi {\field{\*\fldinst{HYPERLINK "http://y.com"}}{\fldrslt  lead}}tail\par}"#,
    );
    assert_eq!(
        doc.blocks,
        vec![para(vec![
            Inline::Link(
                Box::default(),
                vec![Inline::Space, s("lead")],
                Box::new(Target {
                    url: "http://y.com".into(),
                    title: Text::default(),
                }),
            ),
            s("tail"),
        ])]
    );
}

#[test]
fn empty_input_is_empty_document() {
    let doc = read("");
    assert!(doc.blocks.is_empty());
    assert!(doc.meta.is_empty());
}
