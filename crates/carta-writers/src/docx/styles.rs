//! carta's own styling parts: the style catalogue, document settings, web settings, font table and
//! theme. They are the visual design a generated document ships with. A reference document may
//! replace any of them (see the writer's part substitution), so the generated body adopts that
//! document's look while the content stays carta's.
//!
//! Each is a complete, well-formed part held as a string constant.

/// The style catalogue with a per-token character style appended for each token style the theme
/// defines. Each derives from the plain code character style and carries the theme's weight, slant
/// and color for that token kind, so a colorized code run resolves to the intended appearance. The
/// base catalogue already defines the source-code paragraph and plain code character styles the runs
/// build on.
#[cfg(feature = "highlight")]
pub(super) fn styles_with_highlighting(theme: &carta_highlight::Theme) -> String {
    use carta_core::container::xml::escape_attribute;
    use std::fmt::Write as _;

    let mut injected = String::new();
    for (key, style) in &theme.text_styles {
        let mut name = String::new();
        escape_attribute(key, &mut name);
        name.push_str("Tok");

        let mut rpr = String::new();
        if style.bold {
            rpr.push_str("<w:b/>");
        }
        if style.italic {
            rpr.push_str("<w:i/>");
        }
        if style.underline {
            rpr.push_str("<w:u w:val=\"single\"/>");
        }
        if let Some(color) = &style.text_color {
            let mut value = String::new();
            escape_attribute(color.strip_prefix('#').unwrap_or(color), &mut value);
            let _ = write!(rpr, "<w:color w:val=\"{value}\"/>");
        }

        let _ = write!(
            injected,
            "  <w:style w:type=\"character\" w:styleId=\"{name}\">\n    \
             <w:name w:val=\"{name}\"/>\n    \
             <w:basedOn w:val=\"VerbatimChar\"/>\n    \
             <w:rPr>{rpr}</w:rPr>\n  </w:style>\n"
        );
    }
    STYLES.replace("</w:styles>", &format!("{injected}</w:styles>"))
}

/// The style catalogue: the paragraph and character styles the document body refers to.
pub(super) const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:docDefaults>
    <w:rPrDefault>
      <w:rPr>
        <w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:cs="Calibri"/>
        <w:sz w:val="22"/>
      </w:rPr>
    </w:rPrDefault>
    <w:pPrDefault>
      <w:pPr>
        <w:spacing w:after="180" w:line="259" w:lineRule="auto"/>
      </w:pPr>
    </w:pPrDefault>
  </w:docDefaults>
  <w:style w:type="paragraph" w:default="1" w:styleId="Normal">
    <w:name w:val="Normal"/>
  </w:style>
  <w:style w:type="paragraph" w:styleId="BodyText">
    <w:name w:val="Body Text"/>
    <w:basedOn w:val="Normal"/>
  </w:style>
  <w:style w:type="paragraph" w:styleId="FirstParagraph">
    <w:name w:val="First Paragraph"/>
    <w:basedOn w:val="BodyText"/>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Compact">
    <w:name w:val="Compact"/>
    <w:basedOn w:val="BodyText"/>
    <w:pPr><w:spacing w:after="0"/></w:pPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="BlockText">
    <w:name w:val="Block Text"/>
    <w:basedOn w:val="BodyText"/>
    <w:pPr><w:ind w:left="720"/></w:pPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="SourceCode">
    <w:name w:val="Source Code"/>
    <w:basedOn w:val="Normal"/>
    <w:pPr><w:spacing w:after="0"/></w:pPr>
    <w:rPr><w:rFonts w:ascii="Consolas" w:hAnsi="Consolas" w:cs="Consolas"/></w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Heading1">
    <w:name w:val="heading 1"/><w:basedOn w:val="Normal"/>
    <w:pPr><w:keepNext/><w:outlineLvl w:val="0"/></w:pPr>
    <w:rPr><w:b/><w:sz w:val="36"/></w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Heading2">
    <w:name w:val="heading 2"/><w:basedOn w:val="Normal"/>
    <w:pPr><w:keepNext/><w:outlineLvl w:val="1"/></w:pPr>
    <w:rPr><w:b/><w:sz w:val="32"/></w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Heading3">
    <w:name w:val="heading 3"/><w:basedOn w:val="Normal"/>
    <w:pPr><w:keepNext/><w:outlineLvl w:val="2"/></w:pPr>
    <w:rPr><w:b/><w:sz w:val="28"/></w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Heading4">
    <w:name w:val="heading 4"/><w:basedOn w:val="Normal"/>
    <w:pPr><w:keepNext/><w:outlineLvl w:val="3"/></w:pPr>
    <w:rPr><w:b/><w:sz w:val="26"/></w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Heading5">
    <w:name w:val="heading 5"/><w:basedOn w:val="Normal"/>
    <w:pPr><w:keepNext/><w:outlineLvl w:val="4"/></w:pPr>
    <w:rPr><w:b/><w:sz w:val="24"/></w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Heading6">
    <w:name w:val="heading 6"/><w:basedOn w:val="Normal"/>
    <w:pPr><w:keepNext/><w:outlineLvl w:val="5"/></w:pPr>
    <w:rPr><w:b/><w:sz w:val="22"/></w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Heading7">
    <w:name w:val="heading 7"/><w:basedOn w:val="Normal"/>
    <w:pPr><w:keepNext/><w:outlineLvl w:val="6"/></w:pPr>
    <w:rPr><w:b/><w:sz w:val="22"/></w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Heading8">
    <w:name w:val="heading 8"/><w:basedOn w:val="Normal"/>
    <w:pPr><w:keepNext/><w:outlineLvl w:val="7"/></w:pPr>
    <w:rPr><w:b/><w:sz w:val="22"/></w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Heading9">
    <w:name w:val="heading 9"/><w:basedOn w:val="Normal"/>
    <w:pPr><w:keepNext/><w:outlineLvl w:val="8"/></w:pPr>
    <w:rPr><w:b/><w:sz w:val="22"/></w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="FootnoteText">
    <w:name w:val="footnote text"/>
    <w:basedOn w:val="Normal"/>
    <w:pPr><w:spacing w:after="0"/></w:pPr>
    <w:rPr><w:sz w:val="20"/></w:rPr>
  </w:style>
  <w:style w:type="character" w:styleId="VerbatimChar">
    <w:name w:val="Verbatim Char"/>
    <w:rPr><w:rFonts w:ascii="Consolas" w:hAnsi="Consolas" w:cs="Consolas"/></w:rPr>
  </w:style>
  <w:style w:type="character" w:styleId="Hyperlink">
    <w:name w:val="Hyperlink"/>
    <w:rPr><w:color w:val="0563C1"/><w:u w:val="single"/></w:rPr>
  </w:style>
  <w:style w:type="character" w:styleId="FootnoteReference">
    <w:name w:val="footnote reference"/>
    <w:rPr><w:vertAlign w:val="superscript"/></w:rPr>
  </w:style>
</w:styles>
"#;

/// Document-wide settings, including the reserved footnote separators.
pub(super) const SETTINGS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:defaultTabStop w:val="720"/>
  <w:footnotePr>
    <w:footnote w:id="-1"/>
    <w:footnote w:id="0"/>
  </w:footnotePr>
</w:settings>
"#;

/// Web-view settings; carta needs none beyond the empty part.
pub(super) const WEB_SETTINGS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:webSettings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"/>
"#;

/// The font table declaring the fonts the styles refer to.
pub(super) const FONT_TABLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:fonts xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:font w:name="Calibri"><w:family w:val="swiss"/><w:pitch w:val="variable"/></w:font>
  <w:font w:name="Consolas"><w:family w:val="modern"/><w:pitch w:val="fixed"/></w:font>
</w:fonts>
"#;

/// The document theme: color, font and format schemes.
pub(super) const THEME: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="carta">
  <a:themeElements>
    <a:clrScheme name="carta">
      <a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1>
      <a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>
      <a:dk2><a:srgbClr val="44546A"/></a:dk2>
      <a:lt2><a:srgbClr val="E7E6E6"/></a:lt2>
      <a:accent1><a:srgbClr val="4472C4"/></a:accent1>
      <a:accent2><a:srgbClr val="ED7D31"/></a:accent2>
      <a:accent3><a:srgbClr val="A5A5A5"/></a:accent3>
      <a:accent4><a:srgbClr val="FFC000"/></a:accent4>
      <a:accent5><a:srgbClr val="5B9BD5"/></a:accent5>
      <a:accent6><a:srgbClr val="70AD47"/></a:accent6>
      <a:hlink><a:srgbClr val="0563C1"/></a:hlink>
      <a:folHlink><a:srgbClr val="954F72"/></a:folHlink>
    </a:clrScheme>
    <a:fontScheme name="carta">
      <a:majorFont>
        <a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/>
      </a:majorFont>
      <a:minorFont>
        <a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/>
      </a:minorFont>
    </a:fontScheme>
    <a:fmtScheme name="carta">
      <a:fillStyleLst>
        <a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
        <a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
        <a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
      </a:fillStyleLst>
      <a:lnStyleLst>
        <a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>
        <a:ln w="12700"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>
        <a:ln w="19050"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>
      </a:lnStyleLst>
      <a:effectStyleLst>
        <a:effectStyle><a:effectLst/></a:effectStyle>
        <a:effectStyle><a:effectLst/></a:effectStyle>
        <a:effectStyle><a:effectLst/></a:effectStyle>
      </a:effectStyleLst>
      <a:bgFillStyleLst>
        <a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
        <a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
        <a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
      </a:bgFillStyleLst>
    </a:fmtScheme>
  </a:themeElements>
</a:theme>
"#;
