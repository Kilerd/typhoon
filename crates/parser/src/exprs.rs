//! Expression parsing: a precedence-climbing (Pratt style) parser following
//! the table in DESIGN 3.6.
//!
//! ```text
//! or < and < not < comparison/in/is < | < ^ < & < << >> < + - < * / // % <
//! unary - + ~ < ** < call/index/attribute
//! ```
//!
//! `**` is right associative and binds tighter than a unary operator on its
//! left, so `-a ** b` parses as `-(a ** b)`. Parentheses do not produce a node
//! of their own: `(a + b)` is just the `a + b` node.

use typhoon_ast::{
    Arg, BinOp, BoolOp, CmpOp, CompareTail, Expr, ExprKind, FStringPart, Ident, TypeExpr, UnaryOp,
};
use typhoon_diag::{Diagnostic, Span};
use typhoon_lexer::{RawFStringPart, TokenKind};

use crate::parser::Parser;

impl<'a> Parser<'a> {
    /// Parses a full expression.
    pub(crate) fn parse_expr(&mut self) -> Expr {
        if !self.enter() {
            return Expr::new(ExprKind::Error, self.span());
        }
        let expr = self.parse_or();
        self.leave();
        expr
    }

    fn parse_or(&mut self) -> Expr {
        let mut lhs = self.parse_and();
        while self.at(&TokenKind::Or) {
            let op_span = self.span();
            self.bump();
            let rhs = self.parse_and();
            lhs = bool_op(BoolOp::Or, op_span, lhs, rhs);
        }
        lhs
    }

    fn parse_and(&mut self) -> Expr {
        let mut lhs = self.parse_not();
        while self.at(&TokenKind::And) {
            let op_span = self.span();
            self.bump();
            let rhs = self.parse_not();
            lhs = bool_op(BoolOp::And, op_span, lhs, rhs);
        }
        lhs
    }

    fn parse_not(&mut self) -> Expr {
        if self.at(&TokenKind::Not) && !matches!(self.kind_at(1), TokenKind::In) {
            let op_span = self.span();
            self.bump();
            let expr = self.parse_not();
            let span = op_span.merge(expr.span);
            return Expr::new(
                ExprKind::Unary {
                    op: UnaryOp::Not,
                    op_span,
                    expr: Box::new(expr),
                },
                span,
            );
        }
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Expr {
        let left = self.parse_bit_or();
        let mut tail = Vec::new();
        loop {
            let start = self.span();
            let op = match self.kind() {
                TokenKind::EqEq => CmpOp::Eq,
                TokenKind::BangEq => CmpOp::Ne,
                TokenKind::Lt => CmpOp::Lt,
                TokenKind::LtEq => CmpOp::Le,
                TokenKind::Gt => CmpOp::Gt,
                TokenKind::GtEq => CmpOp::Ge,
                TokenKind::In => CmpOp::In,
                TokenKind::Is => CmpOp::Is,
                TokenKind::Not if matches!(self.kind_at(1), TokenKind::In) => CmpOp::NotIn,
                _ => break,
            };
            self.bump();
            // `is not` and `not in` are two tokens.
            let (op, op_span) = match op {
                CmpOp::Is if self.at(&TokenKind::Not) => {
                    let span = start.merge(self.span());
                    self.bump();
                    (CmpOp::IsNot, span)
                }
                CmpOp::NotIn => {
                    let span = start.merge(self.span());
                    self.bump();
                    (CmpOp::NotIn, span)
                }
                op => (op, start),
            };
            let rhs = self.parse_bit_or();
            tail.push(CompareTail { op, op_span, rhs });
        }
        if tail.is_empty() {
            return left;
        }
        let span = left.span.merge(tail[tail.len() - 1].rhs.span);
        Expr::new(
            ExprKind::Compare {
                left: Box::new(left),
                tail,
            },
            span,
        )
    }

    fn parse_bit_or(&mut self) -> Expr {
        let mut lhs = self.parse_bit_xor();
        while self.at(&TokenKind::Pipe) {
            let op_span = self.span();
            self.bump();
            let rhs = self.parse_bit_xor();
            lhs = bin_op(BinOp::BitOr, op_span, lhs, rhs);
        }
        lhs
    }

    fn parse_bit_xor(&mut self) -> Expr {
        let mut lhs = self.parse_bit_and();
        while self.at(&TokenKind::Caret) {
            let op_span = self.span();
            self.bump();
            let rhs = self.parse_bit_and();
            lhs = bin_op(BinOp::BitXor, op_span, lhs, rhs);
        }
        lhs
    }

    fn parse_bit_and(&mut self) -> Expr {
        let mut lhs = self.parse_shift();
        while self.at(&TokenKind::Amp) {
            let op_span = self.span();
            self.bump();
            let rhs = self.parse_shift();
            lhs = bin_op(BinOp::BitAnd, op_span, lhs, rhs);
        }
        lhs
    }

    fn parse_shift(&mut self) -> Expr {
        let mut lhs = self.parse_additive();
        loop {
            let op = match self.kind() {
                TokenKind::LtLt => BinOp::Shl,
                TokenKind::GtGt => BinOp::Shr,
                _ => break,
            };
            let op_span = self.span();
            self.bump();
            let rhs = self.parse_additive();
            lhs = bin_op(op, op_span, lhs, rhs);
        }
        lhs
    }

    fn parse_additive(&mut self) -> Expr {
        let mut lhs = self.parse_multiplicative();
        loop {
            let op = match self.kind() {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                _ => break,
            };
            let op_span = self.span();
            self.bump();
            let rhs = self.parse_multiplicative();
            lhs = bin_op(op, op_span, lhs, rhs);
        }
        lhs
    }

    fn parse_multiplicative(&mut self) -> Expr {
        let mut lhs = self.parse_unary();
        loop {
            let op = match self.kind() {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                TokenKind::SlashSlash => BinOp::FloorDiv,
                TokenKind::Percent => BinOp::Mod,
                _ => break,
            };
            let op_span = self.span();
            self.bump();
            let rhs = self.parse_unary();
            lhs = bin_op(op, op_span, lhs, rhs);
        }
        lhs
    }

    fn parse_unary(&mut self) -> Expr {
        let op = match self.kind() {
            TokenKind::Minus => UnaryOp::Neg,
            TokenKind::Plus => UnaryOp::Pos,
            TokenKind::Tilde => UnaryOp::BitNot,
            _ => return self.parse_power(),
        };
        let op_span = self.span();
        self.bump();
        let expr = self.parse_unary();
        let span = op_span.merge(expr.span);
        Expr::new(
            ExprKind::Unary {
                op,
                op_span,
                expr: Box::new(expr),
            },
            span,
        )
    }

    /// `**` binds tighter than a unary operator on its left and is right
    /// associative: `-a ** b` is `-(a ** b)` and `2 ** 3 ** 2` is
    /// `2 ** (3 ** 2)`.
    fn parse_power(&mut self) -> Expr {
        let base = self.parse_postfix();
        if !self.at(&TokenKind::StarStar) {
            return base;
        }
        let op_span = self.span();
        self.bump();
        let rhs = self.parse_unary();
        bin_op(BinOp::Pow, op_span, base, rhs)
    }

    fn parse_postfix(&mut self) -> Expr {
        let mut expr = self.parse_primary();
        loop {
            match self.kind() {
                TokenKind::Dot => {
                    self.bump();
                    let Some(attr) = self.expect_ident("an attribute name") else {
                        break;
                    };
                    let span = expr.span.merge(attr.span);
                    expr = Expr::new(
                        ExprKind::Attribute {
                            base: Box::new(expr),
                            attr,
                        },
                        span,
                    );
                }
                TokenKind::LBracket => {
                    self.bump();
                    let index = self.parse_expr();
                    self.expect(&TokenKind::RBracket, "`]`");
                    let span = self.span_from(expr.span);
                    expr = Expr::new(
                        ExprKind::Index {
                            base: Box::new(expr),
                            index: Box::new(index),
                        },
                        span,
                    );
                }
                TokenKind::LParen => {
                    let args = self.parse_call_args();
                    let span = self.span_from(expr.span);
                    expr = Expr::new(
                        ExprKind::Call {
                            callee: Box::new(expr),
                            type_args: None,
                            args,
                        },
                        span,
                    );
                }
                // DESIGN 3.9: `Name <` may open an explicit instantiation.
                TokenKind::Lt
                    if matches!(expr.kind, ExprKind::Name(_) | ExprKind::Attribute { .. }) =>
                {
                    let Some(type_args) = self.try_type_args() else {
                        break;
                    };
                    let args = self.parse_call_args();
                    let span = self.span_from(expr.span);
                    expr = Expr::new(
                        ExprKind::Call {
                            callee: Box::new(expr),
                            type_args: Some(type_args),
                            args,
                        },
                        span,
                    );
                }
                _ => break,
            }
        }
        expr
    }

    /// Speculatively parses `<T, U>` in expression context (DESIGN 3.9 point 3,
    /// the C# rule): it is a type-argument list only if the whole list parses as
    /// types and the matching `>` is immediately followed by `(`. Otherwise the
    /// parser rewinds and `<` stays a comparison operator.
    fn try_type_args(&mut self) -> Option<Vec<TypeExpr>> {
        let cp = self.checkpoint();
        let args = self.speculate(|p| {
            if !p.eat(&TokenKind::Lt) {
                return None;
            }
            let mut args = Vec::new();
            loop {
                if matches!(p.kind(), TokenKind::Eof | TokenKind::Newline) {
                    return None;
                }
                args.push(p.try_parse_type()?);
                if p.eat(&TokenKind::Comma) {
                    if p.eat_type_args_close() {
                        break;
                    }
                    continue;
                }
                if p.eat_type_args_close() {
                    break;
                }
                return None;
            }
            if args.is_empty() || !p.at(&TokenKind::LParen) {
                return None;
            }
            Some(args)
        });
        if args.is_none() {
            self.rewind(cp);
        }
        args
    }

    /// Parses `(a, b=1)`, including keyword arguments and a trailing comma.
    fn parse_call_args(&mut self) -> Vec<Arg> {
        let mut args = Vec::new();
        if !self.eat(&TokenKind::LParen) {
            return args;
        }
        let mut keyword_span: Option<Span> = None;
        loop {
            if matches!(
                self.kind(),
                TokenKind::RParen | TokenKind::Eof | TokenKind::Newline
            ) {
                break;
            }
            let before = self.pos();
            let start = self.span();
            let is_keyword = matches!(self.kind(), TokenKind::Ident(_))
                && matches!(self.kind_at(1), TokenKind::Eq);
            let arg = if is_keyword {
                let name = self
                    .expect_ident("an argument name")
                    .expect("checked above");
                self.bump(); // `=`
                let value = self.parse_expr();
                keyword_span = Some(name.span);
                Arg {
                    name: Some(name),
                    value,
                    span: self.span_from(start),
                }
            } else {
                let value = self.parse_expr();
                let span = self.span_from(start);
                if let Some(keyword_span) = keyword_span {
                    self.report(
                        Diagnostic::error("positional argument follows keyword argument")
                            .with_label(span, "positional argument")
                            .with_secondary(keyword_span, "keyword argument used here")
                            .with_help("move positional arguments before keyword arguments"),
                    );
                }
                Arg {
                    name: None,
                    value,
                    span,
                }
            };
            args.push(arg);
            if !self.eat(&TokenKind::Comma) {
                break;
            }
            if self.pos() == before {
                self.bump();
            }
        }
        self.expect(&TokenKind::RParen, "`)`");
        args
    }

    fn parse_primary(&mut self) -> Expr {
        let span = self.span();
        match self.kind().clone() {
            TokenKind::Int(value) => {
                self.bump();
                Expr::new(ExprKind::Int(value), span)
            }
            TokenKind::Float(value) => {
                self.bump();
                Expr::new(ExprKind::Float(value), span)
            }
            TokenKind::Str(value) => {
                self.bump();
                Expr::new(ExprKind::Str(value), span)
            }
            TokenKind::FString(parts) => {
                self.bump();
                let parts = self.parse_fstring_parts(parts);
                Expr::new(ExprKind::FString(parts), span)
            }
            TokenKind::True => {
                self.bump();
                Expr::new(ExprKind::Bool(true), span)
            }
            TokenKind::False => {
                self.bump();
                Expr::new(ExprKind::Bool(false), span)
            }
            TokenKind::None => {
                self.bump();
                Expr::new(ExprKind::None, span)
            }
            TokenKind::Ident(name) => {
                self.bump();
                Expr::new(ExprKind::Name(Ident::new(name, span)), span)
            }
            TokenKind::LParen => self.parse_paren_or_tuple(),
            TokenKind::LBracket => self.parse_list(),
            TokenKind::LBrace => self.parse_dict_or_set(),
            _ => {
                self.expected("an expression");
                Expr::new(ExprKind::Error, span)
            }
        }
    }

    /// `(e)` yields `e` itself; `(a, b)` and `()` yield a tuple.
    fn parse_paren_or_tuple(&mut self) -> Expr {
        let start = self.span();
        self.bump();
        if self.at(&TokenKind::RParen) {
            self.bump();
            return Expr::new(ExprKind::Tuple(Vec::new()), self.span_from(start));
        }
        let first = self.parse_expr();
        if !self.at(&TokenKind::Comma) {
            self.expect(&TokenKind::RParen, "`)`");
            return first;
        }
        let mut items = vec![first];
        while self.eat(&TokenKind::Comma) {
            if self.at(&TokenKind::RParen) {
                break;
            }
            let before = self.pos();
            items.push(self.parse_expr());
            if self.pos() == before {
                self.bump();
            }
        }
        self.expect(&TokenKind::RParen, "`)`");
        Expr::new(ExprKind::Tuple(items), self.span_from(start))
    }

    fn parse_list(&mut self) -> Expr {
        let start = self.span();
        self.bump();
        let items = self.parse_comma_list(&TokenKind::RBracket);
        self.expect(&TokenKind::RBracket, "`]`");
        Expr::new(ExprKind::List(items), self.span_from(start))
    }

    fn parse_dict_or_set(&mut self) -> Expr {
        let start = self.span();
        self.bump();
        // DESIGN 3.7: an empty `{}` is an empty dict; an empty set is `set()`.
        if self.eat(&TokenKind::RBrace) {
            return Expr::new(ExprKind::Dict(Vec::new()), self.span_from(start));
        }
        let first = self.parse_expr();
        if self.eat(&TokenKind::Colon) {
            let value = self.parse_expr();
            let mut entries = vec![(first, value)];
            while self.eat(&TokenKind::Comma) {
                if self.at(&TokenKind::RBrace) || self.at_eof() {
                    break;
                }
                let before = self.pos();
                let key = self.parse_expr();
                self.expect(&TokenKind::Colon, "`:` after a dict key");
                let value = self.parse_expr();
                entries.push((key, value));
                if self.pos() == before {
                    self.bump();
                }
            }
            self.expect(&TokenKind::RBrace, "`}`");
            return Expr::new(ExprKind::Dict(entries), self.span_from(start));
        }
        let mut items = vec![first];
        while self.eat(&TokenKind::Comma) {
            if self.at(&TokenKind::RBrace) || self.at_eof() {
                break;
            }
            let before = self.pos();
            items.push(self.parse_expr());
            if self.pos() == before {
                self.bump();
            }
        }
        self.expect(&TokenKind::RBrace, "`}`");
        Expr::new(ExprKind::Set(items), self.span_from(start))
    }

    /// Comma separated expressions with an optional trailing comma, stopping
    /// before `close`.
    fn parse_comma_list(&mut self, close: &TokenKind) -> Vec<Expr> {
        let mut items = Vec::new();
        loop {
            if self.at(close) || matches!(self.kind(), TokenKind::Eof | TokenKind::Newline) {
                break;
            }
            let before = self.pos();
            items.push(self.parse_expr());
            if !self.eat(&TokenKind::Comma) {
                break;
            }
            if self.pos() == before {
                self.bump();
            }
        }
        items
    }

    /// Turns the lexer's raw f-string parts into AST parts, sub-parsing every
    /// `{...}` hole with spans that point back into the original file.
    fn parse_fstring_parts(&mut self, raw: Vec<RawFStringPart>) -> Vec<FStringPart> {
        let mut parts = Vec::with_capacity(raw.len());
        for part in raw {
            match part {
                RawFStringPart::Literal { value, span } => {
                    parts.push(FStringPart::Literal { value, span });
                }
                RawFStringPart::Expr {
                    text,
                    span,
                    spec,
                    spec_span,
                    full_span,
                } => {
                    let expr = self.parse_fragment(&text, span);
                    parts.push(FStringPart::Expr {
                        expr: Box::new(expr),
                        spec,
                        spec_span,
                        span: full_span,
                    });
                }
            }
        }
        parts
    }

    /// Parses `text` (the source slice of `span`) as a standalone expression.
    fn parse_fragment(&mut self, text: &str, span: Span) -> Expr {
        let file = self.file;
        let tokens = typhoon_lexer::tokenize_fragment(file, text, span.start, self.diags());
        let mut sub = Parser::new(file, tokens, self.diags());
        let expr = sub.parse_expr();
        if !matches!(sub.kind(), TokenKind::Newline | TokenKind::Eof) {
            sub.expected("the end of the f-string expression");
        }
        expr
    }
}

fn bin_op(op: BinOp, op_span: Span, lhs: Expr, rhs: Expr) -> Expr {
    let span = lhs.span.merge(rhs.span);
    Expr::new(
        ExprKind::Binary {
            op,
            op_span,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
        span,
    )
}

fn bool_op(op: BoolOp, op_span: Span, lhs: Expr, rhs: Expr) -> Expr {
    let span = lhs.span.merge(rhs.span);
    Expr::new(
        ExprKind::BoolOp {
            op,
            op_span,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
        span,
    )
}
