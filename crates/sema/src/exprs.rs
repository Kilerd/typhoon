//! Expression checking: synthesis of a type for every AST expression and the
//! lowering into the typed [`hir`](crate::hir).
//!
//! There are no implicit conversions (DESIGN §4.3), so every rule is
//! "synthesize both operands, then demand that they agree"; the checking
//! direction is only used to phrase the diagnostic.

use typhoon_ast as ast;
use typhoon_diag::{Diagnostic, Span};

use crate::check::{Checker, a_type};
use crate::hir;
use crate::types::{ClassId, Type, TypeId};

/// The result of matching the arguments of a call against a signature.
struct CallArgs {
    args: Vec<hir::Expr>,
    eval_order: Vec<u32>,
}

impl Checker<'_> {
    /// Type-checks one expression and lowers it.
    pub(crate) fn check_expr(&mut self, expr: &ast::Expr) -> hir::Expr {
        self.check_expr_hint(expr, None)
    }

    /// Type-checks one expression against an optional expected type.
    ///
    /// Typhoon's inference is bidirectional but strictly local (DESIGN §4.4):
    /// the hint only reaches the three literals that cannot synthesize a type
    /// on their own — an empty `list` literal, a tuple whose members need
    /// widening, and `None` as a `C | None`. Everything else synthesizes and is
    /// then checked by the caller.
    pub(crate) fn check_expr_hint(
        &mut self,
        expr: &ast::Expr,
        expected: Option<TypeId>,
    ) -> hir::Expr {
        let span = expr.span;
        match &expr.kind {
            ast::ExprKind::List(items) => return self.check_list_literal(items, expected, span),
            ast::ExprKind::Tuple(items) => return self.check_tuple_literal(items, expected, span),
            ast::ExprKind::None => {
                if let Some(expected) = expected
                    && matches!(self.types.get(expected), Type::Optional(_))
                {
                    return hir::Expr {
                        kind: hir::ExprKind::NoneRef,
                        ty: expected,
                        span,
                    };
                }
            }
            _ => {}
        }
        match &expr.kind {
            // The parser already reported whatever produced this node.
            ast::ExprKind::Error => self.error_expr(span),
            ast::ExprKind::Int(v) => self.lit(hir::ExprKind::Int(*v), TypeId::INT, span),
            ast::ExprKind::Float(v) => self.lit(hir::ExprKind::Float(*v), TypeId::FLOAT, span),
            ast::ExprKind::Bool(v) => self.lit(hir::ExprKind::Bool(*v), TypeId::BOOL, span),
            ast::ExprKind::Str(v) => self.lit(hir::ExprKind::Str(v.clone()), TypeId::STR, span),
            ast::ExprKind::None => self.lit(hir::ExprKind::Unit, TypeId::UNIT, span),
            ast::ExprKind::Name(name) => self.check_name(name),
            ast::ExprKind::FString(parts) => self.check_fstring(parts, span),
            ast::ExprKind::Unary { op, op_span, expr } => {
                self.check_unary(*op, *op_span, expr, span)
            }
            ast::ExprKind::Binary {
                op,
                op_span,
                lhs,
                rhs,
            } => self.check_binary(*op, *op_span, lhs, rhs, span),
            ast::ExprKind::BoolOp { op, lhs, rhs, .. } => self.check_bool_op(*op, lhs, rhs, span),
            ast::ExprKind::Compare { left, tail } => self.check_compare(left, tail, span),
            ast::ExprKind::Call {
                callee,
                type_args,
                args,
            } => self.check_call(callee, type_args.as_deref(), args, span),
            ast::ExprKind::Attribute { base, attr } => self.check_attribute(base, attr, span),
            ast::ExprKind::Index { base, index } => self.check_index(base, index, span),
            // Handled above, with the expected type.
            ast::ExprKind::List(_) | ast::ExprKind::Tuple(_) => self.error_expr(span),
            ast::ExprKind::Dict(entries) => {
                for (k, v) in entries {
                    self.check_expr(k);
                    self.check_expr(v);
                }
                self.unsupported(span, "a dict literal", "M3");
                self.error_expr(span)
            }
            ast::ExprKind::Set(items) => {
                for item in items {
                    self.check_expr(item);
                }
                self.unsupported(span, "a set literal", "M3");
                self.error_expr(span)
            }
        }
    }

    fn lit(&self, kind: hir::ExprKind, ty: TypeId, span: Span) -> hir::Expr {
        hir::Expr { kind, ty, span }
    }

    // -- names ------------------------------------------------------------

    fn check_name(&mut self, name: &ast::Ident) -> hir::Expr {
        let span = name.span;
        if let Some(id) = self.scope.get(name.as_str()).copied() {
            let ty = self.locals[id.index()].ty;
            if ty == TypeId::ERROR {
                // The declaration was already rejected.
                return self.error_expr(span);
            }
            if !self.assigned.get(id.index()).copied().unwrap_or(false) {
                let decl = self.locals[id.index()].span;
                self.error(
                    Diagnostic::error(format!(
                        "use of possibly-uninitialized variable `{}`",
                        name.as_str()
                    ))
                    .with_label(
                        span,
                        format!("`{}` may be uninitialized here", name.as_str()),
                    )
                    .with_secondary(decl, "it is only assigned on some paths")
                    .with_note(
                        "every path reaching a read must assign the variable, \
                         see DESIGN.md section 3.4",
                    ),
                );
                return self.error_expr(span);
            }
            // A `C | None` the control flow has proved non-null reads as a
            // plain `C` (DESIGN §3.8); the value is the same pointer.
            let ty = match self.types.get(ty) {
                crate::types::Type::Optional(class) if self.narrowed.contains(&id) => {
                    self.types.class_ty(class)
                }
                _ => ty,
            };
            return hir::Expr {
                kind: hir::ExprKind::Local(id),
                ty,
                span,
            };
        }
        if let Some(entry) = self.consts.get(name.as_str()) {
            return entry.value.to_expr(span);
        }
        if self.poisoned.contains(name.as_str()) {
            return self.error_expr(span);
        }
        if self.fn_index.contains_key(name.as_str()) {
            self.error(
                Diagnostic::error(format!("`{}` is a function, not a value", name.as_str()))
                    .with_label(span, "functions cannot be used as values")
                    .with_help(format!("call it: `{}(...)`", name.as_str())),
            );
            return self.error_expr(span);
        }
        if is_builtin(name.as_str()) {
            self.error(
                Diagnostic::error(format!(
                    "the builtin `{}` can only be called",
                    name.as_str()
                ))
                .with_label(span, "expected a value"),
            );
            return self.error_expr(span);
        }
        self.error(
            Diagnostic::error(format!(
                "cannot find value `{}` in this scope",
                name.as_str()
            ))
            .with_label(span, "not found in this scope"),
        );
        self.error_expr(span)
    }

    // -- operators --------------------------------------------------------

    fn check_unary(
        &mut self,
        op: ast::UnaryOp,
        op_span: Span,
        operand: &ast::Expr,
        span: Span,
    ) -> hir::Expr {
        let value = self.check_expr(operand);
        if value.ty == TypeId::ERROR {
            return self.error_expr(span);
        }
        // `-1` is a negated literal, not a negative literal (Python has no
        // negative literals); folding it here keeps `range(10, 0, -2)` and
        // `x // -1` on the constant paths of codegen.
        match (op, &value.kind) {
            (ast::UnaryOp::Neg, hir::ExprKind::Int(v)) => {
                return hir::Expr {
                    kind: hir::ExprKind::Int(v.wrapping_neg()),
                    ty: TypeId::INT,
                    span,
                };
            }
            (ast::UnaryOp::Neg, hir::ExprKind::Float(v)) => {
                return hir::Expr {
                    kind: hir::ExprKind::Float(-v),
                    ty: TypeId::FLOAT,
                    span,
                };
            }
            _ => {}
        }
        let (kind, ty) = match (op, value.ty) {
            (ast::UnaryOp::Neg, TypeId::INT) => {
                (hir::ExprKind::IntNeg(Box::new(value)), TypeId::INT)
            }
            (ast::UnaryOp::Neg, TypeId::FLOAT) => {
                (hir::ExprKind::FloatNeg(Box::new(value)), TypeId::FLOAT)
            }
            (ast::UnaryOp::Pos, TypeId::INT) | (ast::UnaryOp::Pos, TypeId::FLOAT) => {
                let ty = value.ty;
                (value.kind, ty)
            }
            (ast::UnaryOp::BitNot, TypeId::INT) => {
                (hir::ExprKind::IntNot(Box::new(value)), TypeId::INT)
            }
            (ast::UnaryOp::Not, TypeId::BOOL) => {
                (hir::ExprKind::Not(Box::new(value)), TypeId::BOOL)
            }
            _ => {
                let found = self.types.name(value.ty);
                let mut diag = Diagnostic::error(format!(
                    "`{}` cannot be applied to a value of type `{found}`",
                    op.as_str()
                ))
                .with_label(
                    op_span,
                    format!("no `{}` operator for `{found}`", op.as_str()),
                )
                .with_secondary(value.span, format!("this is {}", a_type(&found)));
                if op == ast::UnaryOp::Not {
                    diag = diag
                        .with_help("compare explicitly, e.g. `x != 0`")
                        .with_note("typhoon has no truthiness, see DESIGN.md section 3.6");
                }
                if op == ast::UnaryOp::BitNot {
                    diag = diag.with_help("bitwise operators only apply to `int`");
                }
                self.error(diag);
                return self.error_expr(span);
            }
        };
        hir::Expr { kind, ty, span }
    }

    fn check_binary(
        &mut self,
        op: ast::BinOp,
        op_span: Span,
        lhs: &ast::Expr,
        rhs: &ast::Expr,
        span: Span,
    ) -> hir::Expr {
        let lhs = self.check_expr(lhs);
        let rhs = self.check_expr_hint(rhs, Some(lhs.ty));
        if lhs.ty == TypeId::ERROR || rhs.ty == TypeId::ERROR {
            return self.error_expr(span);
        }
        if lhs.ty != rhs.ty {
            let (l, r) = (self.types.name(lhs.ty), self.types.name(rhs.ty));
            let mut diag =
                Diagnostic::error(format!("mismatched types: expected `{l}`, found `{r}`"))
                    .with_label(rhs.span, format!("expected `{l}`, found `{r}`"))
                    .with_secondary(lhs.span, format!("this is {}", a_type(&l)));
            if (lhs.ty == TypeId::FLOAT && rhs.ty == TypeId::INT)
                || (lhs.ty == TypeId::INT && rhs.ty == TypeId::FLOAT)
            {
                diag = diag
                    .with_help("use `float(x)` to convert an `int` to a `float`")
                    .with_note(
                        "there are no implicit numeric conversions, see DESIGN.md section 4.3",
                    );
            }
            self.error(diag);
            return self.error_expr(span);
        }

        let ty = lhs.ty;
        let int_op = |op: ast::BinOp| -> Option<hir::IntOp> {
            Some(match op {
                ast::BinOp::Add => hir::IntOp::Add,
                ast::BinOp::Sub => hir::IntOp::Sub,
                ast::BinOp::Mul => hir::IntOp::Mul,
                ast::BinOp::FloorDiv => hir::IntOp::FloorDiv,
                ast::BinOp::Mod => hir::IntOp::Mod,
                ast::BinOp::Pow => hir::IntOp::Pow,
                ast::BinOp::BitAnd => hir::IntOp::BitAnd,
                ast::BinOp::BitOr => hir::IntOp::BitOr,
                ast::BinOp::BitXor => hir::IntOp::BitXor,
                ast::BinOp::Shl => hir::IntOp::Shl,
                ast::BinOp::Shr => hir::IntOp::Shr,
                ast::BinOp::Div => return None,
            })
        };

        match ty {
            TypeId::INT => {
                if op == ast::BinOp::Div {
                    return hir::Expr {
                        kind: hir::ExprKind::IntDiv {
                            lhs: Box::new(lhs),
                            rhs: Box::new(rhs),
                        },
                        ty: TypeId::FLOAT,
                        span,
                    };
                }
                // `x ** 2` is a multiplication, not a runtime call.
                if op == ast::BinOp::Pow && matches!(rhs.kind, hir::ExprKind::Int(2)) {
                    return hir::Expr {
                        kind: hir::ExprKind::IntSquare(Box::new(lhs)),
                        ty: TypeId::INT,
                        span,
                    };
                }
                let op = int_op(op).expect("Div handled above");
                hir::Expr {
                    kind: hir::ExprKind::IntBin {
                        op,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                    ty: TypeId::INT,
                    span,
                }
            }
            TypeId::FLOAT => {
                let float_op = match op {
                    ast::BinOp::Add => hir::FloatOp::Add,
                    ast::BinOp::Sub => hir::FloatOp::Sub,
                    ast::BinOp::Mul => hir::FloatOp::Mul,
                    ast::BinOp::Div => hir::FloatOp::Div,
                    ast::BinOp::FloorDiv => hir::FloatOp::FloorDiv,
                    ast::BinOp::Mod => hir::FloatOp::Mod,
                    ast::BinOp::Pow => {
                        // DESIGN §4.3: `x ** 0.5` is a square root and
                        // `x ** 2.0` a multiplication.
                        let literal = match rhs.kind {
                            hir::ExprKind::Float(v) => Some(v),
                            _ => None,
                        };
                        let kind = if literal == Some(0.5) {
                            Some(hir::ExprKind::FloatSqrt(Box::new(lhs.clone())))
                        } else if literal == Some(2.0) {
                            Some(hir::ExprKind::FloatSquare(Box::new(lhs.clone())))
                        } else {
                            None
                        };
                        if let Some(kind) = kind {
                            return hir::Expr {
                                kind,
                                ty: TypeId::FLOAT,
                                span,
                            };
                        }
                        hir::FloatOp::Pow
                    }
                    ast::BinOp::BitAnd
                    | ast::BinOp::BitOr
                    | ast::BinOp::BitXor
                    | ast::BinOp::Shl
                    | ast::BinOp::Shr => {
                        self.bitwise_on_non_int(op, op_span, ty);
                        return self.error_expr(span);
                    }
                };
                hir::Expr {
                    kind: hir::ExprKind::FloatBin {
                        op: float_op,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                    ty: TypeId::FLOAT,
                    span,
                }
            }
            TypeId::STR if op == ast::BinOp::Add => hir::Expr {
                kind: hir::ExprKind::StrConcat {
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                ty: TypeId::STR,
                span,
            },
            _ if op == ast::BinOp::Add && self.types.as_list(ty).is_some() => hir::Expr {
                kind: hir::ExprKind::ListConcat {
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                ty,
                span,
            },
            _ => {
                let name = self.types.name(ty);
                if matches!(
                    op,
                    ast::BinOp::BitAnd
                        | ast::BinOp::BitOr
                        | ast::BinOp::BitXor
                        | ast::BinOp::Shl
                        | ast::BinOp::Shr
                ) {
                    self.bitwise_on_non_int(op, op_span, ty);
                } else {
                    self.error(
                        Diagnostic::error(format!(
                            "`{}` cannot be applied to `{name}` and `{name}`",
                            op.as_str()
                        ))
                        .with_label(
                            op_span,
                            format!("no `{}` operator for `{name}`", op.as_str()),
                        ),
                    );
                }
                self.error_expr(span)
            }
        }
    }

    fn bitwise_on_non_int(&mut self, op: ast::BinOp, op_span: Span, ty: TypeId) {
        let name = self.types.name(ty);
        self.error(
            Diagnostic::error(format!(
                "`{}` cannot be applied to `{name}` and `{name}`",
                op.as_str()
            ))
            .with_label(op_span, "bitwise operators require `int` operands")
            .with_note("see DESIGN.md section 3.6"),
        );
    }

    fn check_bool_op(
        &mut self,
        op: ast::BoolOp,
        lhs: &ast::Expr,
        rhs: &ast::Expr,
        span: Span,
    ) -> hir::Expr {
        let lhs = self.check_bool_operand(lhs, op);
        let rhs = self.check_bool_operand(rhs, op);
        if lhs.ty == TypeId::ERROR || rhs.ty == TypeId::ERROR {
            return self.error_expr(span);
        }
        let kind = match op {
            ast::BoolOp::And => hir::ExprKind::And {
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            ast::BoolOp::Or => hir::ExprKind::Or {
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
        };
        hir::Expr {
            kind,
            ty: TypeId::BOOL,
            span,
        }
    }

    fn check_bool_operand(&mut self, expr: &ast::Expr, op: ast::BoolOp) -> hir::Expr {
        let value = self.check_expr(expr);
        if value.ty == TypeId::BOOL || value.ty == TypeId::ERROR {
            return value;
        }
        let found = self.types.name(value.ty);
        self.error(
            Diagnostic::error(format!(
                "mismatched types: expected `bool`, found `{found}`"
            ))
            .with_label(
                value.span,
                format!("the operands of `{}` must be `bool`", op.as_str()),
            )
            .with_help("compare explicitly, e.g. `x != 0`")
            .with_note("typhoon has no truthiness, see DESIGN.md section 3.6"),
        );
        self.error_expr(value.span)
    }

    fn check_compare(
        &mut self,
        left: &ast::Expr,
        tail: &[ast::CompareTail],
        span: Span,
    ) -> hir::Expr {
        if tail.len() > 1 {
            self.check_expr(left);
            for t in tail {
                self.check_expr(&t.rhs);
            }
            self.unsupported(span, "a comparison chain", "M3");
            return self.error_expr(span);
        }
        let entry = &tail[0];
        let op = match entry.op {
            ast::CmpOp::Eq => hir::CmpOp::Eq,
            ast::CmpOp::Ne => hir::CmpOp::Ne,
            ast::CmpOp::Lt => hir::CmpOp::Lt,
            ast::CmpOp::Le => hir::CmpOp::Le,
            ast::CmpOp::Gt => hir::CmpOp::Gt,
            ast::CmpOp::Ge => hir::CmpOp::Ge,
            ast::CmpOp::Is => return self.check_is(left, &entry.rhs, false, entry.op_span, span),
            ast::CmpOp::IsNot => return self.check_is(left, &entry.rhs, true, entry.op_span, span),
            ast::CmpOp::In => return self.check_in(left, &entry.rhs, false, entry.op_span, span),
            ast::CmpOp::NotIn => return self.check_in(left, &entry.rhs, true, entry.op_span, span),
        };

        let lhs = self.check_expr(left);
        let rhs = self.check_expr_hint(&entry.rhs, Some(lhs.ty));
        if lhs.ty == TypeId::ERROR || rhs.ty == TypeId::ERROR {
            return self.error_expr(span);
        }
        if lhs.ty != rhs.ty {
            let (l, r) = (self.types.name(lhs.ty), self.types.name(rhs.ty));
            let mut diag =
                Diagnostic::error(format!("mismatched types: expected `{l}`, found `{r}`"))
                    .with_label(rhs.span, format!("expected `{l}`, found `{r}`"))
                    .with_secondary(lhs.span, format!("this is {}", a_type(&l)));
            if (lhs.ty == TypeId::FLOAT && rhs.ty == TypeId::INT)
                || (lhs.ty == TypeId::INT && rhs.ty == TypeId::FLOAT)
            {
                diag = diag.with_help("use `float(x)` to convert an `int` to a `float`");
            }
            self.error(diag);
            return self.error_expr(span);
        }

        let ordered = !matches!(op, hir::CmpOp::Eq | hir::CmpOp::Ne);
        let kind = match self.types.get(lhs.ty) {
            crate::types::Type::Int => hir::ExprKind::IntCmp {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            crate::types::Type::Float => hir::ExprKind::FloatCmp {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            crate::types::Type::Bool if !ordered => hir::ExprKind::BoolCmp {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            // `str` compares byte-wise, which for UTF-8 is code point order.
            crate::types::Type::Str => hir::ExprKind::StrCmp {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            crate::types::Type::List(_) | crate::types::Type::Tuple(_) if !ordered => {
                if !self.types.is_printable(lhs.ty) {
                    let name = self.types.name(lhs.ty);
                    self.error(
                        Diagnostic::error(format!("`{name}` values cannot be compared"))
                            .with_label(entry.op_span, "no `==` for this type")
                            .with_help(
                                "a container of class instances has no equality until the \
                                 repr/eq protocols arrive in M3",
                            ),
                    );
                    return self.error_expr(span);
                }
                hir::ExprKind::StructEq {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                }
            }
            crate::types::Type::Class(_) | crate::types::Type::Optional(_) => {
                let name = self.types.name(lhs.ty);
                self.error(
                    Diagnostic::error(format!(
                        "`{name}` values cannot be compared with `{}`",
                        entry.op.as_str()
                    ))
                    .with_label(entry.op_span, "no equality for class instances")
                    .with_help("use `is` to compare identity")
                    .with_note(
                        "an eq protocol for classes arrives in M3, see DESIGN.md section 3.8",
                    ),
                );
                return self.error_expr(span);
            }
            other => {
                let name = crate::types::type_name(other).to_string();
                let name = if name == "list" || name == "tuple" {
                    self.types.name(lhs.ty)
                } else {
                    name
                };
                self.error(
                    Diagnostic::error(format!(
                        "`{}` cannot be applied to `{name}` and `{name}`",
                        entry.op.as_str()
                    ))
                    .with_label(entry.op_span, format!("`{name}` values cannot be ordered"))
                    .with_help("only `==` and `!=` are defined for this type"),
                );
                return self.error_expr(span);
            }
        };
        hir::Expr {
            kind,
            ty: TypeId::BOOL,
            span,
        }
    }

    /// `a is b` / `a is not b`: reference identity, and the `is None` test that
    /// narrows a `C | None` (DESIGN §3.8).
    fn check_is(
        &mut self,
        left: &ast::Expr,
        right: &ast::Expr,
        negated: bool,
        op_span: Span,
        span: Span,
    ) -> hir::Expr {
        let left_is_none = matches!(left.kind, ast::ExprKind::None);
        let right_is_none = matches!(right.kind, ast::ExprKind::None);
        if left_is_none && right_is_none {
            self.check_expr(left);
            self.check_expr(right);
            return hir::Expr {
                kind: hir::ExprKind::Bool(!negated),
                ty: TypeId::BOOL,
                span,
            };
        }
        if left_is_none || right_is_none {
            let value = if left_is_none {
                self.check_expr(right)
            } else {
                self.check_expr(left)
            };
            if value.ty == TypeId::ERROR {
                return self.error_expr(span);
            }
            if self.types.class_of(value.ty).is_none() {
                let found = self.types.name(value.ty);
                self.error(
                    Diagnostic::error(format!("`{found}` can never be `None`"))
                        .with_label(value.span, format!("this is {}", a_type(&found)))
                        .with_help("only a class reference written `C | None` may be absent"),
                );
                return self.error_expr(span);
            }
            return hir::Expr {
                kind: hir::ExprKind::IsNone {
                    value: Box::new(value),
                    negated,
                },
                ty: TypeId::BOOL,
                span,
            };
        }

        let lhs = self.check_expr(left);
        let rhs = self.check_expr(right);
        if lhs.ty == TypeId::ERROR || rhs.ty == TypeId::ERROR {
            return self.error_expr(span);
        }
        match (self.types.class_of(lhs.ty), self.types.class_of(rhs.ty)) {
            (Some(a), Some(b)) if a == b => hir::Expr {
                kind: hir::ExprKind::RefEq {
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                    negated,
                },
                ty: TypeId::BOOL,
                span,
            },
            _ => {
                let (l, r) = (self.types.name(lhs.ty), self.types.name(rhs.ty));
                self.error(
                    Diagnostic::error(format!("`is` cannot compare `{l}` and `{r}`"))
                        .with_label(op_span, "`is` compares class references")
                        .with_help("use `==` to compare values")
                        .with_note("see DESIGN.md section 3.6"),
                );
                self.error_expr(span)
            }
        }
    }

    /// `v in xs` / `v in s` and their `not in` forms (DESIGN §3.6).
    fn check_in(
        &mut self,
        left: &ast::Expr,
        right: &ast::Expr,
        negated: bool,
        op_span: Span,
        span: Span,
    ) -> hir::Expr {
        let haystack = self.check_expr(right);
        match self.types.get(haystack.ty) {
            crate::types::Type::Error => {
                self.check_expr(left);
                self.error_expr(span)
            }
            crate::types::Type::List(elem) => {
                let needle = self.check_expr_hint(left, Some(elem));
                if needle.ty == TypeId::ERROR {
                    return self.error_expr(span);
                }
                if !matches!(
                    elem,
                    TypeId::INT | TypeId::FLOAT | TypeId::BOOL | TypeId::STR
                ) {
                    let name = self.types.name(haystack.ty);
                    self.error(
                        Diagnostic::error(format!("`in` is not defined for `{name}`"))
                            .with_label(op_span, "no membership test for this element type")
                            .with_help(
                                "`in` over a list needs `int`, `float`, `bool` or `str` elements",
                            ),
                    );
                    return self.error_expr(span);
                }
                if needle.ty != elem {
                    self.mismatch(needle.span, elem, needle.ty);
                    return self.error_expr(span);
                }
                hir::Expr {
                    kind: hir::ExprKind::Contains {
                        haystack: Box::new(haystack),
                        needle: Box::new(needle),
                        negated,
                    },
                    ty: TypeId::BOOL,
                    span,
                }
            }
            crate::types::Type::Str => {
                let needle = self.check_expr(left);
                if needle.ty == TypeId::ERROR {
                    return self.error_expr(span);
                }
                if needle.ty != TypeId::STR {
                    let found = self.types.name(needle.ty);
                    self.error(
                        Diagnostic::error(format!(
                            "`in` on a `str` tests for a substring, found `{found}`"
                        ))
                        .with_label(needle.span, format!("this is {}", a_type(&found))),
                    );
                    return self.error_expr(span);
                }
                hir::Expr {
                    kind: hir::ExprKind::Contains {
                        haystack: Box::new(haystack),
                        needle: Box::new(needle),
                        negated,
                    },
                    ty: TypeId::BOOL,
                    span,
                }
            }
            _ => {
                self.check_expr(left);
                let found = self.types.name(haystack.ty);
                self.error(
                    Diagnostic::error(format!("`in` is not defined for `{found}`"))
                        .with_label(op_span, format!("cannot search {}", a_type(&found)))
                        .with_help("`in` works on a `list<T>` and a `str`; dict and set are M3"),
                );
                self.error_expr(span)
            }
        }
    }

    // -- literals with an expected type ------------------------------------

    /// `[a, b]`: the element type comes from the elements, or from the
    /// annotation when the literal is empty (DESIGN §4.4).
    fn check_list_literal(
        &mut self,
        items: &[ast::Expr],
        expected: Option<TypeId>,
        span: Span,
    ) -> hir::Expr {
        let hint = expected.and_then(|ty| self.types.as_list(ty));
        if items.is_empty() {
            let Some(elem) = hint else {
                self.error(
                    Diagnostic::error("cannot infer the element type of an empty list")
                        .with_label(span, "this literal has no elements to infer from")
                        .with_help("annotate the variable, e.g. `xs: list<int> = []`")
                        .with_note(
                            "inference is local, so an empty container needs an annotation, \
                             see DESIGN.md section 4.4",
                        ),
                );
                return self.error_expr(span);
            };
            let ty = self.types.list_of(elem);
            return hir::Expr {
                kind: hir::ExprKind::ListNew {
                    elem,
                    items: Vec::new(),
                },
                ty,
                span,
            };
        }

        // The annotation wins when there is one, so
        // `xs: list<Node | None> = [n]` widens instead of failing. Otherwise
        // the first element that has a type decides, and that type is then the
        // hint for the elements after it — which is what makes the inner `[]`
        // of `[[1], []]` inferable.
        let mut running = hint;
        let mut checked = Vec::with_capacity(items.len());
        for item in items {
            let value = self.check_expr_hint(item, running);
            if running.is_none() && value.ty != TypeId::ERROR {
                running = Some(value.ty);
            }
            checked.push(value);
        }
        let Some(elem) = running else {
            return self.error_expr(span);
        };
        if elem == TypeId::UNIT {
            self.error(
                Diagnostic::error("a list element cannot have type `None`")
                    .with_label(span, "these expressions produce no value"),
            );
            return self.error_expr(span);
        }
        let mut ok = true;
        let mut lowered = Vec::with_capacity(checked.len());
        for value in checked {
            if !self.assignable(elem, value.ty) {
                let (e, f) = (self.types.name(elem), self.types.name(value.ty));
                self.error(
                    Diagnostic::error(format!("mismatched types: expected `{e}`, found `{f}`"))
                        .with_label(value.span, format!("expected `{e}`, found `{f}`"))
                        .with_note("every element of a list literal must have the same type"),
                );
                ok = false;
                continue;
            }
            lowered.push(self.coerce(value, elem));
        }
        if !ok {
            return self.error_expr(span);
        }
        let ty = self.types.list_of(elem);
        hir::Expr {
            kind: hir::ExprKind::ListNew {
                elem,
                items: lowered,
            },
            ty,
            span,
        }
    }

    /// `(a, b)`: a value aggregate (DESIGN §4.2). M2 has no unit tuple.
    fn check_tuple_literal(
        &mut self,
        items: &[ast::Expr],
        expected: Option<TypeId>,
        span: Span,
    ) -> hir::Expr {
        if items.is_empty() {
            self.error(
                Diagnostic::error("the empty tuple `()` has no type")
                    .with_label(span, "a tuple needs at least one member")
                    .with_help("use `None` for the absence of a value"),
            );
            return self.error_expr(span);
        }
        let hint: Option<Vec<TypeId>> = expected
            .and_then(|ty| self.types.as_tuple(ty))
            .filter(|members| members.len() == items.len())
            .map(<[TypeId]>::to_vec);
        let mut members = Vec::with_capacity(items.len());
        let mut lowered = Vec::with_capacity(items.len());
        for (index, item) in items.iter().enumerate() {
            let want = hint.as_ref().map(|h| h[index]);
            let value = self.check_expr_hint(item, want);
            if value.ty == TypeId::ERROR {
                return self.error_expr(span);
            }
            if value.ty == TypeId::UNIT {
                self.error(
                    Diagnostic::error("a tuple member cannot have type `None`")
                        .with_label(value.span, "this expression produces no value"),
                );
                return self.error_expr(span);
            }
            let value = match want {
                Some(want) if self.assignable(want, value.ty) => self.coerce(value, want),
                _ => value,
            };
            members.push(value.ty);
            lowered.push(value);
        }
        let ty = self.types.tuple_of(&members);
        hir::Expr {
            kind: hir::ExprKind::TupleNew(lowered),
            ty,
            span,
        }
    }

    // -- fields and indexing ------------------------------------------------

    /// Resolves `obj.name` to the class and field index it reads, reporting
    /// what went wrong otherwise.
    pub(crate) fn resolve_field(
        &mut self,
        obj: &hir::Expr,
        name: &ast::Ident,
    ) -> Option<(ClassId, u32)> {
        match self.types.get(obj.ty) {
            Type::Error => None,
            Type::Class(class) => match self.types.field_index(class, name.as_str()) {
                Some(index) => Some((class, index)),
                None => {
                    let class_name = self.types.class(class).name.clone();
                    let known = self.field_list(class);
                    let mut diag = Diagnostic::error(format!(
                        "`{class_name}` has no field `{}`",
                        name.as_str()
                    ))
                    .with_label(name.span, "unknown field");
                    if self.methods.contains_key(&(class, name.name.clone())) {
                        diag = diag.with_help(format!("`{}` is a method; call it", name.as_str()));
                    } else if !known.is_empty() {
                        diag = diag.with_help(format!("`{class_name}` has {known}"));
                    }
                    self.error(diag);
                    None
                }
            },
            Type::Optional(_) => {
                self.require_narrowed(obj.span, obj.ty);
                None
            }
            _ => {
                let found = self.types.name(obj.ty);
                self.error(
                    Diagnostic::error(format!("`{found}` has no field `{}`", name.as_str()))
                        .with_label(name.span, format!("this is {}", a_type(&found)))
                        .with_help("only class instances have fields"),
                );
                None
            }
        }
    }

    /// `x`, `x and y`: the fields of a class, for a diagnostic.
    fn field_list(&self, class: ClassId) -> String {
        let names: Vec<String> = self
            .types
            .class(class)
            .fields
            .iter()
            .map(|f| format!("`{}`", f.name))
            .collect();
        match names.len() {
            0 => String::new(),
            1 => format!("one field, {}", names[0]),
            _ => format!("the fields {}", names.join(", ")),
        }
    }

    fn check_attribute(&mut self, base: &ast::Expr, attr: &ast::Ident, span: Span) -> hir::Expr {
        let obj = self.check_expr(base);
        let Some((class, field)) = self.resolve_field(&obj, attr) else {
            return self.error_expr(span);
        };
        let ty = self.types.class(class).fields[field as usize].ty;
        hir::Expr {
            kind: hir::ExprKind::GetField {
                obj: Box::new(obj),
                class,
                field,
            },
            ty,
            span,
        }
    }

    /// `xs[i]` and `t[k]`. `str` has no positional indexing in M2
    /// (DESIGN §9 item 5 is still open).
    fn check_index(&mut self, base: &ast::Expr, index: &ast::Expr, span: Span) -> hir::Expr {
        let value = self.check_expr(base);
        match self.types.get(value.ty) {
            Type::Error => {
                self.check_expr(index);
                self.error_expr(span)
            }
            Type::List(elem) => {
                let idx = self.check_expr(index);
                if idx.ty != TypeId::ERROR && idx.ty != TypeId::INT {
                    let found = self.types.name(idx.ty);
                    self.error(
                        Diagnostic::error(format!(
                            "a list index must be an `int`, found `{found}`"
                        ))
                        .with_label(idx.span, format!("this is {}", a_type(&found))),
                    );
                    return self.error_expr(span);
                }
                hir::Expr {
                    kind: hir::ExprKind::ListGet {
                        list: Box::new(value),
                        index: Box::new(idx),
                    },
                    ty: elem,
                    span,
                }
            }
            Type::Tuple(members) => {
                let arity = self.types.tuple_members(members).len();
                let member_tys = self.types.tuple_members(members).to_vec();
                let Some(constant) = constant_index(index) else {
                    self.check_expr(index);
                    self.error(
                        Diagnostic::error("a tuple index must be a constant")
                            .with_label(index.span, "expected an integer literal")
                            .with_help(
                                "a tuple's members may have different types, so the index \
                                 has to be known at compile time",
                            ),
                    );
                    return self.error_expr(span);
                };
                let resolved = if constant < 0 {
                    constant + arity as i64
                } else {
                    constant
                };
                if resolved < 0 || resolved >= arity as i64 {
                    let name = self.types.name(value.ty);
                    self.error(
                        Diagnostic::error(format!(
                            "tuple index {constant} is out of range for `{name}`"
                        ))
                        .with_label(index.span, format!("this tuple has {arity} member(s)")),
                    );
                    return self.error_expr(span);
                }
                hir::Expr {
                    kind: hir::ExprKind::TupleGet {
                        tuple: Box::new(value),
                        index: resolved as u32,
                    },
                    ty: member_tys[resolved as usize],
                    span,
                }
            }
            Type::Str => {
                self.check_expr(index);
                self.error(
                    Diagnostic::error(
                        "indexing a str by position is not supported yet; \
                         iterate over it or use find/split",
                    )
                    .with_label(span, "`str` has no positional indexing")
                    .with_note(
                        "indexing UTF-8 by code point is O(n), so the semantics are \
                         still open, see DESIGN.md section 9 item 5",
                    ),
                );
                self.error_expr(span)
            }
            Type::Optional(_) => {
                self.check_expr(index);
                self.require_narrowed(value.span, value.ty);
                self.error_expr(span)
            }
            _ => {
                self.check_expr(index);
                let found = self.types.name(value.ty);
                self.error(
                    Diagnostic::error(format!("`{found}` cannot be indexed"))
                        .with_label(base.span, format!("this is {}", a_type(&found)))
                        .with_help("only `list<T>` and `tuple<...>` support `[...]`"),
                );
                self.error_expr(span)
            }
        }
    }

    // -- constructors and methods -------------------------------------------

    /// `C(field=value, …)`: the generated keyword constructor (DESIGN §3.8).
    fn check_construct(
        &mut self,
        class: ClassId,
        callee_span: Span,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let class_name = self.types.class(class).name.clone();
        let fields: Vec<(String, TypeId)> = self
            .types
            .class(class)
            .fields
            .iter()
            .map(|f| (f.name.clone(), f.ty))
            .collect();
        let defaults = self
            .classes
            .get(&class_name)
            .map(|c| c.defaults.clone())
            .unwrap_or_default();

        let mut slots: Vec<Option<hir::Expr>> = (0..fields.len()).map(|_| None).collect();
        let mut eval_order: Vec<u32> = Vec::with_capacity(args.len());
        let mut ok = true;
        for arg in args {
            let Some(name) = &arg.name else {
                self.check_expr(&arg.value);
                self.error(
                    Diagnostic::error(format!(
                        "the constructor of `{class_name}` only takes keyword arguments"
                    ))
                    .with_label(arg.span, "name the field, e.g. `x=1`")
                    .with_note("the generated constructor is keyword-only, see DESIGN.md 3.8"),
                );
                ok = false;
                continue;
            };
            let Some(index) = fields.iter().position(|f| f.0 == name.name) else {
                self.check_expr(&arg.value);
                let known = self.field_list(class);
                let mut diag =
                    Diagnostic::error(format!("`{class_name}` has no field `{}`", name.as_str()))
                        .with_label(name.span, "unknown field")
                        .with_secondary(callee_span, format!("`{class_name}` constructed here"));
                if !known.is_empty() {
                    diag = diag.with_help(format!("`{class_name}` has {known}"));
                }
                self.error(diag);
                ok = false;
                continue;
            };
            if slots[index].is_some() {
                self.check_expr(&arg.value);
                self.error(
                    Diagnostic::error(format!("field `{}` specified twice", name.as_str()))
                        .with_label(name.span, "already supplied"),
                );
                ok = false;
                continue;
            }
            let want = fields[index].1;
            let value = self.check_expr_hint(&arg.value, Some(want));
            if !self.assignable(want, value.ty) {
                self.mismatch(value.span, want, value.ty);
                ok = false;
                continue;
            }
            slots[index] = Some(self.coerce(value, want));
            eval_order.push(index as u32);
        }

        let mut missing = Vec::new();
        for (index, slot) in slots.iter_mut().enumerate() {
            if slot.is_some() {
                continue;
            }
            match defaults.get(index).and_then(|d| d.clone()) {
                Some(default) => {
                    let expr = default.to_expr(callee_span);
                    *slot = Some(self.coerce(expr, fields[index].1));
                }
                None => missing.push(fields[index].0.clone()),
            }
        }
        if !missing.is_empty() && ok {
            let list = missing
                .iter()
                .map(|m| format!("`{m}`"))
                .collect::<Vec<_>>()
                .join(", ");
            self.error(
                Diagnostic::error(format!(
                    "missing field{} {list} in the constructor of `{class_name}`",
                    if missing.len() == 1 { "" } else { "s" }
                ))
                .with_label(span, "every field without a default must be supplied")
                .with_help(
                    "write `{class_name}(field=value, …)`".replace("{class_name}", &class_name),
                ),
            );
            ok = false;
        }
        if !ok {
            return self.error_expr(span);
        }
        let ty = self.types.class_ty(class);
        hir::Expr {
            kind: hir::ExprKind::New {
                class,
                fields: slots
                    .into_iter()
                    .map(|s| s.expect("filled above"))
                    .collect(),
                eval_order,
            },
            ty,
            span,
        }
    }

    /// `recv.name(args)`: a class method, or one of the builtin `list` / `str`
    /// methods of DESIGN §4.6.
    fn check_method_call(
        &mut self,
        base: &ast::Expr,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let recv = self.check_expr(base);
        match self.types.get(recv.ty) {
            Type::Error => {
                for arg in args {
                    self.check_expr(&arg.value);
                }
                self.error_expr(span)
            }
            Type::Class(class) => match self.methods.get(&(class, name.name.clone())).copied() {
                Some(func) => self.check_user_call(func, name.span, args, span, Some(recv)),
                None => {
                    for arg in args {
                        self.check_expr(&arg.value);
                    }
                    let class_name = self.types.class(class).name.clone();
                    let mut diag = Diagnostic::error(format!(
                        "`{class_name}` has no method `{}`",
                        name.as_str()
                    ))
                    .with_label(name.span, "unknown method");
                    if self.types.field_index(class, name.as_str()).is_some() {
                        diag = diag
                            .with_help(format!("`{}` is a field; drop the `()`", name.as_str()));
                    }
                    self.error(diag);
                    self.error_expr(span)
                }
            },
            Type::List(elem) => self.check_list_method(recv, elem, name, args, span),
            Type::Str => self.check_str_method(recv, name, args, span),
            Type::Optional(_) => {
                for arg in args {
                    self.check_expr(&arg.value);
                }
                self.require_narrowed(recv.span, recv.ty);
                self.error_expr(span)
            }
            _ => {
                for arg in args {
                    self.check_expr(&arg.value);
                }
                let found = self.types.name(recv.ty);
                self.error(
                    Diagnostic::error(format!("`{found}` has no method `{}`", name.as_str()))
                        .with_label(name.span, format!("`{found}` has no methods"))
                        .with_help("only `list`, `str` and class instances have methods"),
                );
                self.error_expr(span)
            }
        }
    }

    /// Checks the fixed argument list of a builtin method, returning the
    /// checked arguments when the arity matches.
    fn builtin_args(
        &mut self,
        receiver: &str,
        method: &str,
        args: &[ast::Arg],
        wanted: &[TypeId],
        span: Span,
    ) -> Option<Vec<hir::Expr>> {
        if let Some(kw) = args.iter().find(|a| a.name.is_some()) {
            for arg in args {
                self.check_expr(&arg.value);
            }
            self.error(
                Diagnostic::error(format!(
                    "`{receiver}.{method}` does not take keyword arguments"
                ))
                .with_label(kw.span, "unexpected keyword argument"),
            );
            return None;
        }
        if args.len() != wanted.len() {
            for arg in args {
                self.check_expr(&arg.value);
            }
            self.error(
                Diagnostic::error(format!(
                    "`{receiver}.{method}` takes {} argument{} but {} {} supplied",
                    wanted.len(),
                    if wanted.len() == 1 { "" } else { "s" },
                    args.len(),
                    if args.len() == 1 { "was" } else { "were" }
                ))
                .with_label(span, "wrong number of arguments"),
            );
            return None;
        }
        let mut checked = Vec::with_capacity(args.len());
        let mut ok = true;
        for (arg, want) in args.iter().zip(wanted) {
            let value = self.check_expr_hint(&arg.value, Some(*want));
            if !self.assignable(*want, value.ty) {
                self.mismatch(value.span, *want, value.ty);
                ok = false;
                continue;
            }
            checked.push(self.coerce(value, *want));
        }
        ok.then_some(checked)
    }

    fn check_list_method(
        &mut self,
        recv: hir::Expr,
        elem: TypeId,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let list_name = self.types.name(recv.ty);
        let (kind, ty) = match name.as_str() {
            "append" => {
                let Some(mut checked) =
                    self.builtin_args(&list_name, "append", args, &[elem], span)
                else {
                    return self.error_expr(span);
                };
                (
                    hir::ExprKind::ListAppend {
                        list: Box::new(recv),
                        value: Box::new(checked.remove(0)),
                    },
                    TypeId::UNIT,
                )
            }
            "pop" => {
                if self
                    .builtin_args(&list_name, "pop", args, &[], span)
                    .is_none()
                {
                    return self.error_expr(span);
                }
                (hir::ExprKind::ListPop(Box::new(recv)), elem)
            }
            "insert" => {
                let Some(mut checked) =
                    self.builtin_args(&list_name, "insert", args, &[TypeId::INT, elem], span)
                else {
                    return self.error_expr(span);
                };
                let value = checked.remove(1);
                let index = checked.remove(0);
                (
                    hir::ExprKind::ListInsert {
                        list: Box::new(recv),
                        index: Box::new(index),
                        value: Box::new(value),
                    },
                    TypeId::UNIT,
                )
            }
            "clear" => {
                if self
                    .builtin_args(&list_name, "clear", args, &[], span)
                    .is_none()
                {
                    return self.error_expr(span);
                }
                (hir::ExprKind::ListClear(Box::new(recv)), TypeId::UNIT)
            }
            other => {
                for arg in args {
                    self.check_expr(&arg.value);
                }
                self.error(
                    Diagnostic::error(format!("`{list_name}` has no method `{other}`"))
                        .with_label(name.span, "unknown method")
                        .with_help("`list` has `append`, `pop`, `insert` and `clear`")
                        .with_note("see DESIGN.md section 4.6"),
                );
                return self.error_expr(span);
            }
        };
        hir::Expr { kind, ty, span }
    }

    fn check_str_method(
        &mut self,
        recv: hir::Expr,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let strs = self.types.list_of(TypeId::STR);
        let (op, wanted, ret): (hir::StrMethod, Vec<TypeId>, TypeId) = match name.as_str() {
            "upper" => (hir::StrMethod::Upper, vec![], TypeId::STR),
            "lower" => (hir::StrMethod::Lower, vec![], TypeId::STR),
            "strip" => (hir::StrMethod::Strip, vec![], TypeId::STR),
            "split" => (hir::StrMethod::Split, vec![TypeId::STR], strs),
            "join" => (hir::StrMethod::Join, vec![strs], TypeId::STR),
            "startswith" => (hir::StrMethod::StartsWith, vec![TypeId::STR], TypeId::BOOL),
            "endswith" => (hir::StrMethod::EndsWith, vec![TypeId::STR], TypeId::BOOL),
            "find" => (hir::StrMethod::Find, vec![TypeId::STR], TypeId::INT),
            "replace" => (
                hir::StrMethod::Replace,
                vec![TypeId::STR, TypeId::STR],
                TypeId::STR,
            ),
            other => {
                for arg in args {
                    self.check_expr(&arg.value);
                }
                self.error(
                    Diagnostic::error(format!("`str` has no method `{other}`"))
                        .with_label(name.span, "unknown method")
                        .with_help(
                            "`str` has `upper`, `lower`, `strip`, `split`, `join`, \
                             `startswith`, `endswith`, `find` and `replace`",
                        )
                        .with_note("see DESIGN.md section 4.6"),
                );
                return self.error_expr(span);
            }
        };
        let Some(checked) = self.builtin_args("str", op.as_str(), args, &wanted, span) else {
            return self.error_expr(span);
        };
        hir::Expr {
            kind: hir::ExprKind::StrMethod {
                op,
                recv: Box::new(recv),
                args: checked,
            },
            ty: ret,
            span,
        }
    }

    // -- calls ------------------------------------------------------------

    fn check_call(
        &mut self,
        callee: &ast::Expr,
        type_args: Option<&[ast::TypeExpr]>,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        if type_args.is_some() {
            for arg in args {
                self.check_expr(&arg.value);
            }
            self.unsupported(span, "a generic call", "M3");
            return self.error_expr(span);
        }
        let name = match &callee.kind {
            ast::ExprKind::Name(name) => name,
            ast::ExprKind::Attribute { base, attr } => {
                return self.check_method_call(base, attr, args, span);
            }
            ast::ExprKind::Error => return self.error_expr(span),
            _ => {
                for arg in args {
                    self.check_expr(&arg.value);
                }
                self.error(
                    Diagnostic::error("this expression is not callable")
                        .with_label(callee.span, "not a function"),
                );
                return self.error_expr(span);
            }
        };

        if let Some(local) = self.scope.get(name.as_str()).copied() {
            let ty = self.types.name(self.locals[local.index()].ty).to_string();
            for arg in args {
                self.check_expr(&arg.value);
            }
            self.error(
                Diagnostic::error(format!("cannot call a value of type `{ty}`"))
                    .with_label(callee.span, format!("`{}` is a variable", name.as_str())),
            );
            return self.error_expr(span);
        }
        if let Some(class) = self.classes.get(name.as_str()).map(|c| c.id) {
            return self.check_construct(class, callee.span, args, span);
        }
        if let Some(func) = self.fn_index.get(name.as_str()).copied() {
            return self.check_user_call(func, callee.span, args, span, None);
        }
        if is_builtin(name.as_str()) {
            return self.check_builtin_call(name, args, span);
        }
        for arg in args {
            self.check_expr(&arg.value);
        }
        self.error(
            Diagnostic::error(format!(
                "cannot find function `{}` in this scope",
                name.as_str()
            ))
            .with_label(callee.span, "not found in this scope"),
        );
        self.error_expr(span)
    }

    /// A call to a user function, or — when `receiver` is `Some` — to a method,
    /// whose `self` parameter is filled in from the receiver and evaluated
    /// first (DESIGN §3.8).
    fn check_user_call(
        &mut self,
        func: hir::FuncId,
        callee_span: Span,
        args: &[ast::Arg],
        span: Span,
        receiver: Option<hir::Expr>,
    ) -> hir::Expr {
        let sig_name = self.sigs[func.index()].name.clone();
        let ret = self.sigs[func.index()].ret;
        let mut params: Vec<(String, TypeId, Span, Option<crate::check::ConstValue>)> = self.sigs
            [func.index()]
        .params
        .iter()
        .map(|p| (p.name.clone(), p.ty, p.span, p.default.clone()))
        .collect();
        if receiver.is_some() && !params.is_empty() {
            params.remove(0);
        }

        let Some(matched) = self.match_args(&sig_name, callee_span, &params, args, span) else {
            return self.error_expr(span);
        };
        let (args, eval_order) = match receiver {
            Some(recv) => {
                let mut all = vec![recv];
                all.extend(matched.args);
                let mut order = vec![0u32];
                order.extend(matched.eval_order.iter().map(|i| i + 1));
                (all, order)
            }
            None => (matched.args, matched.eval_order),
        };
        hir::Expr {
            kind: hir::ExprKind::Call {
                func,
                args,
                eval_order,
            },
            ty: ret,
            span,
        }
    }

    /// Matches call arguments to parameters by position and name, filling in
    /// defaults (DESIGN §3.2).
    fn match_args(
        &mut self,
        fn_name: &str,
        callee_span: Span,
        params: &[(String, TypeId, Span, Option<crate::check::ConstValue>)],
        args: &[ast::Arg],
        span: Span,
    ) -> Option<CallArgs> {
        let mut slots: Vec<Option<hir::Expr>> = (0..params.len()).map(|_| None).collect();
        let mut eval_order: Vec<u32> = Vec::with_capacity(args.len());
        let mut ok = true;
        let mut next_positional = 0usize;

        for arg in args {
            match &arg.name {
                None => {
                    if next_positional >= params.len() {
                        self.error(
                            Diagnostic::error(format!(
                                "`{fn_name}` takes {} argument{} but {} {} supplied",
                                params.len(),
                                if params.len() == 1 { "" } else { "s" },
                                args.len(),
                                if args.len() == 1 { "was" } else { "were" }
                            ))
                            .with_label(arg.span, "unexpected argument")
                            .with_secondary(callee_span, format!("`{fn_name}` declared here")),
                        );
                        self.check_expr(&arg.value);
                        ok = false;
                        continue;
                    }
                    let idx = next_positional;
                    next_positional += 1;
                    let want = params[idx].1;
                    let value = self.check_expr_hint(&arg.value, Some(want));
                    if !self.assignable(want, value.ty) {
                        self.mismatch(value.span, want, value.ty);
                        ok = false;
                        continue;
                    }
                    slots[idx] = Some(self.coerce(value, want));
                    eval_order.push(idx as u32);
                }
                Some(name) => {
                    let Some(idx) = params.iter().position(|p| p.0 == name.as_str()) else {
                        self.error(
                            Diagnostic::error(format!(
                                "`{fn_name}` has no parameter named `{}`",
                                name.as_str()
                            ))
                            .with_label(name.span, "unknown keyword argument")
                            .with_secondary(callee_span, format!("`{fn_name}` declared here")),
                        );
                        self.check_expr(&arg.value);
                        ok = false;
                        continue;
                    };
                    if slots[idx].is_some() {
                        self.error(
                            Diagnostic::error(format!(
                                "argument `{}` specified twice",
                                name.as_str()
                            ))
                            .with_label(name.span, "already supplied"),
                        );
                        self.check_expr(&arg.value);
                        ok = false;
                        continue;
                    }
                    let want = params[idx].1;
                    let value = self.check_expr_hint(&arg.value, Some(want));
                    if !self.assignable(want, value.ty) {
                        self.mismatch(value.span, want, value.ty);
                        ok = false;
                        continue;
                    }
                    slots[idx] = Some(self.coerce(value, want));
                    eval_order.push(idx as u32);
                }
            }
        }

        let mut missing = Vec::new();
        for (idx, slot) in slots.iter_mut().enumerate() {
            if slot.is_some() {
                continue;
            }
            match params[idx].3.clone() {
                Some(default) => {
                    let expr = default.to_expr(params[idx].2);
                    *slot = Some(self.coerce(expr, params[idx].1));
                }
                None => missing.push(params[idx].0.clone()),
            }
        }
        if !missing.is_empty() && ok {
            let list = missing
                .iter()
                .map(|m| format!("`{m}`"))
                .collect::<Vec<_>>()
                .join(", ");
            self.error(
                Diagnostic::error(format!(
                    "missing argument{} {list} in the call to `{fn_name}`",
                    if missing.len() == 1 { "" } else { "s" }
                ))
                .with_label(span, "call is missing arguments")
                .with_secondary(callee_span, format!("`{fn_name}` declared here")),
            );
            ok = false;
        }
        if !ok {
            return None;
        }
        Some(CallArgs {
            args: slots
                .into_iter()
                .map(|s| s.expect("filled above"))
                .collect(),
            eval_order,
        })
    }

    // -- builtins ---------------------------------------------------------

    fn check_builtin_call(
        &mut self,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let builtin = name.as_str();
        if let Some(kw) = args.iter().find(|a| a.name.is_some()) {
            for arg in args {
                self.check_expr(&arg.value);
            }
            self.error(
                Diagnostic::error(format!(
                    "the builtin `{builtin}` does not take keyword arguments"
                ))
                .with_label(kw.span, "unexpected keyword argument"),
            );
            return self.error_expr(span);
        }
        match builtin {
            "print" => {
                let mut values = Vec::with_capacity(args.len());
                for arg in args {
                    let value = self.check_expr(&arg.value);
                    if value.ty == TypeId::UNIT {
                        self.error(
                            Diagnostic::error("cannot print a value of type `None`")
                                .with_label(value.span, "this expression produces no value"),
                        );
                        continue;
                    }
                    if value.ty == TypeId::ERROR {
                        continue;
                    }
                    if !self.types.is_printable(value.ty) {
                        self.not_printable(&value);
                        continue;
                    }
                    values.push(value);
                }
                hir::Expr {
                    kind: hir::ExprKind::Print(values),
                    ty: TypeId::UNIT,
                    span,
                }
            }
            "float" | "int" | "abs" => {
                let Some(value) = self.one_arg(builtin, args, span) else {
                    return self.error_expr(span);
                };
                if value.ty == TypeId::ERROR {
                    return self.error_expr(span);
                }
                let (kind, ty) = match (builtin, value.ty) {
                    ("float", TypeId::INT) => {
                        (hir::ExprKind::IntToFloat(Box::new(value)), TypeId::FLOAT)
                    }
                    // An explicit conversion of a `float` is a contraction
                    // barrier (DESIGN §4.3), not a no-op.
                    ("float", TypeId::FLOAT) => {
                        (hir::ExprKind::FloatFence(Box::new(value)), TypeId::FLOAT)
                    }
                    ("int", TypeId::FLOAT) => {
                        (hir::ExprKind::FloatToInt(Box::new(value)), TypeId::INT)
                    }
                    ("int", TypeId::INT) => (value.kind, TypeId::INT),
                    ("abs", TypeId::INT) => (hir::ExprKind::IntAbs(Box::new(value)), TypeId::INT),
                    ("abs", TypeId::FLOAT) => {
                        (hir::ExprKind::FloatAbs(Box::new(value)), TypeId::FLOAT)
                    }
                    _ => {
                        let found = self.types.name(value.ty);
                        self.error(
                            Diagnostic::error(format!(
                                "`{builtin}` cannot be applied to a value of type `{found}`"
                            ))
                            .with_label(value.span, format!("this is {}", a_type(&found)))
                            .with_help("`float`, `int` and `abs` take an `int` or a `float`"),
                        );
                        return self.error_expr(span);
                    }
                };
                hir::Expr { kind, ty, span }
            }
            "min" | "max" => {
                if args.len() != 2 {
                    for arg in args {
                        self.check_expr(&arg.value);
                    }
                    self.error(
                        Diagnostic::error(format!(
                            "`{builtin}` takes 2 arguments but {} were supplied",
                            args.len()
                        ))
                        .with_label(span, "wrong number of arguments")
                        .with_note("`min` and `max` over a container arrive in M2"),
                    );
                    return self.error_expr(span);
                }
                let lhs = self.check_expr(&args[0].value);
                let rhs = self.check_expr(&args[1].value);
                if lhs.ty == TypeId::ERROR || rhs.ty == TypeId::ERROR {
                    return self.error_expr(span);
                }
                if lhs.ty != rhs.ty {
                    self.mismatch(rhs.span, lhs.ty, rhs.ty);
                    return self.error_expr(span);
                }
                let op = if builtin == "min" {
                    hir::MinMax::Min
                } else {
                    hir::MinMax::Max
                };
                let kind = match lhs.ty {
                    TypeId::INT => hir::ExprKind::IntMinMax {
                        op,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                    TypeId::FLOAT => hir::ExprKind::FloatMinMax {
                        op,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                    other => {
                        let found = self.types.name(other);
                        self.error(
                            Diagnostic::error(format!(
                                "`{builtin}` cannot be applied to values of type `{found}`"
                            ))
                            .with_label(span, "expected `int` or `float`"),
                        );
                        return self.error_expr(span);
                    }
                };
                let ty = if lhs_ty_is_float(&kind) {
                    TypeId::FLOAT
                } else {
                    TypeId::INT
                };
                hir::Expr { kind, ty, span }
            }
            "range" => {
                for arg in args {
                    self.check_expr(&arg.value);
                }
                self.error(
                    Diagnostic::error("`range` can only be used as the iterable of a `for` loop")
                        .with_label(span, "not a value")
                        .with_note(
                            "`range` compiles to a counting loop and never builds an object, \
                             see DESIGN.md section 3.5",
                        ),
                );
                self.error_expr(span)
            }
            "len" => {
                let Some(value) = self.one_arg(builtin, args, span) else {
                    return self.error_expr(span);
                };
                if value.ty == TypeId::ERROR {
                    return self.error_expr(span);
                }
                match self.types.get(value.ty) {
                    Type::Str => hir::Expr {
                        kind: hir::ExprKind::StrLen(Box::new(value)),
                        ty: TypeId::INT,
                        span,
                    },
                    Type::List(_) => hir::Expr {
                        kind: hir::ExprKind::ListLen(Box::new(value)),
                        ty: TypeId::INT,
                        span,
                    },
                    // A tuple's arity is part of its type, so `len(t)` folds.
                    Type::Tuple(members) => {
                        let arity = self.types.tuple_members(members).len() as i64;
                        hir::Expr {
                            kind: hir::ExprKind::Int(arity),
                            ty: TypeId::INT,
                            span,
                        }
                    }
                    _ => {
                        let found = self.types.name(value.ty);
                        self.error(
                            Diagnostic::error(format!("`len` is not defined for `{found}`"))
                                .with_label(value.span, format!("this is {}", a_type(&found)))
                                .with_help("`len` takes a `str`, a `list<T>` or a `tuple<...>`"),
                        );
                        self.error_expr(span)
                    }
                }
            }
            "str" => {
                let Some(value) = self.one_arg(builtin, args, span) else {
                    return self.error_expr(span);
                };
                if value.ty == TypeId::ERROR {
                    return self.error_expr(span);
                }
                match value.ty {
                    // `str(s)` on a `str` is the identity.
                    TypeId::STR => value,
                    TypeId::INT | TypeId::FLOAT | TypeId::BOOL => hir::Expr {
                        kind: hir::ExprKind::StrFrom(Box::new(value)),
                        ty: TypeId::STR,
                        span,
                    },
                    _ => {
                        let found = self.types.name(value.ty);
                        self.error(
                            Diagnostic::error(format!("`str` is not defined for `{found}`"))
                                .with_label(value.span, format!("this is {}", a_type(&found)))
                                .with_help("`str` takes an `int`, a `float`, a `bool` or a `str`")
                                .with_note(
                                    "a repr protocol for containers and classes arrives in M3",
                                ),
                        );
                        self.error_expr(span)
                    }
                }
            }
            other => {
                for arg in args {
                    self.check_expr(&arg.value);
                }
                let what = format!("the builtin `{other}`");
                self.unsupported(span, &what, "M3");
                self.error_expr(span)
            }
        }
    }

    /// Reports `print(x)` where `x` is, or contains, a class instance.
    fn not_printable(&mut self, value: &hir::Expr) {
        let found = self.types.name(value.ty);
        self.error(
            Diagnostic::error(format!("cannot print {}", a_type(&found)))
                .with_label(value.span, format!("this is {}", a_type(&found)))
                .with_help("format the fields explicitly; a repr protocol arrives in M3")
                .with_note("see DESIGN.md section 3.8"),
        );
    }

    fn one_arg(&mut self, builtin: &str, args: &[ast::Arg], span: Span) -> Option<hir::Expr> {
        if args.len() != 1 {
            for arg in args {
                self.check_expr(&arg.value);
            }
            self.error(
                Diagnostic::error(format!(
                    "`{builtin}` takes 1 argument but {} were supplied",
                    args.len()
                ))
                .with_label(span, "wrong number of arguments"),
            );
            return None;
        }
        Some(self.check_expr(&args[0].value))
    }

    // -- f-strings --------------------------------------------------------

    fn check_fstring(&mut self, parts: &[ast::FStringPart], span: Span) -> hir::Expr {
        let mut out = Vec::with_capacity(parts.len());
        for part in parts {
            match part {
                ast::FStringPart::Literal { value, .. } => {
                    if !value.is_empty() {
                        out.push(hir::FStringPart::Literal(value.clone()));
                    }
                }
                ast::FStringPart::Expr {
                    expr, spec, span, ..
                } => {
                    let value = self.check_expr(expr);
                    if value.ty == TypeId::ERROR {
                        continue;
                    }
                    if value.ty == TypeId::UNIT {
                        self.error(
                            Diagnostic::error("cannot interpolate a value of type `None`")
                                .with_label(value.span, "this expression produces no value"),
                        );
                        continue;
                    }
                    if !matches!(
                        value.ty,
                        TypeId::INT | TypeId::FLOAT | TypeId::BOOL | TypeId::STR
                    ) {
                        let found = self.types.name(value.ty);
                        self.error(
                            Diagnostic::error(format!("cannot interpolate {}", a_type(&found)))
                                .with_label(value.span, format!("this is {}", a_type(&found)))
                                .with_help(
                                    "an f-string hole takes an `int`, `float`, `bool` or `str`",
                                )
                                .with_note(
                                    "a repr protocol for containers and classes arrives in M3",
                                ),
                        );
                        continue;
                    }
                    let Some(spec) = self.check_format_spec(spec.as_deref(), *span, value.ty)
                    else {
                        continue;
                    };
                    out.push(hir::FStringPart::Value { expr: value, spec });
                }
            }
        }
        hir::Expr {
            kind: hir::ExprKind::FString(out),
            ty: TypeId::STR,
            span,
        }
    }

    /// Validates a format spec against the type of the interpolated value.
    fn check_format_spec(
        &mut self,
        spec: Option<&str>,
        span: Span,
        ty: TypeId,
    ) -> Option<hir::FormatSpec> {
        let Some(spec) = spec else {
            return Some(hir::FormatSpec::Display);
        };
        let type_error = |checker: &mut Self, wanted: &str| {
            let found = checker.types.name(ty);
            let article = if wanted == "int" { "an" } else { "a" };
            let mut diag = Diagnostic::error(format!(
                "the format spec `{spec}` requires {article} `{wanted}`, found `{found}`"
            ))
            .with_label(span, format!("this is {}", a_type(&found)));
            if wanted == "float" && ty == TypeId::INT {
                diag = diag.with_help("use `float(x)` to convert an `int` to a `float`");
            }
            checker.error(diag);
        };
        match spec {
            "d" => {
                if ty != TypeId::INT {
                    type_error(self, "int");
                    return None;
                }
                Some(hir::FormatSpec::Display)
            }
            "s" => {
                if ty != TypeId::STR {
                    type_error(self, "str");
                    return None;
                }
                Some(hir::FormatSpec::Display)
            }
            fixed if fixed.starts_with('.') && fixed.ends_with('f') => {
                let digits = &fixed[1..fixed.len() - 1];
                match digits.parse::<u32>() {
                    Ok(precision) if precision <= 32 => {
                        if ty != TypeId::FLOAT {
                            type_error(self, "float");
                            return None;
                        }
                        Some(hir::FormatSpec::Fixed(precision))
                    }
                    _ => {
                        self.error(
                            Diagnostic::error(format!("invalid format spec `{spec}`"))
                                .with_label(span, "expected `.Nf` with N between 0 and 32"),
                        );
                        None
                    }
                }
            }
            other => {
                self.error(
                    Diagnostic::error(format!("unsupported format spec `{other}`"))
                        .with_label(span, "not a supported format spec")
                        .with_note(
                            "M1 supports `{x}`, `{x:d}`, `{x:s}` and `{x:.Nf}`, \
                             see DESIGN.md section 3.7",
                        ),
                );
                None
            }
        }
    }
}

fn lhs_ty_is_float(kind: &hir::ExprKind) -> bool {
    matches!(kind, hir::ExprKind::FloatMinMax { .. })
}

/// Whether `name` is one of the builtin functions of DESIGN §4.6.
pub(crate) fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "print"
            | "len"
            | "range"
            | "float"
            | "int"
            | "bool"
            | "str"
            | "sorted"
            | "abs"
            | "min"
            | "max"
            | "set"
            | "list"
            | "dict"
            | "tuple"
    )
}

/// The constant integer a tuple index must be, when the expression is one.
///
/// A tuple's members may have different types, so the index has to be known at
/// compile time (DESIGN §4.2). A leading `-` counts from the end.
fn constant_index(expr: &ast::Expr) -> Option<i64> {
    match &expr.kind {
        ast::ExprKind::Int(v) => Some(*v),
        ast::ExprKind::Unary {
            op: ast::UnaryOp::Neg,
            expr,
            ..
        } => match expr.kind {
            ast::ExprKind::Int(v) => Some(v.wrapping_neg()),
            _ => None,
        },
        _ => None,
    }
}
