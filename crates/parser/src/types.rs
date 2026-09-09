//! Parsing of type expressions (DESIGN 3.9, 3.10, 4.1).
//!
//! In type context a `<` *always* opens a type-argument list, so no
//! speculation is needed here; the ambiguity only exists in expressions.

use typhoon_ast::{Ident, TypeExpr, TypeExprKind};
use typhoon_diag::Span;
use typhoon_lexer::TokenKind;

use crate::parser::Parser;

impl Parser<'_> {
    /// Parses a type, reporting `expected a type` and returning an
    /// [`TypeExprKind::Error`] node when the tokens are not a type.
    pub(crate) fn parse_type(&mut self) -> TypeExpr {
        let cp = self.checkpoint();
        match self.try_parse_type() {
            Some(ty) => ty,
            None => {
                self.rewind(cp);
                let span = self.span();
                self.expected("a type");
                TypeExpr::new(TypeExprKind::Error, span)
            }
        }
    }

    /// Parses a type, returning `None` (with tokens possibly consumed, so the
    /// caller must rewind) when the tokens do not form a type.
    pub(crate) fn try_parse_type(&mut self) -> Option<TypeExpr> {
        let first = self.try_parse_type_atom()?;
        if !self.at(&TokenKind::Pipe) {
            return Some(first);
        }
        let mut members = vec![first];
        while self.eat(&TokenKind::Pipe) {
            members.push(self.try_parse_type_atom()?);
        }
        let span = members[0].span.merge(members[members.len() - 1].span);
        Some(TypeExpr::new(TypeExprKind::Union(members), span))
    }

    fn try_parse_type_atom(&mut self) -> Option<TypeExpr> {
        let start = self.span();
        match self.kind().clone() {
            TokenKind::None => {
                self.bump();
                Some(TypeExpr::new(TypeExprKind::None, start))
            }
            TokenKind::Ident(name) => {
                self.bump();
                let name = Ident::new(name, start);
                let mut args = Vec::new();
                if self.eat(&TokenKind::Lt) {
                    loop {
                        args.push(self.try_parse_type()?);
                        if self.eat(&TokenKind::Comma) {
                            // A trailing comma is allowed: `list<int,>`.
                            if self.eat_type_args_close() {
                                break;
                            }
                            continue;
                        }
                        if self.eat_type_args_close() {
                            break;
                        }
                        return Option::None;
                    }
                    if args.is_empty() {
                        return Option::None;
                    }
                }
                Some(TypeExpr::new(
                    TypeExprKind::Named { name, args },
                    self.span_from(start),
                ))
            }
            _ => Option::None,
        }
    }

    /// Consumes the `>` that closes a type-argument list.
    ///
    /// The lexer produces `>>` and `>=` as single tokens, so closing a nested
    /// list such as `dict<str, list<int>>` requires splitting them
    /// (DESIGN 3.9 point 4). The replacement is undone by
    /// [`Parser::rewind`] if the surrounding speculative parse fails.
    pub(crate) fn eat_type_args_close(&mut self) -> bool {
        match self.kind() {
            TokenKind::Gt => {
                self.bump();
                true
            }
            TokenKind::GtGt => {
                let span = self.span();
                let rest = Span::new(self.file, span.start + 1, span.end);
                self.patch(TokenKind::Gt, rest);
                true
            }
            TokenKind::GtEq => {
                let span = self.span();
                let rest = Span::new(self.file, span.start + 1, span.end);
                self.patch(TokenKind::Eq, rest);
                true
            }
            _ => false,
        }
    }

    /// Parses `<T, U>` after a `fn` or `class` name.
    pub(crate) fn parse_generic_params(&mut self) -> Vec<typhoon_ast::GenericParam> {
        let mut generics = Vec::new();
        if !self.eat(&TokenKind::Lt) {
            return generics;
        }
        loop {
            if self.eat_type_args_close() {
                break;
            }
            if matches!(
                self.kind(),
                TokenKind::Eof | TokenKind::Newline | TokenKind::Colon | TokenKind::LParen
            ) {
                self.expected("`>`");
                break;
            }
            let before = self.pos();
            match self.expect_ident("a generic parameter name") {
                Some(name) => {
                    let span = name.span;
                    generics.push(typhoon_ast::GenericParam { name, span });
                }
                None => {
                    if self.pos() == before {
                        self.bump();
                    }
                    continue;
                }
            }
            if self.eat(&TokenKind::Comma) {
                continue;
            }
            if self.eat_type_args_close() {
                break;
            }
            self.expected("`,` or `>`");
            break;
        }
        generics
    }
}
