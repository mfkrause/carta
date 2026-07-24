//! Automatic-style fragments and the named stylesheet for the ODT writer.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use carta_ast::{ListNumberDelim, ListNumberStyle, MetaValue, Text};
use carta_core::container::xml::escape_attribute;

use super::meta::meta_text;
use super::{AlignKind, DECL, Deco, NS, language_country};

pub(super) fn deco_style_xml(index: usize, key: &[Deco]) -> String {
    // Super- and subscript share one text-position property; a run carrying both keeps superscript.
    let drops_subscript = key.contains(&Deco::Superscript) && key.contains(&Deco::Subscript);
    let mut properties = String::new();
    for deco in key {
        if drops_subscript && *deco == Deco::Subscript {
            continue;
        }
        if !properties.is_empty() {
            properties.push(' ');
        }
        properties.push_str(deco.properties());
    }
    format!(
        "<style:style style:name=\"T{index}\" style:family=\"text\">\
         <style:text-properties {properties} /></style:style>"
    )
}

pub(super) fn para_style_xml(index: usize, header: bool, kind: AlignKind) -> String {
    let parent = if header {
        "Table_20_Heading"
    } else {
        "Table_20_Contents"
    };
    let align = match kind {
        AlignKind::Center => "center",
        AlignKind::Right => "end",
    };
    format!(
        "<style:style style:name=\"P{index}\" style:family=\"paragraph\" \
         style:parent-style-name=\"{parent}\">\
         <style:paragraph-properties fo:text-align=\"{align}\" \
         style:justify-single-word=\"false\" /></style:style>"
    )
}

pub(super) fn num_format(style: ListNumberStyle) -> &'static str {
    match style {
        ListNumberStyle::LowerRoman => "i",
        ListNumberStyle::UpperRoman => "I",
        ListNumberStyle::LowerAlpha => "a",
        ListNumberStyle::UpperAlpha => "A",
        _ => "1",
    }
}

pub(super) fn delim_fixes(delim: ListNumberDelim) -> (Option<&'static str>, &'static str) {
    match delim {
        ListNumberDelim::OneParen => (None, ")"),
        ListNumberDelim::TwoParens => (Some("("), ")"),
        _ => (None, "."),
    }
}

/// Builds the `styles.xml` part: the named paragraph, text, and list styles the body references,
/// plus the page layout and its master page.
pub(super) fn styles_xml(meta: &BTreeMap<Text, MetaValue>) -> String {
    let lang = meta_text(meta, "lang");
    let (language, country) = if lang.is_empty() {
        ("en".to_string(), "US".to_string())
    } else {
        language_country(&lang)
    };

    let mut out = String::with_capacity(16 * 1024);
    out.push_str(DECL);
    out.push_str("<office:document-styles");
    out.push_str(NS);
    out.push_str(" office:version=\"1.3\">");

    out.push_str("<office:font-face-decls>");
    out.push_str(
        "<style:font-face style:name=\"Courier New\" style:font-family-generic=\"modern\" \
         style:font-pitch=\"fixed\" svg:font-family=\"'Courier New'\" />\
         <style:font-face style:name=\"Times New Roman\" style:font-family-generic=\"roman\" \
         style:font-pitch=\"variable\" svg:font-family=\"'Times New Roman'\" />\
         <style:font-face style:name=\"Arial\" style:font-family-generic=\"swiss\" \
         style:font-pitch=\"variable\" svg:font-family=\"Arial\" />",
    );
    out.push_str("</office:font-face-decls>");

    out.push_str("<office:styles>");
    push_named_styles(&mut out, &language, &country);
    out.push_str("</office:styles>");

    out.push_str(
        "<office:automatic-styles>\
         <style:page-layout style:name=\"Mpm1\">\
         <style:page-layout-properties fo:page-width=\"8.5in\" fo:page-height=\"11in\" \
         fo:margin-top=\"1in\" fo:margin-bottom=\"1in\" fo:margin-left=\"1in\" \
         fo:margin-right=\"1in\" style:print-orientation=\"portrait\" />\
         </style:page-layout>\
         </office:automatic-styles>",
    );

    out.push_str(
        "<office:master-styles>\
         <style:master-page style:name=\"Standard\" style:page-layout-name=\"Mpm1\" />\
         </office:master-styles>",
    );

    out.push_str("</office:document-styles>");
    out
}

/// Emits every named style the writer references, in the order the schema expects them. The default
/// paragraph style records the document language, which the surrounding builder derives from the
/// metadata.
#[allow(clippy::too_many_lines)]
fn push_named_styles(out: &mut String, language: &str, country: &str) {
    let mut language_attr = String::new();
    escape_attribute(language, &mut language_attr);
    let mut country_attr = String::new();
    escape_attribute(country, &mut country_attr);
    let _ = write!(
        out,
        "<style:default-style style:family=\"paragraph\">\
         <style:paragraph-properties fo:hyphenation-ladder-count=\"no-limit\" \
         style:line-break=\"strict\" style:tab-stop-distance=\"0.5in\" />\
         <style:text-properties style:font-name=\"Times New Roman\" fo:font-size=\"12pt\" \
         fo:language=\"{language_attr}\" fo:country=\"{country_attr}\" /></style:default-style>"
    );

    push_paragraph_style(
        out,
        "Standard",
        None,
        "<style:text-properties style:font-name=\"Times New Roman\" fo:font-size=\"12pt\" />",
    );
    push_paragraph_style(
        out,
        "Text_20_body",
        Some("Standard"),
        "<style:paragraph-properties fo:margin-top=\"0in\" fo:margin-bottom=\"0.0835in\" \
         fo:line-height=\"115%\" />",
    );
    push_paragraph_style(out, "First_20_paragraph", Some("Text_20_body"), "");
    push_paragraph_style(
        out,
        "Heading",
        Some("Standard"),
        "<style:paragraph-properties fo:margin-top=\"0.1665in\" fo:margin-bottom=\"0.0835in\" \
         fo:keep-with-next=\"always\" />\
         <style:text-properties style:font-name=\"Arial\" fo:font-size=\"14pt\" />",
    );
    for level in 1..=6 {
        let size = match level {
            1 => "18pt",
            2 => "16pt",
            3 => "14pt",
            4 => "12pt",
            5 => "11pt",
            _ => "10pt",
        };
        let _ = write!(
            out,
            "<style:style style:name=\"Heading_20_{level}\" style:family=\"paragraph\" \
             style:parent-style-name=\"Heading\" style:default-outline-level=\"{level}\">\
             <style:text-properties fo:font-size=\"{size}\" fo:font-weight=\"bold\" /></style:style>"
        );
    }
    push_paragraph_style(
        out,
        "Title",
        Some("Standard"),
        "<style:paragraph-properties fo:text-align=\"center\" />\
         <style:text-properties fo:font-size=\"28pt\" fo:font-weight=\"bold\" />",
    );
    push_paragraph_style(
        out,
        "Subtitle",
        Some("Standard"),
        "<style:paragraph-properties fo:text-align=\"center\" />\
         <style:text-properties fo:font-size=\"18pt\" />",
    );
    push_paragraph_style(
        out,
        "Author",
        Some("Standard"),
        "<style:paragraph-properties fo:text-align=\"center\" />",
    );
    push_paragraph_style(
        out,
        "Date",
        Some("Standard"),
        "<style:paragraph-properties fo:text-align=\"center\" />",
    );
    push_paragraph_style(
        out,
        "Quotations",
        Some("Standard"),
        "<style:paragraph-properties fo:margin-left=\"0.4in\" fo:margin-right=\"0.4in\" \
         fo:margin-top=\"0in\" fo:margin-bottom=\"0.0835in\" />",
    );
    push_paragraph_style(
        out,
        "Preformatted_20_Text",
        Some("Standard"),
        "<style:paragraph-properties fo:margin-top=\"0in\" fo:margin-bottom=\"0in\" />\
         <style:text-properties style:font-name=\"Courier New\" fo:font-size=\"10pt\" />",
    );
    push_paragraph_style(
        out,
        "Horizontal_20_Line",
        Some("Standard"),
        "<style:paragraph-properties fo:margin-top=\"0in\" fo:margin-bottom=\"0.0398in\" \
         style:border-line-width-bottom=\"0.0008in 0.0016in 0.0008in\" \
         fo:padding=\"0in\" fo:border-bottom=\"0.06pt double #808080\" />",
    );
    push_paragraph_style(
        out,
        "Footnote",
        Some("Standard"),
        "<style:paragraph-properties fo:margin-left=\"0.2in\" fo:text-indent=\"-0.2in\" />\
         <style:text-properties fo:font-size=\"10pt\" />",
    );
    push_paragraph_style(out, "List", Some("Text_20_body"), "");
    for tight in [false, true] {
        let suffix = if tight { "_20_Tight" } else { "" };
        push_paragraph_style(out, &format!("List_20_Bullet{suffix}"), Some("List"), "");
        push_paragraph_style(out, &format!("List_20_Number{suffix}"), Some("List"), "");
        push_paragraph_style(
            out,
            &format!("Definition_20_Term{suffix}"),
            Some("Standard"),
            "<style:text-properties fo:font-weight=\"bold\" />",
        );
        push_paragraph_style(
            out,
            &format!("Definition_20_Definition{suffix}"),
            Some("Standard"),
            "<style:paragraph-properties fo:margin-left=\"0.4in\" />",
        );
    }
    push_paragraph_style(
        out,
        "Table_20_Heading",
        Some("Standard"),
        "<style:paragraph-properties fo:text-align=\"center\" />\
         <style:text-properties fo:font-weight=\"bold\" />",
    );
    push_paragraph_style(out, "Table_20_Contents", Some("Standard"), "");
    push_paragraph_style(
        out,
        "TableCaption",
        Some("Standard"),
        "<style:paragraph-properties fo:text-align=\"center\" fo:margin-top=\"0.0835in\" \
         fo:margin-bottom=\"0.0835in\" />\
         <style:text-properties fo:font-style=\"italic\" fo:font-size=\"10pt\" />",
    );
    push_paragraph_style(
        out,
        "FigureWithCaption",
        Some("Standard"),
        "<style:paragraph-properties fo:text-align=\"center\" />",
    );
    push_paragraph_style(
        out,
        "FigureCaption",
        Some("Standard"),
        "<style:paragraph-properties fo:text-align=\"center\" />\
         <style:text-properties fo:font-style=\"italic\" fo:font-size=\"10pt\" />",
    );
    push_paragraph_style(
        out,
        "Contents_20_Heading",
        Some("Heading"),
        "<style:paragraph-properties fo:keep-with-next=\"always\" />\
         <style:text-properties fo:font-size=\"16pt\" fo:font-weight=\"bold\" />",
    );
    for level in 1..=10 {
        let indent = format!("{:.4}in", f64::from(level - 1) * 0.2);
        let _ = write!(
            out,
            "<style:style style:name=\"Contents_20_{level}\" style:family=\"paragraph\" \
             style:parent-style-name=\"Standard\">\
             <style:paragraph-properties fo:margin-left=\"{indent}\" fo:margin-right=\"0in\" \
             fo:text-indent=\"0in\"><style:tab-stops>\
             <style:tab-stop style:position=\"6.5in\" style:type=\"right\" \
             style:leader-style=\"dotted\" style:leader-text=\".\" /></style:tab-stops>\
             </style:paragraph-properties></style:style>"
        );
    }

    // Character styles.
    push_text_style(
        out,
        "Emphasis",
        "<style:text-properties fo:font-style=\"italic\" />",
    );
    push_text_style(
        out,
        "Strong_20_Emphasis",
        "<style:text-properties fo:font-weight=\"bold\" />",
    );
    push_text_style(
        out,
        "Strikeout",
        "<style:text-properties style:text-line-through-style=\"solid\" \
         style:text-line-through-type=\"single\" />",
    );
    push_text_style(
        out,
        "Superscript",
        "<style:text-properties style:text-position=\"super 58%\" />",
    );
    push_text_style(
        out,
        "Subscript",
        "<style:text-properties style:text-position=\"sub 58%\" />",
    );
    push_text_style(
        out,
        "Source_20_Text",
        "<style:text-properties style:font-name=\"Courier New\" />",
    );
    push_text_style(out, "Definition", "");
    push_text_style(
        out,
        "Internet_20_link",
        "<style:text-properties fo:color=\"#000080\" style:text-underline-color=\"font-color\" \
         style:text-underline-style=\"solid\" style:text-underline-width=\"auto\" />",
    );
    push_text_style(out, "Numbering_20_Symbols", "");
    push_text_style(out, "Bullet_20_Symbols", "");

    // Named list styles.
    out.push_str("<text:list-style style:name=\"List_20_1\">");
    for level in 1..=10 {
        let space = format!("{:.4}in", f64::from(level) * 0.25);
        let _ = write!(
            out,
            "<text:list-level-style-bullet text:level=\"{level}\" \
             text:style-name=\"Bullet_20_Symbols\" text:bullet-char=\"\u{2022}\">\
             <style:list-level-properties text:space-before=\"{space}\" \
             text:min-label-width=\"0.25in\" /></text:list-level-style-bullet>"
        );
    }
    out.push_str("</text:list-style>");

    out.push_str("<text:list-style style:name=\"Numbering_20_1\">");
    for level in 1..=10 {
        let space = format!("{:.4}in", f64::from(level) * 0.1972);
        let _ = write!(
            out,
            "<text:list-level-style-number text:level=\"{level}\" \
             text:style-name=\"Numbering_20_Symbols\" style:num-format=\"1\" style:num-suffix=\".\">\
             <style:list-level-properties text:space-before=\"{space}\" \
             text:min-label-width=\"0.1965in\" /></text:list-level-style-number>"
        );
    }
    out.push_str("</text:list-style>");
}

fn push_paragraph_style(out: &mut String, name: &str, parent: Option<&str>, inner: &str) {
    out.push_str("<style:style style:name=\"");
    out.push_str(name);
    out.push_str("\" style:family=\"paragraph\"");
    if let Some(parent) = parent {
        out.push_str(" style:parent-style-name=\"");
        out.push_str(parent);
        out.push('"');
    }
    out.push('>');
    out.push_str(inner);
    out.push_str("</style:style>");
}

fn push_text_style(out: &mut String, name: &str, inner: &str) {
    out.push_str("<style:style style:name=\"");
    out.push_str(name);
    out.push_str("\" style:family=\"text\">");
    out.push_str(inner);
    out.push_str("</style:style>");
}
