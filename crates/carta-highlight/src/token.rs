//! The fixed vocabulary of token kinds a tokenizer emits, with the naming each output format uses.
//!
//! A grammar's `itemData` attaches a default-style name (`dsKeyword`, `dsComment`, …) to every
//! context and rule; that name selects one of these kinds. Each kind then carries the identifiers the
//! renderers and the color model need: the canonical style key (`Keyword`), the compact HTML class
//! (`kw`), and the macro stem the LaTeX writer defines (`KeywordTok`).

/// A single classified span of source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// What the span was classified as.
    pub kind: TokenKind,
    /// The verbatim source text of the span.
    pub text: String,
}

impl Token {
    /// Build a token from a kind and its text.
    pub fn new(kind: TokenKind, text: impl Into<String>) -> Self {
        Token {
            kind,
            text: text.into(),
        }
    }
}

/// One highlighted line: the classified spans it is composed of, in order. Concatenating the spans'
/// text reproduces the line exactly (without its terminating newline).
pub type SourceLine = Vec<Token>;

/// The kinds a token can be classified as. The set and its names are fixed by the grammar format's
/// default-style vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TokenKind {
    /// Unclassified text.
    Normal,
    /// A language keyword.
    Keyword,
    /// A data type.
    DataType,
    /// A decimal integer literal.
    DecVal,
    /// A non-decimal integer literal.
    BaseN,
    /// A floating-point literal.
    Float,
    /// A constant.
    Constant,
    /// A character literal.
    Char,
    /// A special character inside a string or char literal.
    SpecialChar,
    /// A string literal.
    String,
    /// A verbatim (raw) string literal.
    VerbatimString,
    /// A special string such as a regular expression.
    SpecialString,
    /// An import or inclusion directive.
    Import,
    /// A comment.
    Comment,
    /// Documentation embedded in a comment.
    Documentation,
    /// An annotation inside documentation.
    Annotation,
    /// A variable named inside documentation.
    CommentVar,
    /// A function name.
    Function,
    /// A variable name.
    Variable,
    /// A control-flow keyword.
    ControlFlow,
    /// An operator.
    Operator,
    /// A built-in name.
    BuiltIn,
    /// A language extension.
    Extension,
    /// A preprocessor directive.
    Preprocessor,
    /// An attribute.
    Attribute,
    /// A region (folding) marker.
    RegionMarker,
    /// An informational message.
    Information,
    /// A warning message.
    Warning,
    /// An alert such as `TODO` or `FIXME`.
    Alert,
    /// An error message.
    Error,
    /// Anything else the grammar names.
    Other,
}

impl TokenKind {
    /// Resolve a grammar `defStyleNum` name (`dsKeyword`, …) to a kind. Unknown names fall back to
    /// [`TokenKind::Normal`].
    #[must_use]
    pub fn from_default_style(name: &str) -> Self {
        match name {
            "dsKeyword" => TokenKind::Keyword,
            "dsFunction" => TokenKind::Function,
            "dsVariable" => TokenKind::Variable,
            "dsControlFlow" => TokenKind::ControlFlow,
            "dsOperator" => TokenKind::Operator,
            "dsBuiltIn" => TokenKind::BuiltIn,
            "dsExtension" => TokenKind::Extension,
            "dsPreprocessor" => TokenKind::Preprocessor,
            "dsAttribute" => TokenKind::Attribute,
            "dsChar" => TokenKind::Char,
            "dsSpecialChar" => TokenKind::SpecialChar,
            "dsString" => TokenKind::String,
            "dsVerbatimString" => TokenKind::VerbatimString,
            "dsSpecialString" => TokenKind::SpecialString,
            "dsImport" => TokenKind::Import,
            "dsDataType" => TokenKind::DataType,
            "dsDecVal" => TokenKind::DecVal,
            "dsBaseN" => TokenKind::BaseN,
            "dsFloat" => TokenKind::Float,
            "dsConstant" => TokenKind::Constant,
            "dsComment" => TokenKind::Comment,
            "dsDocumentation" => TokenKind::Documentation,
            "dsAnnotation" => TokenKind::Annotation,
            "dsCommentVar" => TokenKind::CommentVar,
            "dsInformation" => TokenKind::Information,
            "dsWarning" => TokenKind::Warning,
            "dsAlert" => TokenKind::Alert,
            "dsError" => TokenKind::Error,
            "dsRegionMarker" => TokenKind::RegionMarker,
            "dsOthers" => TokenKind::Other,
            _ => TokenKind::Normal,
        }
    }

    /// The canonical style key, as used by the color model's `text-styles` map (`Keyword`, …).
    #[must_use]
    pub fn style_key(self) -> &'static str {
        match self {
            TokenKind::Normal => "Normal",
            TokenKind::Keyword => "Keyword",
            TokenKind::DataType => "DataType",
            TokenKind::DecVal => "DecVal",
            TokenKind::BaseN => "BaseN",
            TokenKind::Float => "Float",
            TokenKind::Constant => "Constant",
            TokenKind::Char => "Char",
            TokenKind::SpecialChar => "SpecialChar",
            TokenKind::String => "String",
            TokenKind::VerbatimString => "VerbatimString",
            TokenKind::SpecialString => "SpecialString",
            TokenKind::Import => "Import",
            TokenKind::Comment => "Comment",
            TokenKind::Documentation => "Documentation",
            TokenKind::Annotation => "Annotation",
            TokenKind::CommentVar => "CommentVar",
            TokenKind::Function => "Function",
            TokenKind::Variable => "Variable",
            TokenKind::ControlFlow => "ControlFlow",
            TokenKind::Operator => "Operator",
            TokenKind::BuiltIn => "BuiltIn",
            TokenKind::Extension => "Extension",
            TokenKind::Preprocessor => "Preprocessor",
            TokenKind::Attribute => "Attribute",
            TokenKind::RegionMarker => "RegionMarker",
            TokenKind::Information => "Information",
            TokenKind::Warning => "Warning",
            TokenKind::Alert => "Alert",
            TokenKind::Error => "Error",
            TokenKind::Other => "Other",
        }
    }

    /// The compact class the HTML writer stamps on a span. [`TokenKind::Normal`] gets no class (the
    /// empty string), so its text is emitted without a wrapping span.
    #[must_use]
    pub fn html_class(self) -> &'static str {
        match self {
            TokenKind::Normal => "",
            TokenKind::Keyword => "kw",
            TokenKind::DataType => "dt",
            TokenKind::DecVal => "dv",
            TokenKind::BaseN => "bn",
            TokenKind::Float => "fl",
            TokenKind::Char => "ch",
            TokenKind::String => "st",
            TokenKind::Comment => "co",
            TokenKind::Other => "ot",
            TokenKind::Alert => "al",
            TokenKind::Function => "fu",
            TokenKind::RegionMarker => "re",
            TokenKind::Error => "er",
            TokenKind::Constant => "cn",
            TokenKind::SpecialChar => "sc",
            TokenKind::VerbatimString => "vs",
            TokenKind::SpecialString => "ss",
            TokenKind::Import => "im",
            TokenKind::Documentation => "do",
            TokenKind::Annotation => "an",
            TokenKind::CommentVar => "cv",
            TokenKind::Variable => "va",
            TokenKind::ControlFlow => "cf",
            TokenKind::Operator => "op",
            TokenKind::BuiltIn => "bu",
            TokenKind::Extension => "ex",
            TokenKind::Preprocessor => "pp",
            TokenKind::Attribute => "at",
            TokenKind::Information => "in",
            TokenKind::Warning => "wa",
        }
    }

    /// The macro stem the LaTeX writer wraps a span in (`KeywordTok`, …). Every kind has one,
    /// including [`TokenKind::Normal`] (`NormalTok`).
    #[must_use]
    pub fn latex_macro(self) -> &'static str {
        match self {
            TokenKind::Normal => "NormalTok",
            TokenKind::Keyword => "KeywordTok",
            TokenKind::DataType => "DataTypeTok",
            TokenKind::DecVal => "DecValTok",
            TokenKind::BaseN => "BaseNTok",
            TokenKind::Float => "FloatTok",
            TokenKind::Constant => "ConstantTok",
            TokenKind::Char => "CharTok",
            TokenKind::SpecialChar => "SpecialCharTok",
            TokenKind::String => "StringTok",
            TokenKind::VerbatimString => "VerbatimStringTok",
            TokenKind::SpecialString => "SpecialStringTok",
            TokenKind::Import => "ImportTok",
            TokenKind::Comment => "CommentTok",
            TokenKind::Documentation => "DocumentationTok",
            TokenKind::Annotation => "AnnotationTok",
            TokenKind::CommentVar => "CommentVarTok",
            TokenKind::Function => "FunctionTok",
            TokenKind::Variable => "VariableTok",
            TokenKind::ControlFlow => "ControlFlowTok",
            TokenKind::Operator => "OperatorTok",
            TokenKind::BuiltIn => "BuiltInTok",
            TokenKind::Extension => "ExtensionTok",
            TokenKind::Preprocessor => "PreprocessorTok",
            TokenKind::Attribute => "AttributeTok",
            TokenKind::RegionMarker => "RegionMarkerTok",
            TokenKind::Information => "InformationTok",
            TokenKind::Warning => "WarningTok",
            TokenKind::Alert => "AlertTok",
            TokenKind::Error => "ErrorTok",
            TokenKind::Other => "OtherTok",
        }
    }
}
