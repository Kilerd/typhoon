//! The hand-written, indentation-aware scanner.
//!
//! The scanner walks the source text once, from left to right, keeping four
//! pieces of state: the byte cursor [`Scanner::pos`], the indentation stack
//! [`Scanner::indents`], the bracket nesting depth [`Scanner::brackets`] and
//! whether the logical line being scanned has produced a token yet
//! ([`Scanner::line_has_tokens`]). Everything else is local to one method.
//!
//! See the crate documentation for the token stream shape that comes out.

use typhoon_diag::{Diagnostic, Diagnostics, FileId, Span};

use crate::token::{RawFStringPart, Token, TokenKind};

/// Returns `true` if `c` can start an identifier.
fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_ascii_alphabetic()
}

/// Returns `true` if `c` can continue an identifier.
fn is_ident_continue(c: char) -> bool {
    c == '_' || c.is_ascii_alphanumeric()
}

/// Renders a character for a diagnostic message, escaping control characters.
fn quoted(c: char) -> String {
    format!("`{}`", c.escape_debug())
}

/// The lexer state machine. Created and driven by [`crate::tokenize_fragment`].
pub(crate) struct Scanner<'a> {
    /// The text being scanned. Offsets in the produced spans are relative to
    /// this text plus [`Scanner::base`].
    src: &'a str,
    /// The file every produced span points into.
    file: FileId,
    /// Added to every offset, so that fragments (f-string holes) produce spans
    /// pointing at the enclosing file.
    base: u32,
    /// Byte offset of the cursor inside `src`; always on a `char` boundary.
    pos: usize,
    /// The tokens produced so far.
    tokens: Vec<Token>,
    /// Open indentation columns, smallest first. Never empty; `indents[0]` is
    /// the column of the outermost (top level) block, normally `0`.
    indents: Vec<u32>,
    /// Indentation column of the current line, held back until the line
    /// actually produces a token. A line that produces no token at all (blank,
    /// comment only, or only invalid characters) never touches `indents`.
    pending_indent: Option<u32>,
    /// How many `(`, `[` or `{` are currently open. Line breaks produce no
    /// layout tokens while this is non-zero (implicit line continuation).
    brackets: u32,
    /// Whether the current logical line has produced at least one token, i.e.
    /// whether it still needs a terminating [`TokenKind::Newline`].
    line_has_tokens: bool,
    /// Where diagnostics go.
    diags: &'a mut Diagnostics,
}

impl<'a> Scanner<'a> {
    /// Creates a scanner over `src`.
    pub(crate) fn new(
        file: FileId,
        src: &'a str,
        base: u32,
        diags: &'a mut Diagnostics,
    ) -> Scanner<'a> {
        Scanner {
            src,
            file,
            base,
            pos: 0,
            tokens: Vec::new(),
            indents: vec![0],
            pending_indent: None,
            brackets: 0,
            line_has_tokens: false,
            diags,
        }
    }

    // ---------------------------------------------------------------- cursor

    /// The character at the cursor, if any.
    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    /// The `n`-th character after the cursor (`n == 0` is [`Scanner::peek`]).
    fn peek_nth(&self, n: usize) -> Option<char> {
        self.src[self.pos..].chars().nth(n)
    }

    /// The byte at absolute offset `at`, if any.
    fn byte_at(&self, at: usize) -> Option<u8> {
        self.src.as_bytes().get(at).copied()
    }

    /// Consumes and returns the character at the cursor.
    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    /// Consumes the character at the cursor if it is `c`.
    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += c.len_utf8();
            true
        } else {
            false
        }
    }

    /// Whether the whole input has been consumed.
    fn at_eof(&self) -> bool {
        self.pos >= self.src.len()
    }

    /// Whether the cursor sits on a line break (`\n` or `\r\n`). A lone `\r`
    /// is ordinary whitespace, not a line break.
    fn at_line_break(&self) -> bool {
        match self.byte_at(self.pos) {
            Some(b'\n') => true,
            Some(b'\r') => self.byte_at(self.pos + 1) == Some(b'\n'),
            _ => false,
        }
    }

    /// Consumes a `\n` or `\r\n` line break; does nothing otherwise.
    fn skip_line_break(&mut self) {
        if self.byte_at(self.pos) == Some(b'\r') && self.byte_at(self.pos + 1) == Some(b'\n') {
            self.pos += 2;
        } else if self.byte_at(self.pos) == Some(b'\n') {
            self.pos += 1;
        }
    }

    /// Absolute (file relative) offset of the local offset `offset`.
    fn abs(&self, offset: usize) -> u32 {
        let offset = u32::try_from(offset).unwrap_or(u32::MAX);
        self.base.saturating_add(offset)
    }

    /// The span of `start..end`, shifted into file coordinates.
    fn span(&self, start: usize, end: usize) -> Span {
        Span::new(self.file, self.abs(start), self.abs(end))
    }

    // ------------------------------------------------------------ diagnostics

    /// Records a diagnostic.
    fn error(&mut self, diag: Diagnostic) {
        self.diags.push(diag);
    }

    // ----------------------------------------------------------------- output

    /// Pushes a token, flushing the pending indentation change first.
    fn emit(&mut self, kind: TokenKind, start: usize, end: usize) {
        let span = self.span(start, end);
        if let Some(col) = self.pending_indent.take() {
            self.flush_indent(col, span);
        }
        self.line_has_tokens = true;
        self.tokens.push(Token::new(kind, span));
    }

    /// Emits the `Indent` / `Dedent` tokens for a line whose first token spans
    /// `first`, and updates the indentation stack.
    fn flush_indent(&mut self, col: u32, first: Span) {
        let at = first.shrink_to_start();
        let current = *self.indents.last().expect("indent stack is never empty");
        if col > current {
            self.indents.push(col);
            self.tokens.push(Token::new(TokenKind::Indent, at));
            return;
        }
        if col == current {
            return;
        }
        while self.indents.len() > 1 && *self.indents.last().expect("non empty") > col {
            self.indents.pop();
            self.tokens.push(Token::new(TokenKind::Dedent, at));
        }
        if *self.indents.last().expect("non empty") != col {
            let levels = self
                .indents
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            let plural = if col == 1 { "space" } else { "spaces" };
            self.error(
                Diagnostic::error("unindent does not match any outer indentation level")
                    .with_label(first, format!("this line is indented by {col} {plural}"))
                    .with_note(format!("the open indentation levels are {levels}")),
            );
            *self.indents.last_mut().expect("non empty") = col;
        }
    }

    // ------------------------------------------------------------------- main

    /// Scans the whole input and returns the token stream.
    pub(crate) fn run(mut self) -> Vec<Token> {
        loop {
            if self.brackets == 0 && !self.start_line() {
                break;
            }
            if !self.scan_line() {
                break;
            }
        }
        self.finish()
    }

    /// Handles the start of a logical line: measures the indentation and skips
    /// blank and comment-only lines. Returns `false` at end of input.
    fn start_line(&mut self) -> bool {
        loop {
            let (col, tab) = self.scan_indentation();
            if self.at_eof() {
                return false;
            }
            if self.at_line_break() {
                self.skip_line_break();
                continue;
            }
            if self.peek() == Some('#') {
                self.skip_comment();
                if self.at_eof() {
                    return false;
                }
                self.skip_line_break();
                continue;
            }
            // Only a line that carries code is indented at all, so a tab on a
            // blank or comment-only line is not worth a diagnostic.
            if let Some(at) = tab {
                self.error(
                    Diagnostic::error("tabs are not allowed in indentation")
                        .with_label(self.span(at, at + 1), "tab used here")
                        .with_help("use 4 spaces per indentation level"),
                );
            }
            self.pending_indent = Some(col);
            self.line_has_tokens = false;
            return true;
        }
    }

    /// Consumes the leading whitespace of a line and returns its column
    /// together with the offset of its first tab, if any.
    ///
    /// Only spaces count as indentation. A tab counts as a single space (and is
    /// reported by the caller, once per line); a lone `\r` is ordinary
    /// whitespace and does not count at all.
    fn scan_indentation(&mut self) -> (u32, Option<usize>) {
        let mut col = 0u32;
        let mut tab = None;
        loop {
            match self.byte_at(self.pos) {
                Some(b' ') => {
                    self.pos += 1;
                    col += 1;
                }
                Some(b'\t') => {
                    if tab.is_none() {
                        tab = Some(self.pos);
                    }
                    self.pos += 1;
                    col += 1;
                }
                Some(b'\r') if !self.at_line_break() => self.pos += 1,
                _ => return (col, tab),
            }
        }
    }

    /// Scans one logical line. Returns `false` at end of input.
    fn scan_line(&mut self) -> bool {
        loop {
            self.skip_inline_whitespace();
            if self.peek() == Some('#') {
                self.skip_comment();
            }
            if self.at_eof() {
                return false;
            }
            if self.at_line_break() {
                let start = self.pos;
                self.skip_line_break();
                if self.brackets > 0 {
                    continue;
                }
                if self.line_has_tokens {
                    self.tokens
                        .push(Token::new(TokenKind::Newline, self.span(start, self.pos)));
                    self.line_has_tokens = false;
                }
                return true;
            }
            self.scan_token();
        }
    }

    /// Skips spaces, tabs and lone `\r` inside a line.
    fn skip_inline_whitespace(&mut self) {
        loop {
            match self.byte_at(self.pos) {
                Some(b' ' | b'\t') => self.pos += 1,
                Some(b'\r') if !self.at_line_break() => self.pos += 1,
                _ => return,
            }
        }
    }

    /// Skips a `#` comment up to (but not including) the line break.
    fn skip_comment(&mut self) {
        while !self.at_eof() && !self.at_line_break() {
            self.bump();
        }
    }

    /// Emits the trailing `Newline`, `Dedent`s and `Eof`.
    fn finish(mut self) -> Vec<Token> {
        let end = self.src.len();
        let at = self.span(end, end);
        if self.line_has_tokens {
            self.tokens.push(Token::new(TokenKind::Newline, at));
            self.line_has_tokens = false;
        }
        while self.indents.len() > 1 {
            self.indents.pop();
            self.tokens.push(Token::new(TokenKind::Dedent, at));
        }
        self.tokens.push(Token::new(TokenKind::Eof, at));
        self.tokens
    }

    // ----------------------------------------------------------------- tokens

    /// Scans a single token (or reports and skips one invalid character).
    fn scan_token(&mut self) {
        let Some(c) = self.peek() else { return };
        match c {
            'f' if matches!(self.peek_nth(1), Some('"' | '\'')) => self.scan_fstring(),
            c if is_ident_start(c) => self.scan_ident(),
            c if c.is_ascii_digit() => self.scan_number(),
            '"' | '\'' => self.scan_string(),
            '.' if matches!(self.peek_nth(1), Some(d) if d.is_ascii_digit()) => self.scan_number(),
            _ => self.scan_operator(),
        }
    }

    /// Scans an identifier or a keyword.
    fn scan_ident(&mut self) {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if is_ident_continue(c)) {
            self.bump();
        }
        let text = &self.src[start..self.pos];
        let kind = TokenKind::keyword(text).unwrap_or_else(|| TokenKind::Ident(text.to_string()));
        self.emit(kind, start, self.pos);
    }

    /// Scans an operator or a punctuation token, maximal munch first.
    fn scan_operator(&mut self) {
        let start = self.pos;
        let Some(c) = self.bump() else { return };
        let kind = match c {
            '+' => {
                if self.eat('=') {
                    TokenKind::PlusEq
                } else {
                    TokenKind::Plus
                }
            }
            '-' => {
                if self.eat('=') {
                    TokenKind::MinusEq
                } else if self.eat('>') {
                    TokenKind::Arrow
                } else {
                    TokenKind::Minus
                }
            }
            '*' => {
                if self.eat('*') {
                    TokenKind::StarStar
                } else if self.eat('=') {
                    TokenKind::StarEq
                } else {
                    TokenKind::Star
                }
            }
            '/' => {
                if self.eat('/') {
                    TokenKind::SlashSlash
                } else if self.eat('=') {
                    TokenKind::SlashEq
                } else {
                    TokenKind::Slash
                }
            }
            '%' => TokenKind::Percent,
            '=' => {
                if self.eat('=') {
                    TokenKind::EqEq
                } else {
                    TokenKind::Eq
                }
            }
            '!' => {
                if self.eat('=') {
                    TokenKind::BangEq
                } else {
                    self.error(
                        Diagnostic::error("unknown character `!`")
                            .with_label(self.span(start, self.pos), "not a typhoon operator")
                            .with_help("use `not` for logical negation, `!=` for inequality"),
                    );
                    return;
                }
            }
            '<' => {
                if self.eat('=') {
                    TokenKind::LtEq
                } else if self.eat('<') {
                    TokenKind::LtLt
                } else {
                    TokenKind::Lt
                }
            }
            '>' => {
                if self.eat('=') {
                    TokenKind::GtEq
                } else if self.eat('>') {
                    TokenKind::GtGt
                } else {
                    TokenKind::Gt
                }
            }
            ':' => TokenKind::Colon,
            ',' => TokenKind::Comma,
            '.' => TokenKind::Dot,
            ';' => TokenKind::Semi,
            '|' => TokenKind::Pipe,
            '&' => TokenKind::Amp,
            '^' => TokenKind::Caret,
            '~' => TokenKind::Tilde,
            '(' | '[' | '{' => {
                self.brackets += 1;
                match c {
                    '(' => TokenKind::LParen,
                    '[' => TokenKind::LBracket,
                    _ => TokenKind::LBrace,
                }
            }
            ')' | ']' | '}' => {
                self.brackets = self.brackets.saturating_sub(1);
                match c {
                    ')' => TokenKind::RParen,
                    ']' => TokenKind::RBracket,
                    _ => TokenKind::RBrace,
                }
            }
            other => {
                self.error(
                    Diagnostic::error(format!("unknown character {}", quoted(other)))
                        .with_label(self.span(start, self.pos), "not valid in typhoon source"),
                );
                return;
            }
        };
        self.emit(kind, start, self.pos);
    }

    // ---------------------------------------------------------------- numbers

    /// Scans a number literal: `Int` or `Float`.
    fn scan_number(&mut self) {
        let start = self.pos;
        if self.peek() == Some('0') {
            let radix = match self.peek_nth(1) {
                Some('x' | 'X') => Some(16),
                Some('b' | 'B') => Some(2),
                Some('o' | 'O') => Some(8),
                _ => None,
            };
            if let Some(radix) = radix {
                self.pos += 2;
                self.scan_radix_int(start, radix);
                return;
            }
        }
        self.scan_decimal(start);
    }

    /// Scans the digits of a `0x` / `0b` / `0o` literal; the cursor is just
    /// after the prefix, which starts at `start`.
    fn scan_radix_int(&mut self, start: usize, radix: u32) {
        let mut digits = String::new();
        let mut bad: Option<(usize, char)> = None;
        while let Some(c) = self.peek() {
            if c == '_' {
                self.bump();
                continue;
            }
            if !is_ident_continue(c) {
                break;
            }
            let at = self.pos;
            self.bump();
            if c.is_digit(radix) {
                digits.push(c);
            } else if bad.is_none() {
                bad = Some((at, c));
            }
        }
        let end = self.pos;
        let prefix = self.src[start..start + 2].to_string();
        let mut value = 0i64;
        if let Some((at, c)) = bad {
            let span = self.span(at, at + c.len_utf8());
            self.error(
                Diagnostic::error(format!("invalid digit for a base {radix} literal"))
                    .with_label(span, format!("{} is not a base {radix} digit", quoted(c))),
            );
        } else if digits.is_empty() {
            let span = self.span(start, end);
            self.error(
                Diagnostic::error(format!("missing digits after the `{prefix}` base prefix"))
                    .with_label(span, "expected at least one digit here")
                    .with_help(format!("write `{prefix}0` for zero")),
            );
        } else if let Ok(v) = i64::from_str_radix(&digits, radix) {
            value = v;
        } else {
            let span = self.span(start, end);
            self.error(Self::too_large(span));
        }
        self.emit(TokenKind::Int(value), start, end);
    }

    /// The "integer literal is too large" diagnostic.
    fn too_large(span: Span) -> Diagnostic {
        Diagnostic::error("integer literal is too large for `int`")
            .with_label(span, "does not fit in a 64-bit signed integer")
            .with_note("`int` is a 64-bit signed integer, see DESIGN 1.4")
    }

    /// Consumes decimal digits and `_` separators, pushing the digits into
    /// `out`. Returns how many digits were pushed.
    fn take_digits(&mut self, out: &mut String) -> usize {
        let mut count = 0;
        while let Some(c) = self.peek() {
            if c == '_' {
                self.bump();
            } else if c.is_ascii_digit() {
                self.bump();
                out.push(c);
                count += 1;
            } else {
                break;
            }
        }
        count
    }

    /// Scans a decimal int or float starting at `start` (the cursor is on the
    /// first digit, or on a `.` directly followed by a digit).
    fn scan_decimal(&mut self, start: usize) {
        // `norm` collects the literal without `_` separators, in a shape that
        // `str::parse` accepts even after error recovery.
        let mut norm = String::new();
        let mut is_float = false;
        let mut had_error = false;

        if self.peek() == Some('.') {
            is_float = true;
            had_error = true;
            self.bump();
            norm.push('0');
            norm.push('.');
            self.take_digits(&mut norm);
            let span = self.span(start, self.pos);
            let text = &self.src[start..self.pos];
            let help = format!("write `0{text}`");
            self.error(
                Diagnostic::error("floats must have a digit before the decimal point")
                    .with_label(span, "missing a digit before `.`")
                    .with_help(help),
            );
        } else {
            self.take_digits(&mut norm);
            let dot_is_decimal_point = self.peek() == Some('.')
                && !matches!(self.peek_nth(1), Some(c) if is_ident_start(c));
            if dot_is_decimal_point {
                is_float = true;
                self.bump();
                norm.push('.');
                if self.take_digits(&mut norm) == 0 {
                    norm.push('0');
                    had_error = true;
                    let span = self.span(start, self.pos);
                    let text = &self.src[start..self.pos];
                    let help = format!("write `{text}0`");
                    self.error(
                        Diagnostic::error("floats must have a digit after the decimal point")
                            .with_label(span, "missing a digit after `.`")
                            .with_help(help),
                    );
                }
            }
        }

        if matches!(self.peek(), Some('e' | 'E')) {
            let exp_start = self.pos;
            let mut look = self.pos + 1;
            let sign = match self.byte_at(look) {
                Some(b'+') => {
                    look += 1;
                    Some('+')
                }
                Some(b'-') => {
                    look += 1;
                    Some('-')
                }
                _ => None,
            };
            let has_digits = matches!(self.byte_at(look), Some(b) if b.is_ascii_digit());
            self.pos = look;
            is_float = true;
            if has_digits {
                norm.push('e');
                if let Some(sign) = sign {
                    norm.push(sign);
                }
                self.take_digits(&mut norm);
            } else {
                had_error = true;
                let span = self.span(exp_start, self.pos);
                self.error(
                    Diagnostic::error("expected at least one digit in the exponent")
                        .with_label(span, "this exponent has no digits")
                        .with_help("write `1e10` or `1e-3`"),
                );
            }
        }

        // A literal may not be glued to an identifier: `123abc`.
        if matches!(self.peek(), Some(c) if is_ident_continue(c)) {
            let suffix_start = self.pos;
            while matches!(self.peek(), Some(c) if is_ident_continue(c)) {
                self.bump();
            }
            if !had_error {
                let span = self.span(suffix_start, self.pos);
                let suffix = &self.src[suffix_start..self.pos];
                let message = format!("invalid suffix `{suffix}` on a numeric literal");
                self.error(
                    Diagnostic::error(message)
                        .with_label(span, "invalid suffix")
                        .with_help("typhoon has no literal suffixes; separate them with a space"),
                );
            }
        }

        let end = self.pos;
        let kind = if is_float {
            TokenKind::Float(norm.parse::<f64>().unwrap_or(0.0))
        } else {
            match norm.parse::<i64>() {
                Ok(value) => TokenKind::Int(value),
                Err(_) => {
                    let span = self.span(start, end);
                    self.error(Self::too_large(span));
                    TokenKind::Int(0)
                }
            }
        };
        self.emit(kind, start, end);
    }

    // ---------------------------------------------------------------- strings

    /// Scans a `"..."` or `'...'` string literal.
    fn scan_string(&mut self) {
        let start = self.pos;
        let quote = self.bump().expect("caller checked the quote");
        let mut value = String::new();
        loop {
            if self.at_eof() || self.at_line_break() {
                self.unterminated(start, quote, "string literal");
                break;
            }
            match self.peek() {
                Some(c) if c == quote => {
                    self.bump();
                    break;
                }
                Some('\\') => {
                    if !self.scan_escape(&mut value) {
                        self.unterminated(start, quote, "string literal");
                        break;
                    }
                }
                Some(c) => {
                    self.bump();
                    value.push(c);
                }
                None => unreachable!("checked by at_eof"),
            }
        }
        self.emit(TokenKind::Str(value), start, self.pos);
    }

    /// Reports an unterminated string or f-string; `open` is the offset of the
    /// opening quote (of the `f` prefix's quote, not of the `f`).
    fn unterminated(&mut self, open: usize, quote: char, what: &str) {
        let span = self.span(open, open + quote.len_utf8());
        self.error(
            Diagnostic::error(format!("unterminated {what}"))
                .with_label(span, "this string is never closed")
                .with_help(format!("add a closing `{quote}`"))
                .with_note("typhoon string literals may not span multiple lines"),
        );
    }

    /// Decodes one `\...` escape into `out`. Returns `false` if the escape is
    /// cut short by the end of the line or of the input, in which case the
    /// caller reports the string as unterminated.
    fn scan_escape(&mut self, out: &mut String) -> bool {
        let start = self.pos;
        self.bump(); // the backslash
        if self.at_eof() || self.at_line_break() {
            return false;
        }
        let Some(c) = self.bump() else { return false };
        match c {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            '0' => out.push('\0'),
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            '\'' => out.push('\''),
            'x' => self.scan_hex_escape(start, out),
            'u' => self.scan_unicode_escape(start, out),
            other => {
                let span = self.span(start, self.pos);
                self.error(
                    Diagnostic::error(format!(
                        "unknown character escape `\\{}`",
                        other.escape_debug()
                    ))
                    .with_label(span, "unknown escape sequence")
                    .with_help("write `\\\\` for a literal backslash"),
                );
                out.push(other);
            }
        }
        true
    }

    /// Decodes `\xNN`; the cursor is just after the `x`.
    fn scan_hex_escape(&mut self, start: usize, out: &mut String) {
        let mut value = 0u32;
        let mut digits = 0;
        while digits < 2 {
            let Some(c) = self.peek() else { break };
            let Some(d) = c.to_digit(16) else { break };
            self.bump();
            value = value * 16 + d;
            digits += 1;
        }
        if digits == 2 {
            out.push(char::from_u32(value).expect("0x00..=0xff is always a scalar value"));
            return;
        }
        let span = self.span(start, self.pos);
        self.error(
            Diagnostic::error("invalid `\\x` escape")
                .with_label(span, "expected two hexadecimal digits after `\\x`")
                .with_help("write `\\x41` for `A`"),
        );
        if digits == 1 {
            out.push(char::from_u32(value).expect("0x0..=0xf is always a scalar value"));
        }
    }

    /// Decodes `\u{XXXX}`; the cursor is just after the `u`.
    fn scan_unicode_escape(&mut self, start: usize, out: &mut String) {
        if !self.eat('{') {
            let span = self.span(start, self.pos);
            self.error(
                Diagnostic::error("invalid `\\u` escape")
                    .with_label(span, "expected `{` after `\\u`")
                    .with_help("write `\\u{1f600}`"),
            );
            return;
        }
        let mut value: u32 = 0;
        let mut digits = 0;
        let mut too_long = false;
        while let Some(c) = self.peek() {
            let Some(d) = c.to_digit(16) else { break };
            self.bump();
            digits += 1;
            if digits <= 6 {
                value = value * 16 + d;
            } else {
                too_long = true;
            }
        }
        if !self.eat('}') {
            let span = self.span(start, self.pos);
            self.error(
                Diagnostic::error("unterminated `\\u` escape")
                    .with_label(span, "expected `}` to close this escape")
                    .with_help("write `\\u{1f600}`"),
            );
            return;
        }
        let span = self.span(start, self.pos);
        if digits == 0 {
            self.error(
                Diagnostic::error("empty `\\u` escape")
                    .with_label(span, "expected between one and six hexadecimal digits")
                    .with_help("write `\\u{41}` for `A`"),
            );
            return;
        }
        if too_long || digits > 6 {
            self.error(
                Diagnostic::error("overlong `\\u` escape")
                    .with_label(span, "expected at most six hexadecimal digits")
                    .with_note("the largest scalar value is `\\u{10ffff}`"),
            );
            return;
        }
        match char::from_u32(value) {
            Some(c) => out.push(c),
            None => self.error(
                Diagnostic::error("invalid unicode scalar value in `\\u` escape")
                    .with_label(span, "not a valid unicode scalar value")
                    .with_note("values `d800..=dfff` and above `10ffff` are not scalar values"),
            ),
        }
    }

    // -------------------------------------------------------------- f-strings

    /// Scans an `f"..."` / `f'...'` literal into [`RawFStringPart`]s.
    fn scan_fstring(&mut self) {
        let start = self.pos;
        self.bump(); // `f`
        let open = self.pos;
        let quote = self.bump().expect("caller checked the quote");
        let mut parts = Vec::new();
        let mut literal = String::new();
        let mut literal_start = self.pos;
        // Where the trailing literal run ends: the closing quote, the line
        // break, or wherever recovery stopped.
        let content_end;
        loop {
            if self.at_eof() || self.at_line_break() {
                content_end = self.pos;
                self.unterminated(open, quote, "f-string literal");
                break;
            }
            match self.peek() {
                Some(c) if c == quote => {
                    content_end = self.pos;
                    self.bump();
                    break;
                }
                Some('{') if self.peek_nth(1) == Some('{') => {
                    self.pos += 2;
                    literal.push('{');
                }
                Some('}') if self.peek_nth(1) == Some('}') => {
                    self.pos += 2;
                    literal.push('}');
                }
                Some('}') => {
                    let at = self.pos;
                    self.bump();
                    let span = self.span(at, self.pos);
                    self.error(
                        Diagnostic::error("single `}` is not allowed in an f-string")
                            .with_label(span, "unmatched `}`")
                            .with_help("write `}}` for a literal brace"),
                    );
                    literal.push('}');
                }
                Some('{') => {
                    self.push_literal(&mut parts, &mut literal, literal_start, self.pos);
                    let keep_going = self.scan_fstring_hole(&mut parts, quote);
                    literal_start = self.pos;
                    if !keep_going {
                        content_end = self.pos;
                        break;
                    }
                }
                Some('\\') => {
                    if !self.scan_escape(&mut literal) {
                        content_end = self.pos;
                        self.unterminated(open, quote, "f-string literal");
                        break;
                    }
                }
                Some(c) => {
                    self.bump();
                    literal.push(c);
                }
                None => unreachable!("checked by at_eof"),
            }
        }
        self.push_literal(&mut parts, &mut literal, literal_start, content_end);
        self.emit(TokenKind::FString(parts), start, self.pos);
    }

    /// Flushes the accumulated literal text of an f-string, if any. The run
    /// covers `start..end` in the source.
    fn push_literal(
        &self,
        parts: &mut Vec<RawFStringPart>,
        value: &mut String,
        start: usize,
        end: usize,
    ) {
        if value.is_empty() {
            return;
        }
        parts.push(RawFStringPart::Literal {
            value: std::mem::take(value),
            span: self.span(start, end),
        });
    }

    /// Scans one `{expr}` / `{expr:spec}` hole. The cursor is on the `{`.
    /// Returns `false` if the enclosing f-string must be abandoned.
    fn scan_fstring_hole(&mut self, parts: &mut Vec<RawFStringPart>, quote: char) -> bool {
        let open = self.pos;
        self.bump(); // `{`
        let expr_start = self.pos;
        let mut expr_end = None;
        let mut spec_start = None;
        let mut depth = 0u32;
        let close;
        loop {
            if self.at_eof() || self.at_line_break() {
                let span = self.span(open, open + 1);
                self.error(
                    Diagnostic::error("unterminated f-string hole")
                        .with_label(span, "this `{` is never closed")
                        .with_help("add a closing `}`"),
                );
                return false;
            }
            let Some(c) = self.peek() else { return false };
            if c == quote {
                let span = self.span(self.pos, self.pos + c.len_utf8());
                let hole = self.span(open, open + 1);
                self.bump();
                self.error(
                    Diagnostic::error(
                        "nested quotes of the same kind are not supported in f-strings",
                    )
                    .with_label(span, "this quote ends the f-string")
                    .with_secondary(hole, "inside this hole")
                    .with_help("use a different quote style inside the braces"),
                );
                return false;
            }
            match c {
                '"' | '\'' => {
                    if !self.skip_nested_string(c) {
                        continue;
                    }
                }
                '(' | '[' | '{' => {
                    depth += 1;
                    self.bump();
                }
                ')' | ']' => {
                    depth = depth.saturating_sub(1);
                    self.bump();
                }
                '}' => {
                    if depth == 0 {
                        close = self.pos;
                        self.bump();
                        break;
                    }
                    depth -= 1;
                    self.bump();
                }
                ':' if depth == 0 && spec_start.is_none() => {
                    expr_end = Some(self.pos);
                    self.bump();
                    spec_start = Some(self.pos);
                }
                _ => {
                    self.bump();
                }
            }
        }

        let raw_end = expr_end.unwrap_or(close);
        let raw = &self.src[expr_start..raw_end];
        if raw.trim().is_empty() {
            let span = self.span(open, self.pos);
            self.error(
                Diagnostic::error("empty expression in f-string")
                    .with_label(span, "this hole has no expression")
                    .with_help("write an expression between `{` and `}`, or `{{` for a brace"),
            );
            return true;
        }
        // Surrounding whitespace is trimmed off `text` and its span together,
        // so that re-lexing the text does not start with an `Indent`.
        let text = raw.trim();
        let text_start = expr_start + (raw.len() - raw.trim_start().len());
        let text_end = text_start + text.len();
        let (spec, spec_span) = match spec_start {
            Some(at) => (
                Some(self.src[at..close].to_string()),
                Some(self.span(at, close)),
            ),
            None => (None, None),
        };
        parts.push(RawFStringPart::Expr {
            text: text.to_string(),
            span: self.span(text_start, text_end),
            spec,
            spec_span,
            full_span: self.span(open, self.pos),
        });
        true
    }

    /// Skips over a string literal of the *other* quote kind inside an
    /// f-string hole. Returns `false` if it ran into the end of the line, in
    /// which case the caller's loop reports the unterminated hole.
    fn skip_nested_string(&mut self, quote: char) -> bool {
        self.bump(); // opening quote
        loop {
            if self.at_eof() || self.at_line_break() {
                return false;
            }
            match self.peek() {
                Some(c) if c == quote => {
                    self.bump();
                    return true;
                }
                Some('\\') => {
                    self.bump();
                    if self.at_eof() || self.at_line_break() {
                        return false;
                    }
                    self.bump();
                }
                _ => {
                    self.bump();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const F: FileId = FileId(7);

    /// Runs the scanner over `src` and returns the tokens and diagnostics.
    fn run(src: &str) -> (Vec<Token>, Diagnostics) {
        let mut diags = Diagnostics::new();
        let tokens = Scanner::new(F, src, 0, &mut diags).run();
        (tokens, diags)
    }

    /// A scanner over `src` with a fresh diagnostics sink.
    fn scanner<'a>(src: &'a str, diags: &'a mut Diagnostics) -> Scanner<'a> {
        Scanner::new(F, src, 0, diags)
    }

    #[test]
    fn ident_character_classes() {
        assert!(is_ident_start('a') && is_ident_start('Z') && is_ident_start('_'));
        assert!(!is_ident_start('1') && !is_ident_start('é') && !is_ident_start(' '));
        assert!(is_ident_continue('a') && is_ident_continue('9') && is_ident_continue('_'));
        assert!(!is_ident_continue('-') && !is_ident_continue('é'));
    }

    #[test]
    fn quoted_escapes_control_characters() {
        assert_eq!(quoted('@'), "`@`");
        assert_eq!(quoted('é'), "`é`");
        assert_eq!(quoted('\u{7}'), "`\\u{7}`");
    }

    #[test]
    fn cursor_walks_multi_byte_characters() {
        let mut diags = Diagnostics::new();
        let mut scanner = scanner("é1", &mut diags);
        assert_eq!(scanner.peek(), Some('é'));
        assert_eq!(scanner.peek_nth(1), Some('1'));
        assert_eq!(scanner.bump(), Some('é'));
        assert_eq!(scanner.pos, 2, "`é` is two bytes wide");
        assert!(scanner.eat('1'));
        assert!(scanner.at_eof());
        assert_eq!(scanner.bump(), None);
    }

    #[test]
    fn line_break_recognition() {
        let mut diags = Diagnostics::new();

        let mut unix = scanner("\n", &mut diags);
        assert!(unix.at_line_break());
        unix.skip_line_break();
        assert_eq!(unix.pos, 1);

        let mut windows = scanner("\r\n", &mut diags);
        assert!(windows.at_line_break());
        windows.skip_line_break();
        assert_eq!(windows.pos, 2);

        let lone = scanner("\rx", &mut diags);
        assert!(!lone.at_line_break(), "a lone `\\r` is not a line break");
    }

    #[test]
    fn indentation_counts_spaces_and_reports_the_first_tab() {
        let mut diags = Diagnostics::new();
        assert_eq!(scanner("    x", &mut diags).scan_indentation(), (4, None));
        assert_eq!(scanner("x", &mut diags).scan_indentation(), (0, None));
        assert_eq!(
            scanner(" \t x", &mut diags).scan_indentation(),
            (3, Some(1))
        );
        assert_eq!(
            scanner("\t\tx", &mut diags).scan_indentation(),
            (2, Some(0))
        );
        // A lone `\r` is whitespace but does not count as a column.
        assert_eq!(scanner("\r  x", &mut diags).scan_indentation(), (2, None));
        assert!(diags.is_empty(), "scan_indentation itself never reports");
    }

    #[test]
    fn spans_are_shifted_by_the_base_offset() {
        let mut diags = Diagnostics::new();
        let scanner = Scanner::new(F, "abc", 100, &mut diags);
        assert_eq!(scanner.span(1, 2), Span::new(F, 101, 102));
    }

    #[test]
    fn the_base_offset_saturates_instead_of_overflowing() {
        let mut diags = Diagnostics::new();
        let scanner = Scanner::new(F, "abc", u32::MAX, &mut diags);
        assert_eq!(scanner.span(1, 2), Span::new(F, u32::MAX, u32::MAX));
    }

    #[test]
    fn the_indent_stack_never_empties() {
        // Recovery may rewrite the outermost level but must never pop it.
        let (tokens, diags) = run("if a:\n        x\n    y\nz\n");
        assert_eq!(diags.len(), 2, "two mismatched dedents");
        assert_eq!(
            tokens
                .iter()
                .filter(|t| t.kind == TokenKind::Indent)
                .count(),
            tokens
                .iter()
                .filter(|t| t.kind == TokenKind::Dedent)
                .count()
        );
        assert_eq!(tokens.last().map(|t| t.kind.clone()), Some(TokenKind::Eof));
    }

    #[test]
    fn run_produces_a_terminated_stream_for_the_empty_input() {
        let (tokens, diags) = run("");
        assert!(diags.is_empty());
        assert_eq!(tokens, vec![Token::new(TokenKind::Eof, Span::new(F, 0, 0))]);
    }

    #[test]
    fn take_digits_skips_separators() {
        let mut diags = Diagnostics::new();
        let mut scanner = scanner("1_2_3x", &mut diags);
        let mut out = String::new();
        assert_eq!(scanner.take_digits(&mut out), 3);
        assert_eq!(out, "123");
        assert_eq!(scanner.peek(), Some('x'));
    }

    #[test]
    fn skip_nested_string_stops_at_the_line_break() {
        let mut diags = Diagnostics::new();
        let mut closed = scanner("'abc'x", &mut diags);
        assert!(closed.skip_nested_string('\''));
        assert_eq!(closed.peek(), Some('x'));

        let mut open = scanner("'abc\n", &mut diags);
        assert!(!open.skip_nested_string('\''));
        assert!(open.at_line_break());

        let mut escaped = scanner(r"'a\'b'x", &mut diags);
        assert!(escaped.skip_nested_string('\''));
        assert_eq!(escaped.peek(), Some('x'));
    }

    #[test]
    fn every_token_of_a_realistic_line_is_emitted_in_order() {
        let (tokens, diags) = run("total = total + xs[i] * 2\n");
        assert!(diags.is_empty());
        let kinds: Vec<_> = tokens.iter().map(|t| t.kind.clone()).collect();
        assert_eq!(
            kinds,
            vec![
                TokenKind::Ident("total".into()),
                TokenKind::Eq,
                TokenKind::Ident("total".into()),
                TokenKind::Plus,
                TokenKind::Ident("xs".into()),
                TokenKind::LBracket,
                TokenKind::Ident("i".into()),
                TokenKind::RBracket,
                TokenKind::Star,
                TokenKind::Int(2),
                TokenKind::Newline,
                TokenKind::Eof,
            ]
        );
        for pair in tokens.windows(2) {
            assert!(
                pair[0].span.start <= pair[1].span.start,
                "tokens are not ordered"
            );
        }
    }
}
