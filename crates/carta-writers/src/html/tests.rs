//! Unit tests for the html writer helpers.

#[cfg(test)]
mod escaping_tests {
    use super::super::{escape_attr, escape_text_into};

    #[test]
    fn attribute_values_and_code_block_bodies_entity_encode_both_quotes() {
        assert_eq!(escape_attr("a\"b'c<&>"), "a&quot;b&#39;c&lt;&amp;&gt;");
    }

    #[test]
    fn running_text_and_inline_code_keep_both_quotes_literal() {
        let mut out = String::new();
        escape_text_into(&mut out, "a\"b'c<&>");
        assert_eq!(out, "a\"b'c&lt;&amp;&gt;");
    }

    #[test]
    fn clean_text_is_copied_verbatim() {
        let mut out = String::new();
        escape_text_into(&mut out, "plain caf\u{e9} text");
        assert_eq!(out, "plain caf\u{e9} text");
    }

    #[test]
    fn triggers_at_the_edges_and_back_to_back_are_escaped() {
        assert_eq!(escape_attr("&x"), "&amp;x");
        assert_eq!(escape_attr("x<"), "x&lt;");
        assert_eq!(escape_attr("<<>>"), "&lt;&lt;&gt;&gt;");
        assert_eq!(escape_attr("caf\u{e9}<\u{e9}>"), "caf\u{e9}&lt;\u{e9}&gt;");
    }

    #[test]
    fn assembly_sentinels_in_content_are_protected() {
        let mut out = String::new();
        escape_text_into(&mut out, "a\u{1}b");
        assert_eq!(out, "a\u{1}\u{1}b");
    }
}

#[cfg(test)]
mod restore_tests {
    use super::super::{BREAK, ESCAPE, FLUSH, SOFT, restore};

    #[test]
    fn text_without_a_sentinel_passes_through() {
        assert_eq!(restore("plain text".to_owned()), "plain text");
    }

    #[test]
    fn escape_sequences_decode_to_their_sentinels() {
        let mut input = String::from("a");
        input.push(ESCAPE);
        input.push('0'); // BREAK_TAG
        input.push(ESCAPE);
        input.push('2'); // SOFT_TAG
        input.push(ESCAPE);
        input.push('3'); // FLUSH_TAG
        input.push(ESCAPE);
        input.push(ESCAPE);
        input.push('b');

        let mut expected = String::from("a");
        expected.push(BREAK);
        expected.push(SOFT);
        expected.push(FLUSH);
        expected.push(ESCAPE);
        expected.push('b');

        assert_eq!(restore(input), expected);
    }
}

#[cfg(test)]
mod char_width_tests {
    use super::super::{char_width, is_zero_width};

    #[test]
    fn low_range_fast_path_matches_category_lookup() {
        for code in 0u32..0x0300 {
            let Some(ch) = char::from_u32(code) else {
                continue;
            };
            let expected = usize::from(!is_zero_width(ch));
            assert_eq!(char_width(ch), expected, "width mismatch at U+{code:04X}");
        }
    }

    #[test]
    fn pins_representative_widths() {
        assert_eq!(char_width('a'), 1);
        assert_eq!(char_width('\u{200B}'), 0); // zero-width space (Format)
        assert_eq!(char_width('\u{0301}'), 0); // combining acute accent (Nonspacing_Mark)
        assert_eq!(char_width('\u{7}'), 0); // bell (Control)
        assert_eq!(char_width('\u{4E00}'), 2); // CJK ideograph (wide)
        assert_eq!(char_width('\u{1F600}'), 2); // grinning face emoji (wide)
    }
}

#[cfg(test)]
mod image_tests {
    use super::super::HtmlWriter;
    use carta_ast::{Block, Document, Inline, Target, Text};
    use carta_core::{WrapMode, Writer, WriterOptions};

    fn image_block(url: &str, alt: &str) -> Block {
        let alt_inlines = if alt.is_empty() {
            Vec::new()
        } else {
            vec![Inline::Str(alt.into())]
        };
        Block::Para(vec![Inline::Image(
            Box::default(),
            alt_inlines,
            Box::new(Target {
                url: url.into(),
                title: Text::default(),
            }),
        )])
    }

    fn render(url: &str, alt: &str) -> String {
        let document = Document {
            blocks: vec![image_block(url, alt)],
            ..Document::default()
        };
        let mut options = WriterOptions::default();
        options.wrap = WrapMode::None;
        HtmlWriter.write(&document, &options).expect("render html")
    }

    #[test]
    fn image_carries_its_alt_text() {
        assert_eq!(
            render("pic.png", "alt text"),
            "<p><img src=\"pic.png\" alt=\"alt text\" /></p>"
        );
    }

    #[test]
    fn image_without_alt_omits_the_attribute() {
        assert_eq!(render("pic.png", ""), "<p><img src=\"pic.png\" /></p>");
    }
}

#[cfg(all(test, feature = "epub"))]
mod xml_sanitize_tests {
    use super::super::{is_xml_char, strip_xml_invalid};

    #[test]
    fn strips_forbidden_c0_controls_and_keeps_whitespace() {
        // tab, newline and carriage return are the only permitted controls
        let input = String::from("a\u{0}b\u{1}\u{7}c\u{1f}\td\r\ne");
        assert_eq!(strip_xml_invalid(input), "abc\td\r\ne");
    }

    #[test]
    fn returns_clean_text_unchanged() {
        let input = String::from("plain text with unicode \u{2603} and a sum \u{2211}");
        assert_eq!(strip_xml_invalid(input.clone()), input);
    }

    #[test]
    fn classifies_boundary_code_points() {
        for forbidden in [
            '\u{0}', '\u{8}', '\u{b}', '\u{c}', '\u{1f}', '\u{fffe}', '\u{ffff}',
        ] {
            assert!(!is_xml_char(forbidden), "{forbidden:?} must be rejected");
        }
        for allowed in ['\t', '\n', '\r', ' ', 'a', '\u{fffd}', '\u{10000}'] {
            assert!(is_xml_char(allowed), "{allowed:?} must be accepted");
        }
    }

    #[test]
    fn deeply_nested_blocks_serialize_without_stack_overflow() {
        use super::super::{Flavor, State};
        use carta_ast::{Block, Inline};
        use std::thread;

        // recursive `Drop` of the chain would overflow the small stack independently of what is under test
        fn dismantle(mut block: Block) {
            while let Block::BlockQuote(mut children) = block {
                match children.pop() {
                    Some(child) => block = child,
                    None => break,
                }
            }
        }

        // modest stack: without the serializer's on-demand stack growth this depth faults
        let rendered = thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(|| {
                let mut block = Block::Para(vec![Inline::Str("deep".into())]);
                for _ in 0..20_000 {
                    block = Block::BlockQuote(vec![block]);
                }
                let mut state = State {
                    flavor: Flavor::Epub3,
                    ..State::default()
                };
                let mut out = String::new();
                state.blocks(&mut out, std::slice::from_ref(&block));
                let opened = out.starts_with("<blockquote>");
                dismantle(block);
                opened
            })
            .expect("spawn render thread")
            .join()
            .expect("serializing deeply nested blocks must not overflow the stack");
        assert!(rendered, "the nested blockquotes must render");
    }
}
