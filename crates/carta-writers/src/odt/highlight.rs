//! Styled source tokens and their ODT character styles.

use std::fmt::Write as _;

use carta_ast::Attr;
use carta_core::container::xml::Element;
use carta_highlight::{Theme, TokenKind};

use super::Builder;

impl Builder<'_> {
    pub(super) fn highlighted_code(&mut self, attr: &Attr, code: &str) -> bool {
        if self.options.highlight.theme.is_none() {
            return false;
        }
        let Some(highlighter) = &self.options.highlight.highlighter else {
            return false;
        };
        let Some(language) = attr
            .classes
            .iter()
            .find(|class| highlighter.registry().is_known(class.as_str()))
        else {
            return false;
        };
        let Some(lines) = highlighter.highlight(language.as_str(), code) else {
            return false;
        };
        for line in lines {
            let has_tokens = !line.is_empty();
            self.body
                .push_str("<text:p text:style-name=\"Preformatted_20_Text\">\n");
            for token in line {
                let _ = write!(
                    self.body,
                    "<text:span text:style-name=\"{}Tok\">",
                    token.kind.style_key()
                );
                self.push_verbatim(&token.text);
                self.body.push_str("</text:span>");
            }
            if has_tokens {
                self.body.push('\n');
            }
            self.body.push_str("</text:p>");
        }
        true
    }
}

pub(super) fn styles(theme: &Theme) -> String {
    let mut output = String::new();
    for kind in TOKEN_ORDER {
        let style = theme.style_for(kind);
        let mut properties = Element::new("style:text-properties");
        if let Some(color) = style
            .and_then(|style| style.text_color.as_deref())
            .or(theme.text_color.as_deref())
        {
            properties = properties.attr("fo:color", color);
        }
        if let Some(color) = style.and_then(|style| style.background_color.as_deref()) {
            properties = properties.attr("fo:background-color", color);
        }
        if let Some(style) = style {
            if style.italic {
                properties = properties.attr("fo:font-style", "italic");
            }
            if style.bold {
                properties = properties.attr("fo:font-weight", "bold");
            }
            if style.underline {
                properties = properties.attr("style:text-underline-style", "solid");
            }
        }
        let _ = write!(
            output,
            "<style:style style:name=\"{}Tok\" style:family=\"text\">{}</style:style>",
            kind.style_key(),
            properties.render()
        );
    }
    output
}

const TOKEN_ORDER: [TokenKind; 31] = [
    TokenKind::Keyword,
    TokenKind::DataType,
    TokenKind::DecVal,
    TokenKind::BaseN,
    TokenKind::Float,
    TokenKind::Constant,
    TokenKind::Char,
    TokenKind::SpecialChar,
    TokenKind::String,
    TokenKind::VerbatimString,
    TokenKind::SpecialString,
    TokenKind::Import,
    TokenKind::Comment,
    TokenKind::Documentation,
    TokenKind::Annotation,
    TokenKind::CommentVar,
    TokenKind::Other,
    TokenKind::Function,
    TokenKind::Variable,
    TokenKind::ControlFlow,
    TokenKind::Operator,
    TokenKind::BuiltIn,
    TokenKind::Extension,
    TokenKind::Preprocessor,
    TokenKind::Attribute,
    TokenKind::RegionMarker,
    TokenKind::Information,
    TokenKind::Warning,
    TokenKind::Alert,
    TokenKind::Error,
    TokenKind::Normal,
];

#[cfg(test)]
mod tests {
    use super::*;
    use carta_core::WriterOptions;
    use carta_highlight::{Highlighter, TokenStyle};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    #[test]
    #[allow(clippy::arc_with_non_send_sync)]
    fn code_tokens_keep_spaces_quotes_and_comments() {
        let mut options = WriterOptions::default();
        let mut highlighter = Highlighter::new();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../carta-highlight/data/syntax-copyleft/python.xml");
        let grammar = std::fs::read_to_string(path).expect("Python grammar");
        highlighter
            .registry_mut()
            .add_definition_with_stem(&grammar, "python")
            .expect("valid Python grammar");
        options.highlight.highlighter = Some(Arc::new(highlighter));
        options.highlight.theme = Some(carta_highlight::builtin_style("tango").unwrap().unwrap());
        let mut builder = Builder::new(&options);
        let attr = Attr {
            classes: vec!["python".into()],
            ..Default::default()
        };
        assert!(builder.highlighted_code(&attr, "x = 1\nprint(\"hello\") # note"));

        assert!(builder.body.contains(
            "<text:span text:style-name=\"NormalTok\"><text:s text:c=\"1\" /></text:span>"
        ));
        assert!(
            builder
                .body
                .contains("<text:span text:style-name=\"StringTok\">&quot;hello&quot;</text:span>")
        );
        assert!(
            builder
                .body
                .contains("<text:span text:style-name=\"CommentTok\"># note</text:span>")
        );
        builder.body.clear();
        assert!(builder.highlighted_code(&attr, "\n"));
        assert_eq!(
            builder.body,
            "<text:p text:style-name=\"Preformatted_20_Text\">\n</text:p>"
        );
    }

    #[test]
    fn token_styles_keep_overrides_and_inherit_the_default_color() {
        let theme = Theme {
            text_color: Some("#abcdef".into()),
            background_color: None,
            line_number_color: None,
            line_number_background_color: None,
            text_styles: BTreeMap::from([(
                "Keyword".into(),
                TokenStyle {
                    text_color: Some("#112233".into()),
                    background_color: Some("#445566".into()),
                    bold: true,
                    italic: true,
                    underline: true,
                },
            )]),
        };
        let xml = styles(&theme);
        assert!(xml.contains("<style:text-properties fo:color=\"#112233\" fo:background-color=\"#445566\" fo:font-style=\"italic\" fo:font-weight=\"bold\" style:text-underline-style=\"solid\" />"));
        assert!(xml.contains("<style:style style:name=\"NormalTok\" style:family=\"text\"><style:text-properties fo:color=\"#abcdef\" /></style:style>"));
    }
}
