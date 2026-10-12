//! The expression rules for `T | None` that go beyond the type table
//! (DESIGN §3.8, §3.10): comparing with `None`, `==` between a `T | None` and
//! a `T`, and refusing an un-narrowed value where a `T` is required.
//!
//! Narrowing itself lives with the statements that perform it (`if` in
//! `check.rs`, the `if` clauses of a comprehension in `comp.rs`); reading a
//! narrowed local is `check_name`'s job.

use typhoon_ast as ast;
use typhoon_diag::{Diagnostic, Span};

use crate::check::Checker;
use crate::hir;

impl Checker<'_> {
    /// Reports `value` when it is an un-narrowed `T | None` used where a `T`
    /// is required, e.g. as an operand of `+` or `<`. Returns whether it did.
    pub(crate) fn reject_optional_operand(&mut self, value: &hir::Expr) -> bool {
        if self.types.as_optional(value.ty).is_none() {
            return false;
        }
        self.require_narrowed(value.span, value.ty);
        true
    }

    /// Widens the plain side of `a == b` when the other side is its
    /// `T | None`, so that `x == 3` asks "is `x` the value 3?" as in Python.
    pub(crate) fn widen_to_optional(
        &mut self,
        lhs: hir::Expr,
        rhs: hir::Expr,
    ) -> (hir::Expr, hir::Expr) {
        if lhs.ty == rhs.ty {
            return (lhs, rhs);
        }
        if self.types.as_optional(lhs.ty) == Some(rhs.ty) {
            let ty = lhs.ty;
            let rhs = self.coerce(rhs, ty);
            return (lhs, rhs);
        }
        if self.types.as_optional(rhs.ty) == Some(lhs.ty) {
            let ty = rhs.ty;
            let lhs = self.coerce(lhs, ty);
            return (lhs, rhs);
        }
        (lhs, rhs)
    }

    /// `x == None` / `x != None`: always an error that points at `is None`,
    /// which is the identity test that also narrows `x`.
    pub(crate) fn compare_with_none(
        &mut self,
        left: &ast::Expr,
        entry: &ast::CompareTail,
        span: Span,
    ) -> hir::Expr {
        let other = if matches!(left.kind, ast::ExprKind::None) {
            &entry.rhs
        } else {
            left
        };
        if !matches!(other.kind, ast::ExprKind::None) {
            self.check_expr(other);
        }
        let (is, wrong) = match entry.op {
            ast::CmpOp::Ne => ("is not", "!="),
            _ => ("is", "=="),
        };
        self.error(
            Diagnostic::error(format!(
                "use `{is} None` to compare with `None`, not `{wrong}`"
            ))
            .with_label(entry.op_span, format!("write `{is}` here"))
            .with_help(format!(
                "`None` is tested by identity: `x {is} None`, which also narrows `x` \
                 inside an `if`"
            ))
            .with_note("see DESIGN.md section 3.8"),
        );
        self.error_expr(span)
    }

    /// The un-narrowed reading of `value`: when it reads a `T | None` local
    /// that the flow has narrowed to `T`, the local itself with its declared
    /// type. `x is None` tests the local, whatever is known about it.
    pub(crate) fn unnarrowed(&self, value: hir::Expr) -> hir::Expr {
        let local = match &value.kind {
            hir::ExprKind::Local(id) => Some(*id),
            hir::ExprKind::OptionalValue(inner) => match inner.kind {
                hir::ExprKind::Local(id) => Some(id),
                _ => None,
            },
            _ => None,
        };
        let Some(id) = local else {
            return value;
        };
        let declared = self.locals[id.index()].ty;
        if declared == value.ty || self.types.as_optional(declared).is_none() {
            return value;
        }
        hir::Expr {
            kind: hir::ExprKind::Local(id),
            ty: declared,
            span: value.span,
        }
    }
}
