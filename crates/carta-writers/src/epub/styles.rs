//! The built-in stylesheet linked from every page when the document supplies none of its own. It
//! styles the structural classes the chapter and title-page markup carries (headings, the title
//! block, code, quotations, footnotes, figures and tables) so a book reads acceptably out of the
//! box. Supplying any stylesheet replaces this one outright rather than layering over it.

/// The file name the built-in stylesheet is stored under, and referred to by from each page.
pub(crate) const DEFAULT_STYLESHEET_NAME: &str = "stylesheet1.css";

/// The contents of the built-in stylesheet.
pub(crate) const DEFAULT_STYLESHEET: &str = "\
/* Built-in stylesheet for a generated publication. */

@namespace epub \"http://www.idpf.org/2007/ops\";

body {
  margin: 5%;
  padding: 0;
  line-height: 1.5;
  text-align: justify;
}

h1, h2, h3, h4, h5, h6 {
  font-weight: bold;
  line-height: 1.2;
  text-align: left;
  page-break-after: avoid;
}

h1 { font-size: 1.6em; margin: 1em 0 0.5em; }
h2 { font-size: 1.4em; margin: 1em 0 0.5em; }
h3 { font-size: 1.2em; margin: 1em 0 0.5em; }
h4, h5, h6 { font-size: 1em; margin: 1em 0 0.5em; }

p {
  margin: 0;
  text-indent: 1.5em;
  widows: 2;
  orphans: 2;
}

/* The first paragraph after a heading or a break is not indented. */
h1 + p, h2 + p, h3 + p, h4 + p, h5 + p, h6 + p,
hr + p, blockquote + p, pre + p, figure + p, div.rights p {
  text-indent: 0;
}

a { color: inherit; text-decoration: underline; }
a:link, a:visited { color: inherit; }

/* Title page. */
section.titlepage, body > h1.title {
  text-align: center;
}
h1.title { margin-top: 2em; }
p.subtitle { font-size: 1.2em; font-style: italic; text-align: center; }
p.author { margin: 0.5em 0 0; text-align: center; }
p.publisher, p.date { text-align: center; }
div.rights { margin-top: 2em; font-size: 0.9em; text-align: center; }

/* Quotations. */
blockquote {
  margin: 1em 1.5em;
  padding: 0;
  font-style: italic;
}

/* Code. */
code {
  font-family: monospace, monospace;
  white-space: pre-wrap;
}
pre {
  margin: 1em 0;
  padding: 0.5em;
  white-space: pre-wrap;
  word-wrap: break-word;
  overflow-wrap: break-word;
  font-size: 0.9em;
}
pre code { white-space: inherit; }
div.sourceCode { overflow: auto; }

/* Lists. */
ul, ol { margin: 1em 0; padding-left: 1.5em; }
li { margin: 0.2em 0; }
ul.task-list { list-style: none; padding-left: 1em; }
ul.task-list li input[type=\"checkbox\"] { margin: 0 0.5em 0 -1.4em; }

/* Definition lists. */
dt { font-weight: bold; }
dd { margin: 0 0 0.5em 1.5em; }

/* Figures and images. */
img { max-width: 100%; }
figure { margin: 1em 0; text-align: center; page-break-inside: avoid; }
figcaption { font-size: 0.9em; font-style: italic; text-align: center; }

/* Tables. */
table {
  margin: 1em auto;
  border-collapse: collapse;
  font-size: 0.9em;
}
th, td { padding: 0.3em 0.6em; border-bottom: 1px solid currentColor; }
th { border-bottom: 2px solid currentColor; text-align: left; }
caption { font-style: italic; margin-bottom: 0.4em; }

/* Horizontal rule. */
hr {
  border: none;
  border-top: 1px solid currentColor;
  margin: 1.5em 0;
}

/* Inline emphasis variants. */
span.smallcaps { font-variant: small-caps; }
span.underline { text-decoration: underline; }
mark { background: none; }

/* Multi-column blocks. */
div.columns { display: flex; gap: 1em; }
div.column { flex: 1; }

/* Footnotes. */
a.footnote-ref, a.footnote-back { text-decoration: none; vertical-align: super; font-size: 0.8em; }
section.footnotes, div.footnotes { margin-top: 2em; font-size: 0.9em; }
section.footnotes { border-top: 1px solid currentColor; }
aside[epub|type~=\"footnote\"] { margin: 0.5em 0; }

/* Numbered heading prefixes. */
span.header-section-number::after { content: \" \"; }
";
