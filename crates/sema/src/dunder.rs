//! Operator overloading by static dispatch to dunder methods (DESIGN §3.11).
//!
//! On a class instance an operator is a call of its dunder method, whether or
//! not the class declares the matching trait: `a + b` is `a.__add__(b)`, with
//! `b` checked against the method's parameter, so `fn __mul__(self, k: float)`
//! makes `v * 2.0` work. `!=` is `not a.__eq__(b)`, `a > b` is `b.__lt__(a)`,
//! `a <= b` is `not b.__lt__(a)` and `a >= b` is `not a.__lt__(b)`; operands
//! are still evaluated left to right. There are no reflected operators.
//!
//! The same dunders written as method calls on a builtin type —
//! `(3).__lt__(4)` — lower to the operator, so a bounded generic that calls
//! `x.__hash__()` works for every type that implements the bound.

use typhoon_ast as ast;
use typhoon_diag::{Diagnostic, Span};

use crate::check::{Checker, a_type};
use crate::hir;
use crate::traits::{Builtin, SynthKind};
use crate::types::{ClassId, Type, TypeId};

impl Checker<'_> {
    /// The parameter type of a dunder used as a binary operator, which must
    /// take exactly one argument besides `self`.
    fn operand_param(&mut self, func: hir::FuncId, op: &str, span: Span) -> Option<TypeId> {
        let sig = &self.sigs[func.index()];
        if sig.params.len() == 2 {
            return Some(sig.params[1].ty);
        }
        let (name, count) = (sig.name.clone(), sig.params.len().saturating_sub(1));
        self.error(
            Diagnostic::error(format!(
                "`{name}` takes {count} argument{} besides `self`, so it cannot implement `{op}`",
                if count == 1 { "" } else { "s" }
            ))
            .with_label(span, format!("`{op}` passes exactly one operand"))
            .with_note("see DESIGN.md section 3.11"),
        );
        None
    }

    /// Checks the right operand of an overloaded operator against the
    /// dunder's parameter.
    fn dunder_operand(
        &mut self,
        func: hir::FuncId,
        op: &str,
        op_span: Span,
        rhs: &ast::Expr,
    ) -> Option<hir::Expr> {
        let Some(param) = self.operand_param(func, op, op_span) else {
            self.check_expr(rhs);
            return None;
        };
        let value = self.check_expr_hint(rhs, Some(param));
        if value.ty == TypeId::ERROR || param == TypeId::ERROR {
            return None;
        }
        if !self.assignable(param, value.ty) {
            let (want, found) = (self.types.name(param), self.types.name(value.ty));
            let method = self.sigs[func.index()].name.clone();
            self.error(
                Diagnostic::error(format!(
                    "mismatched types: expected `{want}`, found `{found}`"
                ))
                .with_label(value.span, format!("expected `{want}`, found `{found}`"))
                .with_note(format!(
                    "`{op}` calls `{method}`, whose operand is `{want}`"
                )),
            );
            return None;
        }
        Some(self.coerce(value, param))
    }

    /// Reports an operator a class has no dunder for.
    fn no_dunder(&mut self, class: ClassId, op: &str, builtin: Option<Builtin>, op_span: Span) {
        if let Some(b) = builtin
            && self.traits.derive_failed.contains(&(class, b))
        {
            return;
        }
        let name = self.types.class(class).name.clone();
        let mut diag = Diagnostic::error(format!("`{op}` cannot be applied to `{name}`"))
            .with_label(op_span, format!("`{name}` has no `{op}` operator"));
        diag = match builtin {
            Some(b) if self.is_opaque(class) => {
                diag.with_help(format!("add the bound `{name}: {}`", b.name()))
            }
            Some(b) => diag.with_help(format!("define `{}` on `{name}`", b.method())),
            None => diag.with_help("only `+ - * / // %` and unary `-` can be overloaded"),
        };
        self.error(diag.with_note("see DESIGN.md section 3.11"));
    }

    /// `a + b` (and `- * / // % ** & | ^ << >>`) with a class instance `a`.
    pub(crate) fn class_binary(
        &mut self,
        op: ast::BinOp,
        op_span: Span,
        lhs: hir::Expr,
        rhs: &ast::Expr,
        span: Span,
    ) -> hir::Expr {
        let Type::Class(class) = self.types.get(lhs.ty) else {
            return self.error_expr(span);
        };
        let builtin = Builtin::of_binop(op);
        let func = builtin.and_then(|b| self.class_method(class, b.method(), op_span));
        let Some(func) = func else {
            self.check_expr(rhs);
            self.no_dunder(class, op.as_str(), builtin, op_span);
            return self.error_expr(span);
        };
        let Some(rhs) = self.dunder_operand(func, op.as_str(), op_span, rhs) else {
            return self.error_expr(span);
        };
        self.call_fn(func, vec![lhs, rhs], vec![0, 1], span)
    }

    /// `-a` (and `+a`, `~a`, `not a`) with a class instance `a`.
    pub(crate) fn class_unary(
        &mut self,
        op: ast::UnaryOp,
        op_span: Span,
        value: hir::Expr,
        span: Span,
    ) -> hir::Expr {
        let Type::Class(class) = self.types.get(value.ty) else {
            return self.error_expr(span);
        };
        let builtin = (op == ast::UnaryOp::Neg).then_some(Builtin::Neg);
        let func = builtin.and_then(|b| self.class_method(class, b.method(), op_span));
        let Some(func) = func else {
            self.no_dunder(class, op.as_str(), builtin, op_span);
            return self.error_expr(span);
        };
        let count = self.sigs[func.index()].params.len();
        if count != 1 {
            let name = self.sigs[func.index()].name.clone();
            self.error(
                Diagnostic::error(format!(
                    "`{name}` takes arguments besides `self`, so it cannot implement unary `-`"
                ))
                .with_label(op_span, "unary `-` passes no operand"),
            );
            return self.error_expr(span);
        }
        self.call_fn(func, vec![value], vec![0], span)
    }

    /// Reports `==` / `<` / … between values that have no such comparison.
    fn cannot_compare(&mut self, ty: TypeId, op: ast::CmpOp, op_span: Span) {
        let name = self.types.name(ty);
        let equality = matches!(op, ast::CmpOp::Eq | ast::CmpOp::Ne);
        if let Type::Class(class) = self.types.get(ty) {
            let builtin = if equality { Builtin::Eq } else { Builtin::Ord };
            if self.traits.derive_failed.contains(&(class, builtin)) {
                return;
            }
        }
        let mut diag = Diagnostic::error(format!(
            "`{name}` values cannot be compared with `{}`",
            op.as_str()
        ));
        diag = match self.types.get(ty) {
            Type::Class(class) if self.is_opaque(class) => {
                let bound = if equality { "Eq" } else { "Ord" };
                diag.with_label(op_span, format!("`{name}` is a type parameter"))
                    .with_help(format!("add the bound `{name}: {bound}`"))
            }
            _ if equality => diag
                .with_label(op_span, "no equality for class instances")
                .with_help("use `is` to compare identity, or define `__eq__` (or declare `Eq`)"),
            _ => diag
                .with_label(op_span, "no ordering for class instances")
                .with_help("define `__lt__` (or declare `Ord`)"),
        };
        self.error(diag.with_note("see DESIGN.md section 3.11"));
    }

    /// `a == b` / `a < b` / … with a class instance `a` (DESIGN §3.11).
    pub(crate) fn class_compare(
        &mut self,
        ast_op: ast::CmpOp,
        op: hir::CmpOp,
        op_span: Span,
        lhs: hir::Expr,
        rhs: &ast::Expr,
        span: Span,
    ) -> hir::Expr {
        let Type::Class(class) = self.types.get(lhs.ty) else {
            return self.error_expr(span);
        };
        let text = ast_op.as_str();
        match op {
            hir::CmpOp::Eq | hir::CmpOp::Ne | hir::CmpOp::Lt | hir::CmpOp::Ge => {
                let equality = matches!(op, hir::CmpOp::Eq | hir::CmpOp::Ne);
                let method = if equality { "__eq__" } else { "__lt__" };
                let Some(func) = self.class_method(class, method, op_span) else {
                    self.check_expr(rhs);
                    self.cannot_compare(lhs.ty, ast_op, op_span);
                    return self.error_expr(span);
                };
                let Some(rhs) = self.dunder_operand(func, text, op_span, rhs) else {
                    return self.error_expr(span);
                };
                if !self.returns_bool(func, op_span) {
                    return self.error_expr(span);
                }
                if equality {
                    let call = self.call_fn(func, vec![lhs, rhs], vec![0, 1], span);
                    if op == hir::CmpOp::Ne {
                        return hir::Expr {
                            kind: hir::ExprKind::Not(Box::new(call)),
                            ty: TypeId::BOOL,
                            span,
                        };
                    }
                    return call;
                }
                self.ordered_call(func, op, lhs, rhs, span)
            }
            // `a > b` is `b < a`, so the right operand's `__lt__` decides.
            hir::CmpOp::Gt | hir::CmpOp::Le => {
                let rhs = self.check_expr_hint(rhs, Some(lhs.ty));
                if rhs.ty == TypeId::ERROR {
                    return self.error_expr(span);
                }
                self.reversed_compare(ast_op, op, op_span, lhs, rhs, span)
            }
        }
    }

    /// `a > b` / `a <= b` through `b.__lt__(a)`, with both operands checked.
    pub(crate) fn reversed_compare(
        &mut self,
        ast_op: ast::CmpOp,
        op: hir::CmpOp,
        op_span: Span,
        lhs: hir::Expr,
        rhs: hir::Expr,
        span: Span,
    ) -> hir::Expr {
        let func = match self.types.get(rhs.ty) {
            Type::Class(class) => self.class_method(class, "__lt__", op_span),
            _ => None,
        };
        let Some(func) = func else {
            let blamed = if matches!(self.types.get(lhs.ty), Type::Class(_)) {
                lhs.ty
            } else {
                rhs.ty
            };
            self.cannot_compare(blamed, ast_op, op_span);
            return self.error_expr(span);
        };
        let Some(param) = self.operand_param(func, ast_op.as_str(), op_span) else {
            return self.error_expr(span);
        };
        if !self.assignable(param, lhs.ty) {
            self.mismatch(lhs.span, param, lhs.ty);
            return self.error_expr(span);
        }
        if !self.returns_bool(func, op_span) {
            return self.error_expr(span);
        }
        let lhs = self.coerce(lhs, param);
        self.ordered_call(func, op, lhs, rhs, span)
    }

    /// Reports a comparison dunder that does not return `bool`.
    fn returns_bool(&mut self, func: hir::FuncId, op_span: Span) -> bool {
        let sig = &self.sigs[func.index()];
        if sig.ret == TypeId::BOOL || sig.ret == TypeId::ERROR {
            return true;
        }
        let (name, found) = (sig.name.clone(), self.types.name(sig.ret));
        self.error(
            Diagnostic::error(format!(
                "`{name}` must return `bool` to be used as a comparison"
            ))
            .with_label(op_span, format!("`{name}` returns `{found}`")),
        );
        false
    }

    /// `<` / `<=` / `>` / `>=` on two tuples, lexicographically and directly:
    /// `a <= b` is not `not (b < a)`, which a NaN member would get wrong.
    pub(crate) fn tuple_ordering(
        &mut self,
        op: hir::CmpOp,
        lhs: hir::Expr,
        rhs: hir::Expr,
        span: Span,
    ) -> hir::Expr {
        self.ordered_values(op, lhs, rhs, span)
    }

    /// `x.__eq__(y)`, `x.__hash__()`, … on a value of a builtin type: the
    /// operator the dunder stands for (DESIGN §3.11).
    pub(crate) fn builtin_dunder_call(
        &mut self,
        builtin: Builtin,
        recv: hir::Expr,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let found = self.types.name(recv.ty);
        let wanted = usize::from(builtin.binary());
        if args.len() != wanted || args.iter().any(|a| a.name.is_some()) {
            for arg in args {
                self.check_expr(&arg.value);
            }
            self.error(
                Diagnostic::error(format!(
                    "`{found}.{}` takes {wanted} positional argument{}",
                    name.as_str(),
                    if wanted == 1 { "" } else { "s" }
                ))
                .with_label(span, "wrong arguments"),
            );
            return self.error_expr(span);
        }
        let other = args
            .first()
            .map(|arg| self.check_expr_hint(&arg.value, Some(recv.ty)));
        if other.as_ref().is_some_and(|o| o.ty == TypeId::ERROR) {
            return self.error_expr(span);
        }
        let unsupported = |checker: &mut Self| {
            checker.error(
                Diagnostic::error(format!("`{found}` has no method `{}`", name.as_str()))
                    .with_label(
                        name.span,
                        format!("`{found}` does not implement `{}`", builtin.name()),
                    )
                    .with_note("see DESIGN.md section 3.11"),
            );
            checker.error_expr(span)
        };
        match builtin {
            Builtin::Eq | Builtin::Ord => {
                let other = other.expect("checked above");
                if other.ty != recv.ty {
                    self.mismatch(other.span, recv.ty, other.ty);
                    return self.error_expr(span);
                }
                if builtin == Builtin::Eq {
                    if !self.ensure_eq(recv.ty, span) {
                        return unsupported(self);
                    }
                    return self.eq_values(recv, other, span);
                }
                if !self.ensure_ord(recv.ty, span) {
                    return unsupported(self);
                }
                self.lt_values(recv, other, span)
            }
            Builtin::Hash => {
                if !self.ensure_hash(recv.ty, span) {
                    return unsupported(self);
                }
                self.hash_value(recv, span)
            }
            Builtin::Repr => {
                if !self.ensure_repr(recv.ty, span) {
                    return unsupported(self);
                }
                self.repr_value(recv, span)
            }
            Builtin::Neg => self.unary_value(ast::UnaryOp::Neg, name.span, recv, span),
            _ => {
                let op = match builtin {
                    Builtin::Add => ast::BinOp::Add,
                    Builtin::Sub => ast::BinOp::Sub,
                    Builtin::Mul => ast::BinOp::Mul,
                    Builtin::Div => ast::BinOp::Div,
                    Builtin::FloorDiv => ast::BinOp::FloorDiv,
                    _ => ast::BinOp::Mod,
                };
                let other = other.expect("checked above");
                self.binary_values(op, name.span, recv, other, span)
            }
        }
    }

    /// Reports a keyword argument in a method call on a bounded type
    /// parameter: such calls are positional-only, since an implementation may
    /// name its parameters differently from the trait (DESIGN §3.11).
    pub(crate) fn keyword_through_bound(
        &mut self,
        class: ClassId,
        name: &ast::Ident,
        args: &[ast::Arg],
    ) {
        for arg in args {
            self.check_expr(&arg.value);
        }
        let param = self.types.class(class).name.clone();
        let method = name.as_str();
        if let Some(keyword) = args.iter().find_map(|a| a.name.as_ref()) {
            self.error(
                Diagnostic::error(format!(
                    "a call through the type parameter `{param}` takes positional arguments only"
                ))
                .with_label(
                    keyword.span,
                    format!("`{}=` names a parameter of `{method}`", keyword.as_str()),
                )
                .with_help(format!(
                    "pass it positionally; an implementation of `{method}` may name its \
                     parameters differently from the trait"
                ))
                .with_note("see DESIGN.md section 3.11"),
            );
        }
    }

    /// `repr(x)` (DESIGN §3.11): the text `print` writes for `x` inside a
    /// container, as a `str`.
    pub(crate) fn check_repr_builtin(&mut self, value: hir::Expr, span: Span) -> hir::Expr {
        if value.ty == TypeId::ERROR {
            return self.error_expr(span);
        }
        if !self.ensure_repr(value.ty, value.span) {
            let found = self.types.name(value.ty);
            self.error(
                Diagnostic::error(format!("`repr` is not defined for `{found}`"))
                    .with_label(value.span, format!("this is {}", a_type(&found)))
                    .with_help("a class needs `__repr__` (or to declare `Repr`)")
                    .with_note("see DESIGN.md section 3.11"),
            );
            return self.error_expr(span);
        }
        self.repr_value(value, span)
    }

    /// `hash(x)` (DESIGN §3.11), an `int`.
    pub(crate) fn check_hash_builtin(&mut self, value: hir::Expr, span: Span) -> hir::Expr {
        if value.ty == TypeId::ERROR {
            return self.error_expr(span);
        }
        if !self.ensure_hash(value.ty, value.span) {
            let found = self.types.name(value.ty);
            let help = match self.types.get(value.ty) {
                Type::Float => "`float` has no hash: NaN breaks hash consistency",
                Type::List(_) => "a `list` is mutable and has no hash; use a tuple",
                Type::Optional(_) => {
                    "a `T | None` has no hash yet, like a `T | None` dict key; \
                     narrow it first"
                }
                Type::Tuple(_) => {
                    "only a tuple of `int`, `bool` and `str` has a hash yet, \
                     like a tuple dict key"
                }
                _ => "a class needs `__hash__` (or to declare `Eq` and `Hash`)",
            };
            self.error(
                Diagnostic::error(format!("`{found}` is not hashable"))
                    .with_label(value.span, format!("this is {}", a_type(&found)))
                    .with_help(help)
                    .with_note("see DESIGN.md section 3.11"),
            );
            return self.error_expr(span);
        }
        self.hash_value(value, span)
    }

    /// `min(a, b)` / `max(a, b)` on an `Ord` type that has no native
    /// minimum: a synthesized helper calling `__lt__` (DESIGN §3.11).
    pub(crate) fn ordered_min_max(
        &mut self,
        is_max: bool,
        lhs: hir::Expr,
        rhs: hir::Expr,
        span: Span,
    ) -> Option<hir::Expr> {
        if !self.ensure_ord(lhs.ty, span) {
            return None;
        }
        let kind = if is_max {
            SynthKind::Max
        } else {
            SynthKind::Min
        };
        let func = self.synth_fn(kind, lhs.ty, span);
        Some(self.call_fn(func, vec![lhs, rhs], vec![0, 1], span))
    }
}
