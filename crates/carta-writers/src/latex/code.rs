//! Code-block and inline-code rendering, with optional syntax highlighting.

#[cfg(feature = "highlight")]
use std::fmt::Write as _;

use carta_ast::Attr;
use carta_core::WriterOptions;

#[cfg(feature = "highlight")]
use crate::highlight::{is_number_lines_class, plain_source_lines, start_line};
#[cfg(feature = "highlight")]
use carta_highlight::{Highlighter, Token as HighlightToken, TokenKind};

#[cfg(feature = "highlight")]
use super::escaping::{EscapeMode, escape};
use super::phantom_label;

/// The code-block presentation threaded through a render, or a zero-size placeholder when the feature
/// is compiled out, so every rendering function keeps one signature. `highlighter` colorizes a block;
/// `idiomatic` instead selects the `lstlisting` environment for the uncolorized fallback.
#[cfg(feature = "highlight")]
#[derive(Clone, Copy)]
pub(crate) struct Hl<'a> {
    highlighter: Option<&'a Highlighter>,
    idiomatic: bool,
}
#[cfg(not(feature = "highlight"))]
pub(crate) type Hl<'a> = core::marker::PhantomData<&'a ()>;

/// The code-block presentation a render draws from the writer options.
#[cfg(feature = "highlight")]
pub(crate) fn code_highlighting(options: &WriterOptions) -> Hl<'_> {
    Hl {
        highlighter: options.highlight.highlighter.as_deref(),
        idiomatic: options.highlight.idiomatic,
    }
}
#[cfg(not(feature = "highlight"))]
pub(crate) fn code_highlighting(_options: &WriterOptions) -> Hl<'_> {
    core::marker::PhantomData
}

pub(super) fn code_block(attr: &Attr, text: &str, hl: Hl<'_>) -> String {
    highlighted_code_block(attr, text, hl)
        .unwrap_or_else(|| code_block_fallback(attr, text, hl, "verbatim"))
}

/// The uncolorized rendering of a code block: under idiomatic presentation the format's own
/// `lstlisting` construct, carrying the block's language, numbering, and identifier as options;
/// otherwise the plain verbatim environment named by `plain_env` (`verbatim` in the document body,
/// the reduced `Verbatim` inside a footnote).
#[cfg(feature = "highlight")]
pub(super) fn code_block_fallback(attr: &Attr, text: &str, hl: Hl<'_>, plain_env: &str) -> String {
    if hl.idiomatic {
        listings_code_block(attr, text)
    } else {
        code_block_env(attr, text, plain_env)
    }
}
#[cfg(not(feature = "highlight"))]
pub(super) fn code_block_fallback(attr: &Attr, text: &str, _hl: Hl<'_>, plain_env: &str) -> String {
    code_block_env(attr, text, plain_env)
}

/// A code block rendered as a `lstlisting` environment. The listings package names the language, line
/// numbering, and starting line through bracket options, so the block's first mappable class, its
/// numbering class and `startFrom` key, any further key–value attributes, and its identifier all
/// become options rather than markup around the verbatim body.
#[cfg(feature = "highlight")]
fn listings_code_block(attr: &Attr, text: &str) -> String {
    let body = text.strip_suffix('\n').unwrap_or(text);
    let options = listings_options(attr);
    if options.is_empty() {
        format!("\\begin{{lstlisting}}\n{body}\n\\end{{lstlisting}}")
    } else {
        format!("\\begin{{lstlisting}}[{options}]\n{body}\n\\end{{lstlisting}}")
    }
}

/// Assemble the bracket options for a `lstlisting` block. The order is fixed (language, then
/// `numbers=left`, then `firstnumber`, then any pass-through attributes, then the `label`) so the
/// rendering is deterministic and reads left to right the way the listings package documents.
#[cfg(feature = "highlight")]
pub(super) fn listings_options(attr: &Attr) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(language) = attr
        .classes
        .iter()
        .find_map(|class| listings_language(class.as_str()))
    {
        parts.push(format!("language={language}"));
    }
    if attr.classes.iter().any(is_number_lines_class) {
        parts.push(String::from("numbers=left"));
    }
    if let Some((_, value)) = attr
        .attributes
        .iter()
        .find(|(key, _)| key.as_str() == "startFrom")
        && let Ok(start) = value.as_str().parse::<i64>()
    {
        parts.push(format!("firstnumber={start}"));
    }
    for (key, value) in &attr.attributes {
        if key.as_str() == "startFrom" {
            continue;
        }
        parts.push(format!(
            "{}={}",
            key.as_str(),
            listings_value(value.as_str())
        ));
    }
    if !attr.id.is_empty() {
        parts.push(format!("label={}", attr.id));
    }
    parts.join(", ")
}

/// A listings option value: emitted bare when it is a single run of ASCII letters and digits, and
/// otherwise wrapped in a group with the LaTeX metacharacters escaped so the brackets and commas of
/// the option list stay unambiguous.
#[cfg(feature = "highlight")]
fn listings_value(value: &str) -> String {
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        value.to_owned()
    } else {
        format!("{{{}}}", escape(value, EscapeMode::Text))
    }
}

/// The `listings` package name for a code-block class, or `None` when the package does not cover the
/// language. The lookup is case-insensitive; the returned name is the exact spelling the package's
/// `language=` option expects, with the handful that carry special characters already braced so they
/// survive the option list. Names follow the language table published with the listings package.
#[cfg(feature = "highlight")]
// Aliases and dialects share target names, so equal arms are expected; the table is long by nature.
#[allow(clippy::match_same_arms, clippy::too_many_lines)]
pub(super) fn listings_language(class: &str) -> Option<&'static str> {
    let key = class.to_ascii_lowercase();
    Some(match key.as_str() {
        "abap" => "ABAP",
        "acsl" => "ACSL",
        "ada" => "Ada",
        "algol" => "Algol",
        "ant" => "Ant",
        "assembler" => "Assembler",
        "awk" => "Awk",
        "bash" => "bash",
        "c" => "C",
        "c++" => "{C++}",
        "cil" => "CIL",
        "clean" => "Clean",
        "cobol" => "Cobol",
        "comal80" => "Comal80",
        "command.com" => "{command.com}",
        "commonlisp" => "Lisp",
        "comsol" => "Comsol",
        "cpp" => "{C++}",
        "cs" => "C",
        "csh" => "csh",
        "delphi" => "Delphi",
        "eiffel" => "Eiffel",
        "elan" => "Elan",
        "erlang" => "erlang",
        "euphoria" => "Euphoria",
        "fortran" => "Fortran",
        "gap" => "GAP",
        "gcl" => "GCL",
        "gnuassembler" => "Assembler",
        "gnuplot" => "Gnuplot",
        "go" => "Go",
        "hansl" => "hansl",
        "haskell" => "Haskell",
        "html" => "HTML",
        "idl" => "IDL",
        "inform" => "inform",
        "java" => "Java",
        "jvmis" => "JVMIS",
        "ksh" => "ksh",
        "latex" => "TeX",
        "lingo" => "Lingo",
        "lisp" => "Lisp",
        "llvm" => "LLVM",
        "logo" => "Logo",
        "lua" => "Lua",
        "make" => "make",
        "makefile" => "make",
        "mathematica" => "Mathematica",
        "matlab" => "Matlab",
        "mercury" => "Mercury",
        "metapost" => "MetaPost",
        "miranda" => "Miranda",
        "mizar" => "Mizar",
        "ml" => "ML",
        "modula2" => "{Modula-2}",
        "monobasic" => "Basic",
        "mupad" => "MuPAD",
        "nastran" => "NASTRAN",
        "oberon2" => "{Oberon-2}",
        "objective-c" => "C",
        "objectivec" => "C",
        "ocaml" => "Caml",
        "ocl" => "OCL",
        "octave" => "Octave",
        "oz" => "Oz",
        "pascal" => "Pascal",
        "perl" => "Perl",
        "php" => "PHP",
        "plasm" => "Plasm",
        "pli" => "{PL/I}",
        "postscript" => "PostScript",
        "pov" => "POV",
        "prolog" => "Prolog",
        "promela" => "Promela",
        "pstricks" => "PSTricks",
        "purebasic" => "Basic",
        "python" => "Python",
        "r" => "R",
        "reduce" => "Reduce",
        "rexx" => "Rexx",
        "rsl" => "RSL",
        "ruby" => "Ruby",
        "s" => "S",
        "sas" => "SAS",
        "scala" => "Scala",
        "scilab" => "Scilab",
        "sh" => "sh",
        "shelxl" => "SHELXL",
        "simula" => "Simula",
        "sparql" => "SPARQL",
        "sql" => "SQL",
        "swift" => "Swift",
        "tcl" => "tcl",
        "tex" => "TeX",
        "vbscript" => "VBScript",
        "verilog" => "Verilog",
        "vhdl" => "VHDL",
        "vrml" => "VRML",
        "xml" => "XML",
        "xslt" => "XSLT",
        _ => return None,
    })
}

/// The colorized form of a code block, or `None` when it is not colorized: no highlighter is active,
/// or the block carries no class to name a language or request line numbering. A colorized block is a
/// `Shaded`/`Highlighting` environment pair regardless of the surrounding context.
#[cfg(feature = "highlight")]
pub(super) fn highlighted_code_block(attr: &Attr, text: &str, hl: Hl<'_>) -> Option<String> {
    let highlighter = hl.highlighter?;
    if attr.classes.is_empty() {
        return None;
    }
    let code = text.strip_suffix('\n').unwrap_or(text);
    let language = attr
        .classes
        .iter()
        .find(|class| highlighter.registry().is_known(class.as_str()));
    let lines = match language {
        Some(language) => highlighter
            .highlight(language.as_str(), code)
            .unwrap_or_default(),
        None => plain_source_lines(code),
    };
    let numbered = attr.classes.iter().any(is_number_lines_class);
    let mut body = String::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            body.push('\n');
        }
        for token in line {
            push_highlight_token(&mut body, token);
        }
    }
    let options = highlight_environment_options(attr, numbered);
    let shaded = format!(
        "\\begin{{Shaded}}\n\\begin{{Highlighting}}[{options}]\n{body}\n\\end{{Highlighting}}\n\\end{{Shaded}}"
    );
    Some(if attr.id.is_empty() {
        shaded
    } else {
        format!("{}%\n{shaded}", phantom_label(&attr.id))
    })
}

#[cfg(not(feature = "highlight"))]
pub(super) fn highlighted_code_block(_attr: &Attr, _text: &str, _hl: Hl<'_>) -> Option<String> {
    None
}

/// The colorized form of inline code, or `None` when it does not apply: no highlighter is active, or
/// the span names no known language. The token macros ride inside a `\VERB` group delimited by a
/// bar; a literal bar in the source becomes `\VerbBar{}` so it cannot close the group early.
#[cfg(feature = "highlight")]
pub(super) fn highlighted_code_inline(attr: &Attr, text: &str, hl: Hl<'_>) -> Option<String> {
    let highlighter = hl.highlighter?;
    let language = attr
        .classes
        .iter()
        .find(|class| highlighter.registry().is_known(class.as_str()))?;
    let lines = highlighter
        .highlight(language.as_str(), text)
        .unwrap_or_default();
    let mut body = String::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            body.push('\n');
        }
        for token in line {
            push_highlight_token(&mut body, token);
        }
    }
    Some(format!("\\VERB|{}|", body.replace('|', "\\VerbBar{}")))
}

/// The idiomatic-listings form of inline code, or `None` when idiomatic presentation is off. The
/// source rides verbatim inside `\lstinline`, carrying a `language=` option when a class maps to a
/// listings language; the delimiter is the first candidate glyph absent from the text.
#[cfg(feature = "highlight")]
pub(super) fn idiomatic_code_inline(attr: &Attr, text: &str, hl: Hl<'_>) -> Option<String> {
    if !hl.idiomatic {
        return None;
    }
    let language = attr
        .classes
        .iter()
        .find_map(|class| listings_language(class.as_str()))
        .map(|name| format!("[language={name}]"))
        .unwrap_or_default();
    let delimiter = lstinline_delimiter(text);
    Some(format!(
        "\\passthrough{{\\lstinline{language}{delimiter}{text}{delimiter}}}"
    ))
}

/// Pick the delimiter for an `\lstinline` argument: the first punctuation glyph the source does not
/// itself contain, so the argument cannot terminate early. When the source contains every candidate,
/// the first is reused (the argument cannot be delimited cleanly, matching the listings fallback).
#[cfg(feature = "highlight")]
fn lstinline_delimiter(text: &str) -> char {
    const CANDIDATES: &str = "!\"'()*+,-./:;<=>?@";
    CANDIDATES
        .chars()
        .find(|candidate| !text.contains(*candidate))
        .unwrap_or('!')
}

#[cfg(not(feature = "highlight"))]
pub(super) fn highlighted_code_inline(_attr: &Attr, _text: &str, _hl: Hl<'_>) -> Option<String> {
    None
}

#[cfg(not(feature = "highlight"))]
pub(super) fn idiomatic_code_inline(_attr: &Attr, _text: &str, _hl: Hl<'_>) -> Option<String> {
    None
}

/// Append one classified token as its style macro, `\StyleTok{escaped}`. A `Normal` token that is
/// only whitespace is emitted bare so inter-token spacing is not wrapped in a macro.
#[cfg(feature = "highlight")]
fn push_highlight_token(out: &mut String, token: &HighlightToken) {
    let escaped = escape_highlight(&token.text);
    if token.kind == TokenKind::Normal && token.text.chars().all(char::is_whitespace) {
        out.push_str(&escaped);
    } else {
        let _ = write!(out, "\\{}Tok{{{escaped}}}", token.kind.style_key());
    }
}

/// The bracket options on the `Highlighting` environment: empty unless the block numbers its lines,
/// where it states `numbers=left` and, when the first line is not 1, a `firstnumber`.
#[cfg(feature = "highlight")]
fn highlight_environment_options(attr: &Attr, numbered: bool) -> String {
    if !numbered {
        return String::new();
    }
    let start = start_line(attr);
    let mut parts = vec![String::from("numbers=left"), String::new()];
    if start != 1 {
        parts.push(format!("firstnumber={start}"));
    }
    parts.push(String::new());
    parts.join(",")
}

/// Escape a run of source text for a highlighting token macro's argument. Unlike prose escaping, a
/// control-word escape always closes with an empty group, and whitespace and several glyphs that are
/// literal inside a listing pass through untouched.
#[cfg(feature = "highlight")]
fn escape_highlight(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\textbackslash{}"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '_' => out.push_str("\\_"),
            '&' => out.push_str("\\&"),
            '%' => out.push_str("\\%"),
            '#' => out.push_str("\\#"),
            '^' => out.push_str("\\^{}"),
            '~' => out.push_str("\\textasciitilde{}"),
            '<' => out.push_str("\\textless{}"),
            '>' => out.push_str("\\textgreater{}"),
            '`' => out.push_str("\\textasciigrave{}"),
            '\'' => out.push_str("\\textquotesingle{}"),
            '-' => out.push_str("{-}"),
            other => out.push(other),
        }
    }
    out
}

fn code_block_env(attr: &Attr, text: &str, environment: &str) -> String {
    let body = text.strip_suffix('\n').unwrap_or(text);
    let verbatim = format!("\\begin{{{environment}}}\n{body}\n\\end{{{environment}}}");
    if attr.id.is_empty() {
        verbatim
    } else {
        format!("{}%\n{verbatim}", phantom_label(&attr.id))
    }
}
