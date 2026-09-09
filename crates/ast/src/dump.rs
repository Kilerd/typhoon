//! A deterministic, compact pretty-printer for the AST, used by snapshot tests.
//!
//! Every node is printed on its own line as
//!
//! ```text
//! <indent><label ><Kind> <detail> @<start>..<end>
//! ```
//!
//! where the indentation shows nesting, `label` names the slot the child sits
//! in (`cond`, `body`, `lhs`, ...) and is omitted for positional children, and
//! the span is the node's byte range. A [`Span::dummy`] prints as `@?`.
//!
//! ```
//! # use typhoon_ast::*;
//! # use typhoon_diag::{FileId, Span};
//! let span = Span::new(FileId(0), 0, 1);
//! let expr = Expr::new(ExprKind::Int(1), span);
//! assert_eq!(dump_expr(&expr), "Int 1 @0..1");
//! ```

use std::fmt::Write as _;

use typhoon_diag::Span;

use crate::nodes::*;

/// Pretty-prints a whole module.
pub fn dump(module: &Module) -> String {
    let mut d = Dumper::new();
    d.module(module);
    d.finish()
}

/// Pretty-prints a single expression.
pub fn dump_expr(expr: &Expr) -> String {
    let mut d = Dumper::new();
    d.expr("", expr);
    d.finish()
}

/// Pretty-prints a single statement.
pub fn dump_stmt(stmt: &Stmt) -> String {
    let mut d = Dumper::new();
    d.stmt(stmt);
    d.finish()
}

/// Pretty-prints a single type expression.
pub fn dump_type(ty: &TypeExpr) -> String {
    let mut d = Dumper::new();
    d.ty("", ty);
    d.finish()
}

struct Dumper {
    out: String,
    depth: usize,
}

impl Dumper {
    fn new() -> Dumper {
        Dumper {
            out: String::new(),
            depth: 0,
        }
    }

    fn finish(mut self) -> String {
        while self.out.ends_with('\n') {
            self.out.pop();
        }
        self.out
    }

    fn line(&mut self, label: &str, body: &str, span: Span) {
        for _ in 0..self.depth {
            self.out.push_str("  ");
        }
        if !label.is_empty() {
            self.out.push_str(label);
            self.out.push(' ');
        }
        self.out.push_str(body);
        if span.is_dummy() {
            self.out.push_str(" @?");
        } else {
            let _ = write!(self.out, " @{}..{}", span.start, span.end);
        }
        self.out.push('\n');
    }

    fn nested(&mut self, f: impl FnOnce(&mut Dumper)) {
        self.depth += 1;
        f(self);
        self.depth -= 1;
    }

    fn module(&mut self, module: &Module) {
        self.line("", "Module", module.span);
        self.nested(|d| {
            for item in &module.items {
                d.item(item);
            }
        });
    }

    fn item(&mut self, item: &Item) {
        match item {
            Item::Fn(decl) => self.fn_decl("", decl),
            Item::Class(decl) => self.class_decl(decl),
            Item::Const(decl) => self.const_decl(decl),
        }
    }

    fn fn_decl(&mut self, label: &str, decl: &FnDecl) {
        self.line(label, &format!("FnDecl `{}`", decl.name.name), decl.span);
        self.nested(|d| {
            for g in &decl.generics {
                d.line("generic", &format!("`{}`", g.name.name), g.span);
            }
            for p in &decl.params {
                let flag = if p.is_self { " (self)" } else { "" };
                d.line("param", &format!("`{}`{}", p.name.name, flag), p.span);
                d.nested(|d| {
                    if let Some(ty) = &p.ty {
                        d.ty("type", ty);
                    }
                    if let Some(default) = &p.default {
                        d.expr("default", default);
                    }
                });
            }
            if let Some(ret) = &decl.ret {
                d.ty("ret", ret);
            }
            d.block("body", &decl.body);
        });
    }

    fn class_decl(&mut self, decl: &ClassDecl) {
        self.line("", &format!("ClassDecl `{}`", decl.name.name), decl.span);
        self.nested(|d| {
            for g in &decl.generics {
                d.line("generic", &format!("`{}`", g.name.name), g.span);
            }
            for f in &decl.fields {
                d.line("field", &format!("`{}`", f.name.name), f.span);
                d.nested(|d| {
                    d.ty("type", &f.ty);
                    if let Some(default) = &f.default {
                        d.expr("default", default);
                    }
                });
            }
            for m in &decl.methods {
                d.fn_decl("method", m);
            }
        });
    }

    fn const_decl(&mut self, decl: &ConstDecl) {
        self.line("", &format!("ConstDecl `{}`", decl.name.name), decl.span);
        self.nested(|d| {
            d.ty("type", &decl.ty);
            d.expr("value", &decl.value);
        });
    }

    fn block(&mut self, label: &str, block: &Block) {
        self.line(label, "Block", block.span);
        self.nested(|d| {
            for stmt in &block.stmts {
                d.stmt(stmt);
            }
        });
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Expr(e) => {
                self.line("", "ExprStmt", stmt.span);
                self.nested(|d| d.expr("", e));
            }
            StmtKind::Assign { target, value } => {
                self.line("", "Assign", stmt.span);
                self.nested(|d| {
                    d.expr("target", target);
                    d.expr("value", value);
                });
            }
            StmtKind::AnnAssign { name, ty, value } => {
                self.line("", &format!("AnnAssign `{}`", name.name), stmt.span);
                self.nested(|d| {
                    d.ty("type", ty);
                    if let Some(value) = value {
                        d.expr("value", value);
                    }
                });
            }
            StmtKind::Return(value) => {
                self.line("", "Return", stmt.span);
                if let Some(value) = value {
                    self.nested(|d| d.expr("", value));
                }
            }
            StmtKind::If {
                cond,
                then,
                elifs,
                else_,
            } => {
                self.line("", "If", stmt.span);
                self.nested(|d| {
                    d.expr("cond", cond);
                    d.block("then", then);
                    for elif in elifs {
                        d.line("elif", "ElifBranch", elif.span);
                        d.nested(|d| {
                            d.expr("cond", &elif.cond);
                            d.block("body", &elif.body);
                        });
                    }
                    if let Some(else_) = else_ {
                        d.block("else", else_);
                    }
                });
            }
            StmtKind::While { cond, body } => {
                self.line("", "While", stmt.span);
                self.nested(|d| {
                    d.expr("cond", cond);
                    d.block("body", body);
                });
            }
            StmtKind::For { var, iter, body } => {
                self.line("", &format!("For `{}`", var.name), stmt.span);
                self.nested(|d| {
                    d.expr("iter", iter);
                    d.block("body", body);
                });
            }
            StmtKind::Break => self.line("", "Break", stmt.span),
            StmtKind::Continue => self.line("", "Continue", stmt.span),
            StmtKind::Pass => self.line("", "Pass", stmt.span),
        }
    }

    fn expr(&mut self, label: &str, expr: &Expr) {
        match &expr.kind {
            ExprKind::Int(v) => self.line(label, &format!("Int {v}"), expr.span),
            ExprKind::Float(v) => self.line(label, &format!("Float {v:?}"), expr.span),
            ExprKind::Str(v) => self.line(label, &format!("Str {v:?}"), expr.span),
            ExprKind::Bool(v) => {
                let v = if *v { "True" } else { "False" };
                self.line(label, &format!("Bool {v}"), expr.span);
            }
            ExprKind::None => self.line(label, "None", expr.span),
            ExprKind::Name(name) => self.line(label, &format!("Name `{}`", name.name), expr.span),
            ExprKind::FString(parts) => {
                self.line(label, "FString", expr.span);
                self.nested(|d| {
                    for part in parts {
                        match part {
                            FStringPart::Literal { value, span } => {
                                d.line("literal", &format!("{value:?}"), *span);
                            }
                            FStringPart::Expr {
                                expr,
                                spec,
                                spec_span,
                                span,
                            } => {
                                d.line("hole", "Hole", *span);
                                d.nested(|d| {
                                    d.expr("", expr);
                                    if let Some(spec) = spec {
                                        d.line(
                                            "spec",
                                            &format!("{spec:?}"),
                                            spec_span.unwrap_or(Span::dummy()),
                                        );
                                    }
                                });
                            }
                        }
                    }
                });
            }
            ExprKind::Attribute { base, attr } => {
                self.line(label, &format!("Attribute `{}`", attr.name), expr.span);
                self.nested(|d| d.expr("base", base));
            }
            ExprKind::Call {
                callee,
                type_args,
                args,
            } => {
                self.line(label, "Call", expr.span);
                self.nested(|d| {
                    d.expr("callee", callee);
                    if let Some(type_args) = type_args {
                        for ty in type_args {
                            d.ty("typearg", ty);
                        }
                    }
                    for arg in args {
                        match &arg.name {
                            Some(name) => d.line("arg", &format!("`{}`=", name.name), arg.span),
                            None => d.line("arg", "positional", arg.span),
                        }
                        d.nested(|d| d.expr("", &arg.value));
                    }
                });
            }
            ExprKind::Index { base, index } => {
                self.line(label, "Index", expr.span);
                self.nested(|d| {
                    d.expr("base", base);
                    d.expr("index", index);
                });
            }
            ExprKind::Unary {
                op,
                op_span,
                expr: operand,
            } => {
                self.line(label, &format!("Unary {op}"), expr.span);
                self.nested(|d| {
                    d.line("op", "Op", *op_span);
                    d.expr("", operand);
                });
            }
            ExprKind::Binary {
                op,
                op_span,
                lhs,
                rhs,
            } => {
                self.line(label, &format!("Binary {op}"), expr.span);
                self.nested(|d| {
                    d.line("op", "Op", *op_span);
                    d.expr("lhs", lhs);
                    d.expr("rhs", rhs);
                });
            }
            ExprKind::BoolOp {
                op,
                op_span,
                lhs,
                rhs,
            } => {
                self.line(label, &format!("BoolOp {op}"), expr.span);
                self.nested(|d| {
                    d.line("op", "Op", *op_span);
                    d.expr("lhs", lhs);
                    d.expr("rhs", rhs);
                });
            }
            ExprKind::Compare { left, tail } => {
                self.line(label, "Compare", expr.span);
                self.nested(|d| {
                    d.expr("left", left);
                    for t in tail {
                        d.line("cmp", &format!("`{}`", t.op), t.op_span);
                        d.nested(|d| d.expr("", &t.rhs));
                    }
                });
            }
            ExprKind::List(items) => {
                self.line(label, "List", expr.span);
                self.nested(|d| {
                    for item in items {
                        d.expr("", item);
                    }
                });
            }
            ExprKind::Set(items) => {
                self.line(label, "Set", expr.span);
                self.nested(|d| {
                    for item in items {
                        d.expr("", item);
                    }
                });
            }
            ExprKind::Tuple(items) => {
                self.line(label, "Tuple", expr.span);
                self.nested(|d| {
                    for item in items {
                        d.expr("", item);
                    }
                });
            }
            ExprKind::Dict(entries) => {
                self.line(label, "Dict", expr.span);
                self.nested(|d| {
                    for (k, v) in entries {
                        d.expr("key", k);
                        d.expr("value", v);
                    }
                });
            }
            ExprKind::Error => self.line(label, "Error", expr.span),
        }
    }

    fn ty(&mut self, label: &str, ty: &TypeExpr) {
        match &ty.kind {
            TypeExprKind::Named { name, args } => {
                self.line(label, &format!("Named `{}`", name.name), ty.span);
                self.nested(|d| {
                    for arg in args {
                        d.ty("arg", arg);
                    }
                });
            }
            TypeExprKind::Union(members) => {
                self.line(label, "Union", ty.span);
                self.nested(|d| {
                    for m in members {
                        d.ty("", m);
                    }
                });
            }
            TypeExprKind::None => self.line(label, "NoneType", ty.span),
            TypeExprKind::Error => self.line(label, "ErrorType", ty.span),
        }
    }
}

impl std::fmt::Display for Module {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&dump(self))
    }
}
