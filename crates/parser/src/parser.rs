//! The parser state machine: token access, error reporting and recovery.

use typhoon_ast::Ident;
use typhoon_diag::{Diagnostic, Diagnostics, FileId, Span};
use typhoon_lexer::{Token, TokenKind};

/// A saved parser position, used for the speculative parse of generic type
/// arguments (DESIGN 3.9).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Checkpoint {
    pos: usize,
    patches: usize,
}

/// The recursive-descent parser.
pub(crate) struct Parser<'a> {
    pub(crate) file: FileId,
    tokens: Vec<Token>,
    pos: usize,
    /// Tokens replaced by [`Parser::patch`] (used when splitting `>>`), so that
    /// a rewind can restore them.
    patches: Vec<(usize, Token)>,
    /// While non-zero, diagnostics are dropped (speculative parsing).
    silence: u32,
    /// Current nesting depth of expressions and blocks.
    depth: u32,
    /// Set once the nesting limit was hit, so it is reported only once.
    depth_exceeded: bool,
    diags: &'a mut Diagnostics,
}

impl<'a> Parser<'a> {
    /// How deeply expressions and blocks may nest before the parser gives up,
    /// chosen so that recursive descent cannot overflow the stack.
    const MAX_DEPTH: u32 = 128;

    pub(crate) fn new(file: FileId, tokens: Vec<Token>, diags: &'a mut Diagnostics) -> Parser<'a> {
        Parser {
            file,
            tokens,
            pos: 0,
            patches: Vec::new(),
            silence: 0,
            depth: 0,
            depth_exceeded: false,
            diags,
        }
    }

    /// Enters one level of expression or block nesting.
    ///
    /// Returns `false` when the nesting limit is reached; the caller must then
    /// produce an error node instead of recursing. Hitting the limit reports
    /// one diagnostic and silences the rest, because everything after it would
    /// be noise from unwinding the recursion.
    pub(crate) fn enter(&mut self) -> bool {
        if self.depth >= Self::MAX_DEPTH {
            if !self.depth_exceeded {
                self.depth_exceeded = true;
                let span = self.span();
                self.report(
                    Diagnostic::error("this input is nested too deeply")
                        .with_label(span, "the parser gave up here")
                        .with_note(format!(
                            "expressions and blocks may be nested at most {} levels deep",
                            Self::MAX_DEPTH
                        )),
                );
                // Everything reported while unwinding would be noise.
                self.silence += 1;
            }
            return false;
        }
        self.depth += 1;
        true
    }

    /// Leaves one level of nesting, balancing [`Parser::enter`].
    pub(crate) fn leave(&mut self) {
        self.depth -= 1;
    }

    /// Borrows the diagnostic sink, for sub-parsers (f-string holes).
    pub(crate) fn diags(&mut self) -> &mut Diagnostics {
        self.diags
    }

    // ---- token access -------------------------------------------------

    /// The index of the current token, used by progress guards.
    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    pub(crate) fn kind(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    pub(crate) fn kind_at(&self, n: usize) -> &TokenKind {
        let i = (self.pos + n).min(self.tokens.len() - 1);
        &self.tokens[i].kind
    }

    pub(crate) fn span(&self) -> Span {
        self.tokens[self.pos].span
    }

    /// The span of the last consumed token (the current one at the start of
    /// input).
    pub(crate) fn prev_span(&self) -> Span {
        if self.pos == 0 {
            self.tokens[0].span
        } else {
            self.tokens[self.pos - 1].span
        }
    }

    /// Builds a node span reaching from `start` to the last consumed token.
    pub(crate) fn span_from(&self, start: Span) -> Span {
        start.merge(self.prev_span())
    }

    pub(crate) fn at(&self, kind: &TokenKind) -> bool {
        self.kind() == kind
    }

    pub(crate) fn at_eof(&self) -> bool {
        matches!(self.kind(), TokenKind::Eof)
    }

    /// Consumes and returns the current token. At `Eof` the position does not
    /// advance, which guarantees termination of every recovery loop.
    pub(crate) fn bump(&mut self) -> Token {
        let token = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        token
    }

    /// Consumes the current token if it has the given kind.
    pub(crate) fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// Consumes an identifier, or reports `expected <what>`.
    pub(crate) fn expect_ident(&mut self, what: &str) -> Option<Ident> {
        match self.kind() {
            TokenKind::Ident(name) => {
                let name = name.clone();
                let span = self.span();
                self.bump();
                Some(Ident::new(name, span))
            }
            _ => {
                self.expected(what);
                None
            }
        }
    }

    /// Consumes `kind`, or reports `expected <what>, found <token>`.
    pub(crate) fn expect(&mut self, kind: &TokenKind, what: &str) -> bool {
        if self.eat(kind) {
            true
        } else {
            self.expected(what);
            false
        }
    }

    /// Replaces the current token, remembering the old one so that
    /// [`Parser::rewind`] can restore it. Used to split `>>` into two `>`.
    pub(crate) fn patch(&mut self, kind: TokenKind, span: Span) {
        let old = self.tokens[self.pos].clone();
        self.patches.push((self.pos, old));
        self.tokens[self.pos] = Token::new(kind, span);
    }

    // ---- speculation --------------------------------------------------

    pub(crate) fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            pos: self.pos,
            patches: self.patches.len(),
        }
    }

    pub(crate) fn rewind(&mut self, cp: Checkpoint) {
        while self.patches.len() > cp.patches {
            let (index, token) = self.patches.pop().expect("patch stack underflow");
            self.tokens[index] = token;
        }
        self.pos = cp.pos;
    }

    /// Runs `f` without reporting any diagnostic.
    pub(crate) fn speculate<T>(&mut self, f: impl FnOnce(&mut Parser<'a>) -> T) -> T {
        self.silence += 1;
        let result = f(self);
        self.silence -= 1;
        result
    }

    // ---- diagnostics --------------------------------------------------

    pub(crate) fn report(&mut self, diag: Diagnostic) {
        if self.silence == 0 {
            self.diags.push(diag);
        }
    }

    /// The span to point a diagnostic at for the current token.
    ///
    /// Layout tokens (`Newline`, `Indent`, `Dedent`, `Eof`) are collapsed to an
    /// empty span at their start, so that "expected ..., found end of line"
    /// renders a single caret at the end of the line instead of underlining the
    /// line break itself.
    pub(crate) fn error_span(&self) -> Span {
        let span = self.span();
        match self.kind() {
            TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent | TokenKind::Eof => {
                span.shrink_to_start()
            }
            _ => span,
        }
    }

    /// Reports `expected <what>, found <token>` at the current token and
    /// returns its span.
    pub(crate) fn expected(&mut self, what: &str) -> Span {
        let span = self.error_span();
        let found = self.tokens[self.pos].describe();
        self.report(
            Diagnostic::error(format!("expected {what}, found {found}"))
                .with_label(span, format!("expected {what}")),
        );
        span
    }

    /// Like [`Parser::expected`], plus a `help` line.
    pub(crate) fn expected_with_help(&mut self, what: &str, help: &str) -> Span {
        let span = self.error_span();
        let found = self.tokens[self.pos].describe();
        self.report(
            Diagnostic::error(format!("expected {what}, found {found}"))
                .with_label(span, format!("expected {what}"))
                .with_help(help),
        );
        span
    }

    /// Expects `:` at the end of a block header, with a dedicated hint when the
    /// author wrote a brace instead.
    pub(crate) fn expect_colon(&mut self, after: &str) -> bool {
        if self.eat(&TokenKind::Colon) {
            return true;
        }
        if self.at(&TokenKind::LBrace) {
            self.expected_with_help(
                &format!("`:` after {after}"),
                "typhoon uses indentation blocks, not braces",
            );
            // Swallow the brace so the block behind it still parses and the
            // author gets one diagnostic instead of a cascade.
            self.bump();
        } else {
            self.expected(&format!("`:` after {after}"));
        }
        false
    }

    // ---- recovery -----------------------------------------------------

    /// Consumes the newline that ends a simple statement. If something else is
    /// there, reports it once and skips to the end of the line.
    pub(crate) fn expect_newline(&mut self, after: &str) {
        if self.eat(&TokenKind::Newline) {
            return;
        }
        if matches!(self.kind(), TokenKind::Eof | TokenKind::Dedent) {
            return;
        }
        if self.at(&TokenKind::Semi) {
            let span = self.span();
            self.report(
                Diagnostic::error("semicolons are not used in typhoon")
                    .with_label(span, "remove this `;`")
                    .with_help("write each statement on its own line"),
            );
            self.bump();
            self.eat(&TokenKind::Newline);
            return;
        }
        self.expected(&format!("end of line after {after}"));
        self.sync_to_newline();
    }

    /// Skips tokens until just past the end of the current logical line,
    /// staying inside the current block.
    pub(crate) fn sync_to_newline(&mut self) {
        loop {
            match self.kind() {
                TokenKind::Eof | TokenKind::Dedent => return,
                TokenKind::Newline => {
                    self.bump();
                    return;
                }
                TokenKind::Indent => {
                    // A nested block opened on a broken line: skip all of it.
                    self.skip_block();
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    /// Skips a whole `Indent ... Dedent` region, including nested ones.
    pub(crate) fn skip_block(&mut self) {
        if !self.eat(&TokenKind::Indent) {
            return;
        }
        let mut depth = 1usize;
        while depth > 0 {
            match self.kind() {
                TokenKind::Eof => return,
                TokenKind::Indent => {
                    depth += 1;
                    self.bump();
                }
                TokenKind::Dedent => {
                    depth -= 1;
                    self.bump();
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    /// Skips forward to the next plausible top-level item: a `fn` / `class` /
    /// `NAME :` at the start of a line and at nesting depth zero.
    pub(crate) fn sync_to_item(&mut self) {
        let mut depth = 0i32;
        loop {
            match self.kind() {
                TokenKind::Eof => return,
                TokenKind::Indent => {
                    depth += 1;
                    self.bump();
                }
                TokenKind::Dedent => {
                    depth -= 1;
                    self.bump();
                }
                TokenKind::Fn | TokenKind::Class if depth <= 0 && self.at_line_start() => return,
                TokenKind::Ident(_)
                    if depth <= 0
                        && self.at_line_start()
                        && matches!(self.kind_at(1), TokenKind::Colon) =>
                {
                    return;
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    /// Whether the current token is the first one of its logical line.
    pub(crate) fn at_line_start(&self) -> bool {
        self.pos == 0
            || matches!(
                self.tokens[self.pos - 1].kind,
                TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent
            )
    }

    /// Skips blank logical lines.
    pub(crate) fn skip_newlines(&mut self) {
        while self.eat(&TokenKind::Newline) {}
    }
}
