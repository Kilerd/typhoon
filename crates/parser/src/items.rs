//! Top-level declarations: `fn`, `class` and constants (DESIGN 3.2, 3.3, 3.8).

use typhoon_ast::{
    Block, ClassDecl, ConstDecl, Field, FnDecl, Ident, Item, Param, TraitDecl, TraitMethod,
};
use typhoon_diag::Diagnostic;
use typhoon_lexer::TokenKind;

use crate::parser::Parser;

impl Parser<'_> {
    /// Parses one top-level declaration, recovering to the next item on error.
    pub(crate) fn parse_item(&mut self) -> Option<Item> {
        match self.kind() {
            TokenKind::Fn => self.parse_fn_decl().map(Item::Fn),
            TokenKind::Class => self.parse_class_decl().map(Item::Class),
            TokenKind::Trait => self.parse_trait_decl().map(Item::Trait),
            TokenKind::Ident(name) if name == "def" => {
                let span = self.span();
                self.report(
                    Diagnostic::error("unknown keyword `def`")
                        .with_label(span, "`def` is not a typhoon keyword")
                        .with_help("use `fn` to declare a function"),
                );
                self.bump();
                self.sync_to_item();
                None
            }
            TokenKind::Ident(_) if matches!(self.kind_at(1), TokenKind::Colon) => {
                self.parse_const_decl().map(Item::Const)
            }
            TokenKind::Indent => {
                let span = self.span();
                self.report(
                    Diagnostic::error("unexpected indentation")
                        .with_label(span, "top-level declarations start at column 1"),
                );
                self.skip_block();
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
            _ => {
                let span = self.span();
                self.report(
                    Diagnostic::error("top-level statements are not allowed")
                        .with_label(
                            span,
                            "only `fn`, `class` and constant declarations may appear here",
                        )
                        .with_help("put code in `fn main():`"),
                );
                self.bump();
                self.sync_to_item();
                None
            }
        }
    }

    /// `fn name<T>(params) -> ret:` plus its body.
    pub(crate) fn parse_fn_decl(&mut self) -> Option<FnDecl> {
        let start = self.span();
        self.bump(); // `fn`
        let Some(name) = self.expect_ident("a function name") else {
            self.sync_to_item();
            return None;
        };
        let generics = self.parse_generic_params();
        let params = self.parse_params();
        let ret = if self.eat(&TokenKind::Arrow) {
            Some(self.parse_type())
        } else {
            None
        };
        let body = self.parse_body("the function header");
        let span = start.merge(body.span);
        Some(FnDecl {
            name,
            generics,
            params,
            ret,
            body,
            span,
        })
    }

    /// `(a: int, b: float = 1.0)`, or `(self, ...)` for a method.
    fn parse_params(&mut self) -> Vec<Param> {
        let mut params = Vec::new();
        if !self.expect(&TokenKind::LParen, "`(` after the function name") {
            return params;
        }
        loop {
            if matches!(
                self.kind(),
                TokenKind::RParen | TokenKind::Eof | TokenKind::Newline | TokenKind::Colon
            ) {
                break;
            }
            let before = self.pos();
            if let Some(param) = self.parse_param() {
                params.push(param);
            }
            if self.pos() == before {
                self.bump();
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RParen, "`)`");
        params
    }

    fn parse_param(&mut self) -> Option<Param> {
        let start = self.span();
        let name = self.expect_ident("a parameter name")?;
        // DESIGN 3.8: the `self` receiver carries no type annotation.
        let is_self = name.name == "self" && !self.at(&TokenKind::Colon);
        let ty = if is_self {
            None
        } else if self.expect(&TokenKind::Colon, "`:` after the parameter name") {
            Some(self.parse_type())
        } else {
            self.report(
                Diagnostic::error(format!("parameter `{}` needs a type annotation", name.name))
                    .with_label(name.span, "add `: T` here")
                    .with_note("parameter types are always explicit, see DESIGN.md section 3.2"),
            );
            None
        };
        let default = if self.eat(&TokenKind::Eq) {
            Some(self.parse_expr())
        } else {
            None
        };
        Some(Param {
            name,
            ty,
            default,
            is_self,
            span: self.span_from(start),
        })
    }

    /// `NAME: T = value` at the top level.
    fn parse_const_decl(&mut self) -> Option<ConstDecl> {
        let start = self.span();
        let name = self.expect_ident("a constant name")?;
        self.expect(&TokenKind::Colon, "`:` after the constant name");
        let ty = self.parse_type();
        if !self.expect(&TokenKind::Eq, "`=` after the constant type") {
            self.sync_to_item();
            return None;
        }
        let value = self.parse_expr();
        let span = self.span_from(start);
        self.expect_newline("a constant declaration");
        Some(ConstDecl {
            name,
            ty,
            value,
            span,
        })
    }

    /// `class Name<T>:` plus its body of fields and methods.
    pub(crate) fn parse_class_decl(&mut self) -> Option<ClassDecl> {
        let start = self.span();
        self.bump(); // `class`
        let Some(name) = self.expect_ident("a class name") else {
            self.sync_to_item();
            return None;
        };
        let generics = self.parse_generic_params();
        // `class C(T1, T2):` lists the traits `C` implements (DESIGN 3.11).
        // Whether a name is a trait or a class (inheritance, which typhoon
        // does not have) is for sema to say.
        let traits = if self.at(&TokenKind::LParen) {
            self.parse_trait_list()
        } else {
            Vec::new()
        };
        if self.at(&TokenKind::LBrace) {
            let span = self.span();
            self.report(
                Diagnostic::error("expected `:` after the class header, found `{`")
                    .with_label(span, "expected `:` after the class header")
                    .with_help("typhoon uses indentation blocks, not braces"),
            );
            let body = self.parse_body("the class header");
            return Some(ClassDecl {
                name,
                generics,
                traits,
                fields: Vec::new(),
                methods: Vec::new(),
                span: start.merge(body.span),
            });
        }
        self.expect_colon("the class header");

        let mut fields = Vec::new();
        let mut methods = Vec::new();
        let mut end = self.prev_span();

        if !self.eat(&TokenKind::Newline) && !self.at(&TokenKind::Indent) {
            self.expected("end of line after the class header");
            self.sync_to_newline();
        }
        if !self.eat(&TokenKind::Indent) {
            let span = self.span();
            self.report(
                Diagnostic::error("expected an indented block after the class header")
                    .with_label(span, "expected an indented block")
                    .with_help("indent the fields and methods by 4 spaces"),
            );
            return Some(ClassDecl {
                name,
                generics,
                traits,
                fields,
                methods,
                span: start.merge(span),
            });
        }

        loop {
            self.skip_newlines();
            if matches!(self.kind(), TokenKind::Dedent | TokenKind::Eof) {
                break;
            }
            let before = self.pos();
            match self.kind() {
                TokenKind::Fn => {
                    if let Some(method) = self.parse_fn_decl() {
                        end = method.span;
                        methods.push(method);
                    }
                }
                TokenKind::Pass => {
                    self.bump();
                    end = self.prev_span();
                    self.expect_newline("`pass`");
                }
                TokenKind::Ident(_) if matches!(self.kind_at(1), TokenKind::Colon) => {
                    if let Some(field) = self.parse_field() {
                        end = field.span;
                        fields.push(field);
                    }
                }
                _ => {
                    let span = self.span();
                    self.report(
                        Diagnostic::error("expected a field or method declaration")
                            .with_label(span, "not a field (`name: T`) or a method (`fn ...`)")
                            .with_help("a class body contains only fields, methods and `pass`"),
                    );
                    self.recover_from_stray_header();
                }
            }
            if self.pos() == before {
                self.bump();
            }
        }
        self.eat(&TokenKind::Dedent);
        Some(ClassDecl {
            name,
            generics,
            traits,
            fields,
            methods,
            span: start.merge(end),
        })
    }

    /// `(T1, T2)` after a class name: the traits the class implements.
    fn parse_trait_list(&mut self) -> Vec<Ident> {
        let mut traits = Vec::new();
        self.bump(); // `(`
        loop {
            if matches!(
                self.kind(),
                TokenKind::RParen | TokenKind::Eof | TokenKind::Newline | TokenKind::Colon
            ) {
                break;
            }
            let before = self.pos();
            if let Some(name) = self.expect_ident("a trait name") {
                traits.push(name);
            } else if self.pos() == before {
                self.bump();
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        if !self.expect(&TokenKind::RParen, "`)` after the trait list") {
            // Skip the rest of a malformed list so the header still parses.
            while !matches!(
                self.kind(),
                TokenKind::Colon | TokenKind::Newline | TokenKind::Eof | TokenKind::Indent
            ) {
                self.bump();
            }
        }
        traits
    }

    /// `trait Name:` plus its body of method signatures (DESIGN 3.11). A
    /// method whose header ends at the line is required; one with a `:` and a
    /// body is a default implementation.
    pub(crate) fn parse_trait_decl(&mut self) -> Option<TraitDecl> {
        let start = self.span();
        self.bump(); // `trait`
        let Some(name) = self.expect_ident("a trait name") else {
            self.sync_to_item();
            return None;
        };
        if self.at(&TokenKind::Lt) {
            let span = self.span();
            self.report(
                Diagnostic::error("generic traits are not supported")
                    .with_label(span, "a trait takes no type parameters")
                    .with_note("see DESIGN.md section 3.11"),
            );
            self.parse_generic_params();
        }
        self.expect_colon("the trait header");
        let mut methods = Vec::new();
        let mut end = self.prev_span();
        if !self.eat(&TokenKind::Newline) && !self.at(&TokenKind::Indent) {
            self.expected("end of line after the trait header");
            self.sync_to_newline();
        }
        if !self.eat(&TokenKind::Indent) {
            let span = self.span();
            self.report(
                Diagnostic::error("expected an indented block after the trait header")
                    .with_label(span, "expected an indented block")
                    .with_help("indent the method signatures by 4 spaces"),
            );
            return Some(TraitDecl {
                name,
                methods,
                span: start.merge(span),
            });
        }
        loop {
            self.skip_newlines();
            if matches!(self.kind(), TokenKind::Dedent | TokenKind::Eof) {
                break;
            }
            let before = self.pos();
            match self.kind() {
                TokenKind::Fn => {
                    if let Some(method) = self.parse_trait_method() {
                        end = method.decl.span;
                        methods.push(method);
                    }
                }
                TokenKind::Pass => {
                    self.bump();
                    end = self.prev_span();
                    self.expect_newline("`pass`");
                }
                _ => {
                    let span = self.span();
                    self.report(
                        Diagnostic::error("expected a method declaration")
                            .with_label(span, "a trait contains only `fn` signatures")
                            .with_help("traits have no fields; declare them on the classes"),
                    );
                    self.recover_from_stray_header();
                }
            }
            if self.pos() == before {
                self.bump();
            }
        }
        self.eat(&TokenKind::Dedent);
        Some(TraitDecl {
            name,
            methods,
            span: start.merge(end),
        })
    }

    /// One `fn` inside a trait: a signature, with or without a default body.
    fn parse_trait_method(&mut self) -> Option<TraitMethod> {
        let start = self.span();
        self.bump(); // `fn`
        let Some(name) = self.expect_ident("a method name") else {
            self.sync_to_newline();
            return None;
        };
        let generics = self.parse_generic_params();
        let params = self.parse_params();
        let ret = if self.eat(&TokenKind::Arrow) {
            Some(self.parse_type())
        } else {
            None
        };
        if self.at(&TokenKind::Colon) || self.at(&TokenKind::LBrace) {
            let body = self.parse_body("the method header");
            let span = start.merge(body.span);
            return Some(TraitMethod {
                decl: FnDecl {
                    name,
                    generics,
                    params,
                    ret,
                    body,
                    span,
                },
                has_body: true,
            });
        }
        let span = self.span_from(start);
        self.expect_newline("a required method signature");
        Some(TraitMethod {
            decl: FnDecl {
                name,
                generics,
                params,
                ret,
                body: Block {
                    stmts: Vec::new(),
                    span,
                },
                span,
            },
            has_body: false,
        })
    }

    fn parse_field(&mut self) -> Option<Field> {
        let start = self.span();
        let name = self.expect_ident("a field name")?;
        self.expect(&TokenKind::Colon, "`:` after the field name");
        let ty = self.parse_type();
        let default = if self.eat(&TokenKind::Eq) {
            Some(self.parse_expr())
        } else {
            None
        };
        let span = self.span_from(start);
        self.expect_newline("a field declaration");
        Some(Field {
            name,
            ty,
            default,
            span,
        })
    }
}
