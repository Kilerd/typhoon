//! Statement and block parsing (DESIGN 3.4, 3.5).

use typhoon_ast::{Block, ElifBranch, Expr, ExprKind, Stmt, StmtKind};
use typhoon_diag::{Diagnostic, Span};
use typhoon_lexer::TokenKind;

use crate::parser::Parser;

impl Parser<'_> {
    /// Parses `':' NEWLINE INDENT stmt+ DEDENT`, the body of a block header.
    ///
    /// A `{` where the `:` should be gets one dedicated diagnostic and the
    /// whole braced region is skipped, so that brace-style code produces a
    /// single error instead of a cascade.
    pub(crate) fn parse_body(&mut self, header: &str) -> Block {
        if self.at(&TokenKind::LBrace) {
            let span = self.span();
            self.report(
                Diagnostic::error(format!("expected `:` after {header}, found `{{`"))
                    .with_label(span, format!("expected `:` after {header}"))
                    .with_help("typhoon uses indentation blocks, not braces"),
            );
            return self.skip_braced_body();
        }
        self.expect_colon(header);
        self.parse_block(header)
    }

    /// Consumes a balanced `{ ... }` region after a brace-style block header
    /// and yields an empty block in its place.
    fn skip_braced_body(&mut self) -> Block {
        let start = self.span();
        let mut depth = 0usize;
        loop {
            match self.kind() {
                TokenKind::LBrace => {
                    depth += 1;
                    self.bump();
                }
                TokenKind::RBrace => {
                    self.bump();
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                TokenKind::Eof => break,
                _ => {
                    self.bump();
                }
            }
        }
        let span = start.merge(self.prev_span());
        self.eat(&TokenKind::Newline);
        Block {
            stmts: Vec::new(),
            span,
        }
    }

    /// Parses `NEWLINE INDENT stmt+ DEDENT` after a block header.
    pub(crate) fn parse_block(&mut self, header: &str) -> Block {
        if !self.enter() {
            let span = self.span();
            self.skip_block();
            return Block {
                stmts: Vec::new(),
                span,
            };
        }
        let block = self.parse_block_inner(header);
        self.leave();
        block
    }

    fn parse_block_inner(&mut self, header: &str) -> Block {
        if !self.eat(&TokenKind::Newline) && !self.at(&TokenKind::Indent) {
            self.expected(&format!("end of line after {header}"));
            self.sync_to_newline();
        }
        if !self.eat(&TokenKind::Indent) {
            let span = self.span();
            self.report(
                Diagnostic::error(format!("expected an indented block after {header}"))
                    .with_label(span, "expected an indented block")
                    .with_help("indent the body of the block by 4 spaces"),
            );
            return Block {
                stmts: Vec::new(),
                span,
            };
        }
        let start = self.span();
        let mut stmts = Vec::new();
        loop {
            self.skip_newlines();
            if matches!(self.kind(), TokenKind::Dedent | TokenKind::Eof) {
                break;
            }
            let before = self.pos();
            if let Some(stmt) = self.parse_stmt() {
                stmts.push(stmt);
            }
            if self.pos() == before {
                self.bump();
            }
        }
        // The block ends at its last statement, not at the newline or the
        // dedent that follows it.
        let end = stmts.last().map(|stmt| stmt.span).unwrap_or(start);
        self.eat(&TokenKind::Dedent);
        Block {
            stmts,
            span: start.merge(end),
        }
    }

    /// Parses one statement. Returns `None` when the input only produced an
    /// error that was recovered from.
    pub(crate) fn parse_stmt(&mut self) -> Option<Stmt> {
        let start = self.span();
        match self.kind() {
            TokenKind::Pass => {
                self.bump();
                let span = self.span_from(start);
                self.expect_newline("`pass`");
                Some(Stmt {
                    kind: StmtKind::Pass,
                    span,
                })
            }
            TokenKind::Break => {
                self.bump();
                let span = self.span_from(start);
                self.expect_newline("`break`");
                Some(Stmt {
                    kind: StmtKind::Break,
                    span,
                })
            }
            TokenKind::Continue => {
                self.bump();
                let span = self.span_from(start);
                self.expect_newline("`continue`");
                Some(Stmt {
                    kind: StmtKind::Continue,
                    span,
                })
            }
            TokenKind::Return => {
                self.bump();
                let value = if matches!(
                    self.kind(),
                    TokenKind::Newline | TokenKind::Dedent | TokenKind::Eof
                ) {
                    None
                } else {
                    Some(self.parse_expr())
                };
                let span = self.span_from(start);
                self.expect_newline("`return`");
                Some(Stmt {
                    kind: StmtKind::Return(value),
                    span,
                })
            }
            TokenKind::If => self.parse_if(),
            TokenKind::While => self.parse_while(),
            TokenKind::For => self.parse_for(),
            TokenKind::Elif => {
                let span = self.span();
                self.report(
                    Diagnostic::error("`elif` without a matching `if`")
                        .with_label(span, "no `if` block precedes this `elif`")
                        .with_help("an `elif` must directly follow the body of an `if`"),
                );
                self.recover_from_stray_header();
                None
            }
            TokenKind::Else => {
                let span = self.span();
                self.report(
                    Diagnostic::error("`else` without a matching `if`")
                        .with_label(span, "no `if` block precedes this `else`")
                        .with_help("an `else` must directly follow the body of an `if` or `elif`"),
                );
                self.recover_from_stray_header();
                None
            }
            TokenKind::Fn => {
                let span = self.span();
                self.report(
                    Diagnostic::error("nested functions are not supported")
                        .with_label(span, "a `fn` cannot be declared inside another function")
                        .with_help("declare the function at the top level"),
                );
                self.recover_from_stray_header();
                None
            }
            TokenKind::Class => {
                let span = self.span();
                self.report(
                    Diagnostic::error("nested classes are not supported")
                        .with_label(span, "a `class` cannot be declared inside a function")
                        .with_help("declare the class at the top level"),
                );
                self.recover_from_stray_header();
                None
            }
            TokenKind::Import
            | TokenKind::From
            | TokenKind::Try
            | TokenKind::Except
            | TokenKind::Finally
            | TokenKind::Raise
            | TokenKind::Match
            | TokenKind::Case => {
                self.unsupported_keyword();
                None
            }
            TokenKind::Indent => {
                let span = self.span();
                self.report(
                    Diagnostic::error("unexpected indentation")
                        .with_label(span, "this line is indented deeper than the block it is in"),
                );
                self.skip_block();
                None
            }
            _ => {
                let expr = self.parse_expr();
                self.parse_stmt_after_expr(start, expr)
            }
        }
    }

    /// After an expression at statement position: assignment, annotated
    /// declaration or a plain expression statement.
    fn parse_stmt_after_expr(&mut self, start: Span, expr: Expr) -> Option<Stmt> {
        match self.kind() {
            TokenKind::Colon => {
                self.bump();
                let ty = self.parse_type();
                let value = if self.eat(&TokenKind::Eq) {
                    Some(self.parse_expr())
                } else {
                    None
                };
                let span = self.span_from(start);
                self.expect_newline("a variable declaration");
                match expr.kind {
                    ExprKind::Name(name) => {
                        if value.is_none() {
                            self.report(
                                Diagnostic::error(
                                    "a local variable declaration must have an initializer",
                                )
                                .with_label(span, "no value is assigned here")
                                .with_help("write `x: T = value`"),
                            );
                        }
                        Some(Stmt {
                            kind: StmtKind::AnnAssign { name, ty, value },
                            span,
                        })
                    }
                    kind => {
                        self.report(
                            Diagnostic::error("invalid annotation target").with_label(
                                expr.span,
                                "only a simple variable name can be annotated",
                            ),
                        );
                        Some(Stmt {
                            kind: StmtKind::Expr(Expr::new(kind, expr.span)),
                            span,
                        })
                    }
                }
            }
            TokenKind::Eq => {
                self.bump();
                let value = self.parse_expr();
                let span = self.span_from(start);
                self.expect_newline("an assignment");
                if !expr.is_assign_target() {
                    self.report(
                        Diagnostic::error("invalid assignment target")
                            .with_label(expr.span, "cannot assign to this expression")
                            .with_help(
                                "assign to a variable, an attribute (`p.x`) or an index (`xs[0]`)",
                            ),
                    );
                }
                Some(Stmt {
                    kind: StmtKind::Assign {
                        target: expr,
                        value,
                    },
                    span,
                })
            }
            TokenKind::PlusEq | TokenKind::MinusEq | TokenKind::StarEq | TokenKind::SlashEq => {
                let op_span = self.span();
                let op = self.kind().fixed_text().unwrap_or("+=");
                let name = match &expr.kind {
                    ExprKind::Name(name) => name.name.clone(),
                    _ => "x".to_string(),
                };
                let plain = op.trim_end_matches('=');
                self.report(
                    Diagnostic::error("augmented assignment is not supported yet")
                        .with_label(op_span, format!("`{op}` is not supported"))
                        .with_help(format!("write `{name} = {name} {plain} ...` instead")),
                );
                self.bump();
                let _ = self.parse_expr();
                let span = self.span_from(start);
                self.expect_newline("an assignment");
                Some(Stmt {
                    kind: StmtKind::Expr(expr),
                    span,
                })
            }
            _ => {
                let span = self.span_from(start);
                self.expect_newline("an expression statement");
                Some(Stmt {
                    kind: StmtKind::Expr(expr),
                    span,
                })
            }
        }
    }

    fn parse_if(&mut self) -> Option<Stmt> {
        let start = self.span();
        self.bump();
        let cond = self.parse_expr();
        let then = self.parse_body("the `if` condition");
        let mut elifs = Vec::new();
        let mut end = then.span;
        while self.at(&TokenKind::Elif) {
            let elif_start = self.span();
            self.bump();
            let cond = self.parse_expr();
            let body = self.parse_body("the `elif` condition");
            end = body.span;
            elifs.push(ElifBranch {
                cond,
                body,
                span: elif_start.merge(end),
            });
        }
        let else_ = if self.at(&TokenKind::Else) {
            self.bump();
            let body = self.parse_body("`else`");
            end = body.span;
            Some(body)
        } else {
            None
        };
        Some(Stmt {
            kind: StmtKind::If {
                cond,
                then,
                elifs,
                else_,
            },
            span: start.merge(end),
        })
    }

    fn parse_while(&mut self) -> Option<Stmt> {
        let start = self.span();
        self.bump();
        let cond = self.parse_expr();
        let body = self.parse_body("the `while` condition");
        let span = start.merge(body.span);
        Some(Stmt {
            kind: StmtKind::While { cond, body },
            span,
        })
    }

    fn parse_for(&mut self) -> Option<Stmt> {
        let start = self.span();
        self.bump();
        let Some(var) = self.expect_ident("a loop variable name") else {
            self.recover_from_stray_header();
            return None;
        };
        if !self.expect(&TokenKind::In, "`in` after the loop variable") {
            self.sync_to_newline();
            if self.at(&TokenKind::Indent) {
                self.skip_block();
            }
            return None;
        }
        let iter = self.parse_expr();
        let body = self.parse_body("the `for` header");
        let span = start.merge(body.span);
        Some(Stmt {
            kind: StmtKind::For { var, iter, body },
            span,
        })
    }

    /// Skips a `header:` line plus the block that follows it, after the header
    /// itself has been rejected.
    pub(crate) fn recover_from_stray_header(&mut self) {
        self.sync_to_newline();
        if self.at(&TokenKind::Indent) {
            self.skip_block();
        }
    }

    /// Reports a keyword that is reserved but not implemented yet.
    pub(crate) fn unsupported_keyword(&mut self) {
        let span = self.span();
        let text = self.kind().fixed_text().unwrap_or("this keyword");
        let note = match self.kind() {
            TokenKind::Match | TokenKind::Case => {
                "pattern matching is an open question, see DESIGN.md section 9"
            }
            TokenKind::Import | TokenKind::From => {
                "modules are planned for milestone M4, see DESIGN.md section 3.10"
            }
            _ => "exceptions are planned for milestone M4, see DESIGN.md section 3.10",
        };
        self.report(
            Diagnostic::error(format!("`{text}` is not supported yet"))
                .with_label(span, format!("`{text}` is reserved but has no meaning yet"))
                .with_note(note),
        );
        self.recover_from_stray_header();
    }
}
