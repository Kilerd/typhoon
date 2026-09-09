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
use crate::types::TypeId;

/// The result of matching the arguments of a call against a signature.
struct CallArgs {
    args: Vec<hir::Expr>,
    eval_order: Vec<u32>,
}

impl Checker<'_> {
    /// Type-checks one expression and lowers it.
    pub(crate) fn check_expr(&mut self, expr: &ast::Expr) -> hir::Expr {
        let span = expr.span;
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
            ast::ExprKind::Attribute { base, .. } => {
                let base = self.check_expr(base);
                if base.ty != TypeId::ERROR {
                    self.unsupported(span, "attribute access", "M2");
                }
                self.error_expr(span)
            }
            ast::ExprKind::Index { base, index } => {
                let base = self.check_expr(base);
                self.check_expr(index);
                if base.ty != TypeId::ERROR {
                    self.unsupported(span, "indexing", "M2");
                }
                self.error_expr(span)
            }
            ast::ExprKind::List(items) => {
                for item in items {
                    self.check_expr(item);
                }
                self.unsupported(span, "a list literal", "M2");
                self.error_expr(span)
            }
            ast::ExprKind::Tuple(items) => {
                for item in items {
                    self.check_expr(item);
                }
                self.unsupported(span, "a tuple", "M2");
                self.error_expr(span)
            }
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
                .with_secondary(value.span, format!("this is {}", a_type(found)));
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
        let rhs = self.check_expr(rhs);
        if lhs.ty == TypeId::ERROR || rhs.ty == TypeId::ERROR {
            return self.error_expr(span);
        }
        if lhs.ty != rhs.ty {
            let (l, r) = (self.types.name(lhs.ty), self.types.name(rhs.ty));
            let mut diag =
                Diagnostic::error(format!("mismatched types: expected `{l}`, found `{r}`"))
                    .with_label(rhs.span, format!("expected `{l}`, found `{r}`"))
                    .with_secondary(lhs.span, format!("this is {}", a_type(l)));
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
            ast::CmpOp::Is | ast::CmpOp::IsNot => {
                self.check_expr(left);
                self.check_expr(&entry.rhs);
                self.unsupported(entry.op_span, "the `is` operator", "M3");
                return self.error_expr(span);
            }
            ast::CmpOp::In | ast::CmpOp::NotIn => {
                self.check_expr(left);
                self.check_expr(&entry.rhs);
                self.unsupported(entry.op_span, "the `in` operator", "M2");
                return self.error_expr(span);
            }
        };

        let lhs = self.check_expr(left);
        let rhs = self.check_expr(&entry.rhs);
        if lhs.ty == TypeId::ERROR || rhs.ty == TypeId::ERROR {
            return self.error_expr(span);
        }
        if lhs.ty != rhs.ty {
            let (l, r) = (self.types.name(lhs.ty), self.types.name(rhs.ty));
            let mut diag =
                Diagnostic::error(format!("mismatched types: expected `{l}`, found `{r}`"))
                    .with_label(rhs.span, format!("expected `{l}`, found `{r}`"))
                    .with_secondary(lhs.span, format!("this is {}", a_type(l)));
            if (lhs.ty == TypeId::FLOAT && rhs.ty == TypeId::INT)
                || (lhs.ty == TypeId::INT && rhs.ty == TypeId::FLOAT)
            {
                diag = diag.with_help("use `float(x)` to convert an `int` to a `float`");
            }
            self.error(diag);
            return self.error_expr(span);
        }

        let ordered = !matches!(op, hir::CmpOp::Eq | hir::CmpOp::Ne);
        let kind = match lhs.ty {
            TypeId::INT => hir::ExprKind::IntCmp {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            TypeId::FLOAT => hir::ExprKind::FloatCmp {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            TypeId::BOOL if !ordered => hir::ExprKind::BoolCmp {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            TypeId::STR if !ordered => hir::ExprKind::StrCmp {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            other => {
                let name = self.types.name(other);
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
            ast::ExprKind::Attribute { base, .. } => {
                self.check_expr(base);
                for arg in args {
                    self.check_expr(&arg.value);
                }
                self.unsupported(span, "a method call", "M2");
                return self.error_expr(span);
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
        if let Some(func) = self.fn_index.get(name.as_str()).copied() {
            return self.check_user_call(func, callee.span, args, span);
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

    fn check_user_call(
        &mut self,
        func: hir::FuncId,
        callee_span: Span,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let sig_name = self.sigs[func.index()].name.clone();
        let ret = self.sigs[func.index()].ret;
        let params: Vec<(String, TypeId, Span, Option<crate::check::ConstValue>)> = self.sigs
            [func.index()]
        .params
        .iter()
        .map(|p| (p.name.clone(), p.ty, p.span, p.default.clone()))
        .collect();

        let Some(matched) = self.match_args(&sig_name, callee_span, &params, args, span) else {
            return self.error_expr(span);
        };
        hir::Expr {
            kind: hir::ExprKind::Call {
                func,
                args: matched.args,
                eval_order: matched.eval_order,
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
                    let value = self.check_expr(&arg.value);
                    let value = self.expect_ty(value, params[idx].1);
                    slots[idx] = Some(value);
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
                    let value = self.check_expr(&arg.value);
                    let value = self.expect_ty(value, params[idx].1);
                    slots[idx] = Some(value);
                    eval_order.push(idx as u32);
                }
            }
        }

        let mut missing = Vec::new();
        for (idx, slot) in slots.iter_mut().enumerate() {
            if slot.is_some() {
                continue;
            }
            match &params[idx].3 {
                Some(default) => *slot = Some(default.to_expr(params[idx].2)),
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
                            .with_label(value.span, format!("this is {}", a_type(found)))
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
            "len" | "str" => {
                for arg in args {
                    self.check_expr(&arg.value);
                }
                let what = format!("the builtin `{builtin}`");
                self.unsupported(span, &what, "M2");
                self.error_expr(span)
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
            .with_label(span, format!("this is {}", a_type(found)));
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
