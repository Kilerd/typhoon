//! Tokens produced by the lexer.

use std::fmt;

use typhoon_diag::Span;

/// A lexed token: its kind and the source range it covers.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    /// What kind of token this is.
    pub kind: TokenKind,
    /// The source range of the token. Layout tokens ([`TokenKind::Indent`],
    /// [`TokenKind::Dedent`], [`TokenKind::Eof`]) are empty spans.
    pub span: Span,
}

impl Token {
    /// Creates a token.
    pub fn new(kind: TokenKind, span: Span) -> Token {
        Token { kind, span }
    }

    /// A human readable description of the token, for `expected ..., found ...`
    /// diagnostics. See [`TokenKind::describe`].
    pub fn describe(&self) -> String {
        self.kind.describe()
    }
}

/// One part of an f-string as the lexer sees it.
///
/// The lexer does not parse the interpolated expressions; it hands the parser
/// the raw source text together with the span it came from, so the parser can
/// re-lex it and still produce spans that point at the original file.
#[derive(Debug, Clone, PartialEq)]
pub enum RawFStringPart {
    /// Literal text; escape sequences and `{{` / `}}` are already decoded.
    Literal {
        /// The decoded text.
        value: String,
        /// Span of the text inside the literal (including the `{{` if any).
        span: Span,
    },
    /// A `{expr}` or `{expr:spec}` hole.
    Expr {
        /// The raw source text of the expression, exactly as written.
        text: String,
        /// Span of `text` in the file; `text` is the source slice of this span.
        span: Span,
        /// The raw format spec after the `:`, if present.
        spec: Option<String>,
        /// Span of the format spec text.
        spec_span: Option<Span>,
        /// Span of the whole hole, from `{` to `}` inclusive.
        full_span: Span,
    },
}

/// Every token kind of the typhoon language.
///
/// All keywords listed in DESIGN.md are reserved from day one, even those whose
/// features arrive in a later milestone (`import`, `try`, `match`, ...), so that
/// programs do not silently use them as identifiers today.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    /// An identifier.
    Ident(String),

    // ---- keywords ----
    /// `fn`
    Fn,
    /// `class`
    Class,
    /// `if`
    If,
    /// `elif`
    Elif,
    /// `else`
    Else,
    /// `while`
    While,
    /// `for`
    For,
    /// `in`
    In,
    /// `not`
    Not,
    /// `and`
    And,
    /// `or`
    Or,
    /// `return`
    Return,
    /// `break`
    Break,
    /// `continue`
    Continue,
    /// `pass`
    Pass,
    /// `True`
    True,
    /// `False`
    False,
    /// `None`
    None,
    /// `is`
    Is,
    /// `import` (reserved, M4)
    Import,
    /// `from` (reserved, M4)
    From,
    /// `try` (reserved, M4)
    Try,
    /// `except` (reserved, M4)
    Except,
    /// `finally` (reserved, M4)
    Finally,
    /// `raise` (reserved, M4)
    Raise,
    /// `match` (reserved, open question)
    Match,
    /// `case` (reserved, open question)
    Case,
    /// `as` (reserved, M4)
    As,

    // ---- literals ----
    /// An integer literal, already decoded (`0x`, `0b`, `0o` and `_` handled).
    Int(i64),
    /// A float literal.
    Float(f64),
    /// A string literal with escapes decoded.
    Str(String),
    /// An f-string literal, split into literal and expression parts.
    FString(Vec<RawFStringPart>),

    // ---- operators and punctuation ----
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `/`
    Slash,
    /// `//`
    SlashSlash,
    /// `%`
    Percent,
    /// `**`
    StarStar,
    /// `==`
    EqEq,
    /// `!=`
    BangEq,
    /// `<`
    Lt,
    /// `<=`
    LtEq,
    /// `>`
    Gt,
    /// `>=`
    GtEq,
    /// `=`
    Eq,
    /// `:`
    Colon,
    /// `,`
    Comma,
    /// `.`
    Dot,
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `[`
    LBracket,
    /// `]`
    RBracket,
    /// `{`
    LBrace,
    /// `}`
    RBrace,
    /// `->`
    Arrow,
    /// `|`
    Pipe,
    /// `&`
    Amp,
    /// `^`
    Caret,
    /// `~`
    Tilde,
    /// `<<`
    LtLt,
    /// `>>`
    GtGt,
    /// `+=` (only lexed so the parser can explain that it is not supported)
    PlusEq,
    /// `-=` (only lexed so the parser can explain that it is not supported)
    MinusEq,
    /// `*=` (only lexed so the parser can explain that it is not supported)
    StarEq,
    /// `/=` (only lexed so the parser can explain that it is not supported)
    SlashEq,
    /// `;` (only lexed so the parser can explain that it is not used)
    Semi,

    // ---- layout ----
    /// End of a logical line.
    Newline,
    /// Start of a deeper indented block.
    Indent,
    /// End of an indented block.
    Dedent,
    /// End of input.
    Eof,
}

impl TokenKind {
    /// Maps an identifier spelling to its keyword token, if it is one.
    pub fn keyword(text: &str) -> Option<TokenKind> {
        let kind = match text {
            "fn" => TokenKind::Fn,
            "class" => TokenKind::Class,
            "if" => TokenKind::If,
            "elif" => TokenKind::Elif,
            "else" => TokenKind::Else,
            "while" => TokenKind::While,
            "for" => TokenKind::For,
            "in" => TokenKind::In,
            "not" => TokenKind::Not,
            "and" => TokenKind::And,
            "or" => TokenKind::Or,
            "return" => TokenKind::Return,
            "break" => TokenKind::Break,
            "continue" => TokenKind::Continue,
            "pass" => TokenKind::Pass,
            "True" => TokenKind::True,
            "False" => TokenKind::False,
            "None" => TokenKind::None,
            "is" => TokenKind::Is,
            "import" => TokenKind::Import,
            "from" => TokenKind::From,
            "try" => TokenKind::Try,
            "except" => TokenKind::Except,
            "finally" => TokenKind::Finally,
            "raise" => TokenKind::Raise,
            "match" => TokenKind::Match,
            "case" => TokenKind::Case,
            "as" => TokenKind::As,
            _ => return Option::None,
        };
        Some(kind)
    }

    /// The source spelling of a keyword, operator or punctuation token.
    /// Returns `None` for tokens whose text varies (identifiers, literals) and
    /// for layout tokens.
    pub fn fixed_text(&self) -> Option<&'static str> {
        let text = match self {
            TokenKind::Fn => "fn",
            TokenKind::Class => "class",
            TokenKind::If => "if",
            TokenKind::Elif => "elif",
            TokenKind::Else => "else",
            TokenKind::While => "while",
            TokenKind::For => "for",
            TokenKind::In => "in",
            TokenKind::Not => "not",
            TokenKind::And => "and",
            TokenKind::Or => "or",
            TokenKind::Return => "return",
            TokenKind::Break => "break",
            TokenKind::Continue => "continue",
            TokenKind::Pass => "pass",
            TokenKind::True => "True",
            TokenKind::False => "False",
            TokenKind::None => "None",
            TokenKind::Is => "is",
            TokenKind::Import => "import",
            TokenKind::From => "from",
            TokenKind::Try => "try",
            TokenKind::Except => "except",
            TokenKind::Finally => "finally",
            TokenKind::Raise => "raise",
            TokenKind::Match => "match",
            TokenKind::Case => "case",
            TokenKind::As => "as",
            TokenKind::Plus => "+",
            TokenKind::Minus => "-",
            TokenKind::Star => "*",
            TokenKind::Slash => "/",
            TokenKind::SlashSlash => "//",
            TokenKind::Percent => "%",
            TokenKind::StarStar => "**",
            TokenKind::EqEq => "==",
            TokenKind::BangEq => "!=",
            TokenKind::Lt => "<",
            TokenKind::LtEq => "<=",
            TokenKind::Gt => ">",
            TokenKind::GtEq => ">=",
            TokenKind::Eq => "=",
            TokenKind::Colon => ":",
            TokenKind::Comma => ",",
            TokenKind::Dot => ".",
            TokenKind::LParen => "(",
            TokenKind::RParen => ")",
            TokenKind::LBracket => "[",
            TokenKind::RBracket => "]",
            TokenKind::LBrace => "{",
            TokenKind::RBrace => "}",
            TokenKind::Arrow => "->",
            TokenKind::Pipe => "|",
            TokenKind::Amp => "&",
            TokenKind::Caret => "^",
            TokenKind::Tilde => "~",
            TokenKind::LtLt => "<<",
            TokenKind::GtGt => ">>",
            TokenKind::PlusEq => "+=",
            TokenKind::MinusEq => "-=",
            TokenKind::StarEq => "*=",
            TokenKind::SlashEq => "/=",
            TokenKind::Semi => ";",
            _ => return Option::None,
        };
        Some(text)
    }

    /// Whether this token is a reserved keyword.
    pub fn is_keyword(&self) -> bool {
        matches!(
            self,
            TokenKind::Fn
                | TokenKind::Class
                | TokenKind::If
                | TokenKind::Elif
                | TokenKind::Else
                | TokenKind::While
                | TokenKind::For
                | TokenKind::In
                | TokenKind::Not
                | TokenKind::And
                | TokenKind::Or
                | TokenKind::Return
                | TokenKind::Break
                | TokenKind::Continue
                | TokenKind::Pass
                | TokenKind::True
                | TokenKind::False
                | TokenKind::None
                | TokenKind::Is
                | TokenKind::Import
                | TokenKind::From
                | TokenKind::Try
                | TokenKind::Except
                | TokenKind::Finally
                | TokenKind::Raise
                | TokenKind::Match
                | TokenKind::Case
                | TokenKind::As
        )
    }

    /// The identifier text, for [`TokenKind::Ident`] only.
    pub fn ident_name(&self) -> Option<&str> {
        match self {
            TokenKind::Ident(name) => Some(name),
            _ => Option::None,
        }
    }

    /// A human readable description of the token, used in
    /// `expected ..., found ...` diagnostics.
    pub fn describe(&self) -> String {
        match self {
            TokenKind::Ident(name) => format!("`{name}`"),
            TokenKind::Int(v) => format!("`{v}`"),
            TokenKind::Float(v) => format!("`{v:?}`"),
            TokenKind::Str(_) => "string literal".to_string(),
            TokenKind::FString(_) => "f-string".to_string(),
            TokenKind::Newline => "end of line".to_string(),
            TokenKind::Indent => "indented block".to_string(),
            TokenKind::Dedent => "end of block".to_string(),
            TokenKind::Eof => "end of file".to_string(),
            other => match other.fixed_text() {
                Some(text) => format!("`{text}`"),
                Option::None => "token".to_string(),
            },
        }
    }
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_round_trip() {
        for text in [
            "fn", "class", "if", "elif", "else", "while", "for", "in", "not", "and", "or",
            "return", "break", "continue", "pass", "True", "False", "None", "is", "import", "from",
            "try", "except", "finally", "raise", "match", "case", "as",
        ] {
            let kind =
                TokenKind::keyword(text).unwrap_or_else(|| panic!("`{text}` is not a keyword"));
            assert!(kind.is_keyword(), "`{text}` should be a keyword");
            assert_eq!(kind.fixed_text(), Some(text));
        }
    }

    #[test]
    fn non_keywords() {
        for text in ["def", "self", "print", "iff", "Fn", "lambda", "int", "list"] {
            assert_eq!(
                TokenKind::keyword(text),
                None,
                "`{text}` must not be a keyword"
            );
        }
    }

    #[test]
    fn identifiers_are_not_keywords() {
        assert!(!TokenKind::Ident("fnord".into()).is_keyword());
        assert_eq!(TokenKind::Ident("fnord".into()).ident_name(), Some("fnord"));
        assert_eq!(TokenKind::Int(1).ident_name(), None);
    }

    #[test]
    fn describe_is_diagnostic_friendly() {
        assert_eq!(TokenKind::Colon.describe(), "`:`");
        assert_eq!(TokenKind::Fn.describe(), "`fn`");
        assert_eq!(TokenKind::Ident("x".into()).describe(), "`x`");
        assert_eq!(TokenKind::Int(42).describe(), "`42`");
        assert_eq!(TokenKind::Float(1.5).describe(), "`1.5`");
        assert_eq!(TokenKind::Str("hi".into()).describe(), "string literal");
        assert_eq!(TokenKind::FString(vec![]).describe(), "f-string");
        assert_eq!(TokenKind::Newline.describe(), "end of line");
        assert_eq!(TokenKind::Indent.describe(), "indented block");
        assert_eq!(TokenKind::Dedent.describe(), "end of block");
        assert_eq!(TokenKind::Eof.describe(), "end of file");
        assert_eq!(TokenKind::GtGt.to_string(), "`>>`");
    }

    #[test]
    fn fixed_text_is_none_for_variable_tokens() {
        assert_eq!(TokenKind::Ident("x".into()).fixed_text(), None);
        assert_eq!(TokenKind::Int(1).fixed_text(), None);
        assert_eq!(TokenKind::Newline.fixed_text(), None);
    }
}
