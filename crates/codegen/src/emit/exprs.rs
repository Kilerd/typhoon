//! Expression lowering: every [`hir::ExprKind`] to the LLVM instructions that
//! compute it, returning the operand string holding the result.

use typhoon_sema::hir;
use typhoon_sema::types::{Type, TypeId};

use super::{Emitter, I64_MIN, Tbaa, float_literal};

impl<'p> Emitter<'p> {
    /// Emits `expr` and returns the operand holding its value. Expressions of
    /// unit type return an empty string.
    pub(super) fn expr(&mut self, func: &'p hir::Function, expr: &hir::Expr) -> String {
        match &expr.kind {
            hir::ExprKind::Int(v) => v.to_string(),
            hir::ExprKind::Float(v) => float_literal(*v),
            hir::ExprKind::Bool(v) => v.to_string(),
            hir::ExprKind::Str(v) => self.string_literal(v),
            hir::ExprKind::Unit => String::new(),
            hir::ExprKind::Local(id) => {
                let local = &func.locals[id.index()];
                let out = self.fresh();
                self.emit(&format!(
                    "{out} = load {}, ptr {}, align {}",
                    self.llvm_ty(local.ty),
                    self.slots[id.index()],
                    self.align_of(local.ty)
                ));
                out
            }

            // -- unary --------------------------------------------------
            hir::ExprKind::IntNeg(value) => {
                let value = self.expr(func, value);
                self.binop("sub", "i64", "0", &value)
            }
            hir::ExprKind::IntNot(value) => {
                let value = self.expr(func, value);
                self.binop("xor", "i64", &value, "-1")
            }
            hir::ExprKind::FloatNeg(value) => {
                let value = self.expr(func, value);
                let out = self.fresh();
                self.emit(&format!("{out} = fneg double {value}"));
                out
            }
            hir::ExprKind::Not(value) => {
                let value = self.expr(func, value);
                self.binop("xor", "i1", &value, "true")
            }

            // -- int arithmetic -----------------------------------------
            hir::ExprKind::IntBin { op, lhs, rhs } => {
                let constant = constant_int(rhs);
                let lhs = self.expr(func, lhs);
                let rhs = self.expr(func, rhs);
                self.int_bin(*op, &lhs, &rhs, constant)
            }
            hir::ExprKind::IntSquare(value) => {
                let value = self.expr(func, value);
                self.binop("mul", "i64", &value, &value)
            }
            hir::ExprKind::IntDiv { lhs, rhs } => {
                let constant = constant_int(rhs);
                let lhs = self.expr(func, lhs);
                let rhs = self.expr(func, rhs);
                match constant {
                    // A literal zero divisor always panics.
                    Some(0) => {
                        self.panic("integer division by zero");
                        return "0.0".to_string();
                    }
                    Some(_) => {}
                    None => {
                        let zero = self.fresh();
                        self.emit(&format!("{zero} = icmp eq i64 {rhs}, 0"));
                        self.panic_if(&zero, "integer division by zero");
                    }
                }
                let a = self.fresh();
                let b = self.fresh();
                self.emit(&format!("{a} = sitofp i64 {lhs} to double"));
                self.emit(&format!("{b} = sitofp i64 {rhs} to double"));
                self.binop("fdiv", "double", &a, &b)
            }

            // -- float arithmetic ---------------------------------------
            hir::ExprKind::FloatBin { op, lhs, rhs } => {
                let lhs = self.expr(func, lhs);
                let rhs = self.expr(func, rhs);
                self.float_bin(*op, &lhs, &rhs)
            }
            hir::ExprKind::FloatSqrt(value) => {
                let value = self.expr(func, value);
                self.decls.insert("declare double @llvm.sqrt.f64(double)");
                let out = self.fresh();
                self.emit(&format!(
                    "{out} = call double @llvm.sqrt.f64(double {value})"
                ));
                out
            }
            hir::ExprKind::FloatSquare(value) => {
                let value = self.expr(func, value);
                self.binop("fmul contract", "double", &value, &value)
            }

            // -- comparisons --------------------------------------------
            hir::ExprKind::IntCmp { op, lhs, rhs } => {
                let lhs = self.expr(func, lhs);
                let rhs = self.expr(func, rhs);
                let pred = int_predicate(*op);
                self.binop(&format!("icmp {pred}"), "i64", &lhs, &rhs)
            }
            hir::ExprKind::BoolCmp { op, lhs, rhs } => {
                let lhs = self.expr(func, lhs);
                let rhs = self.expr(func, rhs);
                let pred = int_predicate(*op);
                self.binop(&format!("icmp {pred}"), "i1", &lhs, &rhs)
            }
            hir::ExprKind::FloatCmp { op, lhs, rhs } => {
                let lhs = self.expr(func, lhs);
                let rhs = self.expr(func, rhs);
                let pred = float_predicate(*op);
                self.binop(&format!("fcmp {pred}"), "double", &lhs, &rhs)
            }
            hir::ExprKind::StrCmp { op, lhs, rhs } => {
                let lhs = self.expr(func, lhs);
                let rhs = self.expr(func, rhs);
                match op {
                    hir::CmpOp::Eq | hir::CmpOp::Ne => {
                        self.decls
                            .insert("declare i8 @ty_str_eq(ptr nocapture readonly, ptr nocapture readonly) nounwind willreturn");
                        let raw = self.fresh();
                        self.emit(&format!("{raw} = call i8 @ty_str_eq(ptr {lhs}, ptr {rhs})"));
                        let pred = if *op == hir::CmpOp::Eq { "ne" } else { "eq" };
                        self.binop(&format!("icmp {pred}"), "i8", &raw, "0")
                    }
                    // Byte-wise order, which for UTF-8 is code point order.
                    _ => {
                        self.decls
                            .insert("declare i64 @ty_str_cmp(ptr nocapture readonly, ptr nocapture readonly) nounwind willreturn");
                        let raw = self.fresh();
                        self.emit(&format!(
                            "{raw} = call i64 @ty_str_cmp(ptr {lhs}, ptr {rhs})"
                        ));
                        let pred = int_predicate(*op);
                        self.binop(&format!("icmp {pred}"), "i64", &raw, "0")
                    }
                }
            }
            hir::ExprKind::StrConcat { lhs, rhs } => {
                let lhs = self.expr(func, lhs);
                let rhs = self.expr(func, rhs);
                self.concat(&lhs, &rhs)
            }

            // -- short circuit ------------------------------------------
            hir::ExprKind::And { lhs, rhs } => self.short_circuit(func, lhs, rhs, true),
            hir::ExprKind::Or { lhs, rhs } => self.short_circuit(func, lhs, rhs, false),

            // -- calls and conversions ----------------------------------
            hir::ExprKind::Call {
                func: callee,
                args,
                eval_order,
            } => self.call(func, *callee, args, eval_order),
            hir::ExprKind::IntToFloat(value) => {
                let value = self.expr(func, value);
                let out = self.fresh();
                self.emit(&format!("{out} = sitofp i64 {value} to double"));
                out
            }
            hir::ExprKind::FloatToInt(value) => {
                let value = self.expr(func, value);
                self.decls
                    .insert("declare i64 @llvm.fptosi.sat.i64.f64(double)");
                let out = self.fresh();
                self.emit(&format!(
                    "{out} = call i64 @llvm.fptosi.sat.i64.f64(double {value})"
                ));
                out
            }
            // An explicit `float(x)` on a `float` is a contraction barrier
            // (DESIGN section 4.3), mirroring Go's rule for conversions.
            hir::ExprKind::FloatFence(value) => {
                let value = self.expr(func, value);
                self.decls
                    .insert("declare double @llvm.arithmetic.fence.f64(double)");
                let out = self.fresh();
                self.emit(&format!(
                    "{out} = call double @llvm.arithmetic.fence.f64(double {value})"
                ));
                out
            }
            hir::ExprKind::IntAbs(value) => {
                let value = self.expr(func, value);
                self.decls.insert("declare i64 @llvm.abs.i64(i64, i1)");
                let out = self.fresh();
                // `false`: `abs(-2**63)` wraps to itself instead of being poison.
                self.emit(&format!(
                    "{out} = call i64 @llvm.abs.i64(i64 {value}, i1 false)"
                ));
                out
            }
            hir::ExprKind::FloatAbs(value) => {
                let value = self.expr(func, value);
                self.decls.insert("declare double @llvm.fabs.f64(double)");
                let out = self.fresh();
                self.emit(&format!(
                    "{out} = call double @llvm.fabs.f64(double {value})"
                ));
                out
            }
            hir::ExprKind::IntMinMax { op, lhs, rhs } => {
                let lhs = self.expr(func, lhs);
                let rhs = self.expr(func, rhs);
                let pred = if *op == hir::MinMax::Min {
                    "slt"
                } else {
                    "sgt"
                };
                let cond = self.binop(&format!("icmp {pred}"), "i64", &rhs, &lhs);
                let out = self.fresh();
                self.emit(&format!("{out} = select i1 {cond}, i64 {rhs}, i64 {lhs}"));
                out
            }
            hir::ExprKind::FloatMinMax { op, lhs, rhs } => {
                let lhs = self.expr(func, lhs);
                let rhs = self.expr(func, rhs);
                let pred = if *op == hir::MinMax::Min {
                    "olt"
                } else {
                    "ogt"
                };
                let cond = self.binop(&format!("fcmp {pred}"), "double", &rhs, &lhs);
                let out = self.fresh();
                self.emit(&format!(
                    "{out} = select i1 {cond}, double {rhs}, double {lhs}"
                ));
                out
            }

            // -- output --------------------------------------------------
            hir::ExprKind::Print(args) => {
                self.print(func, args);
                String::new()
            }
            hir::ExprKind::FString(parts) => self.fstring(func, parts),

            // -- str ------------------------------------------------------
            hir::ExprKind::StrLen(value) => {
                let value = self.expr(func, value);
                self.str_char_len(&value)
            }
            hir::ExprKind::StrMethod { op, recv, args } => self.str_method(func, *op, recv, args),
            hir::ExprKind::StrFrom(value) => {
                let ty = value.ty;
                let value = self.expr(func, value);
                self.format_value(ty, &value, hir::FormatSpec::Display)
            }

            // -- list -----------------------------------------------------
            hir::ExprKind::ListNew { elem, items } => self.list_new(func, *elem, items),
            hir::ExprKind::ListGet { list, index } => self.list_get(func, list, index, expr.ty),
            hir::ExprKind::ListLen(list) => {
                let list = self.expr(func, list);
                self.list_len(&list)
            }
            hir::ExprKind::ListAppend { list, value } => {
                self.list_append(func, list, value);
                String::new()
            }
            hir::ExprKind::ListPop(list) => self.list_pop(func, list, expr.ty),
            hir::ExprKind::ListInsert { list, index, value } => {
                self.list_insert(func, list, index, value);
                String::new()
            }
            hir::ExprKind::ListClear(list) => {
                self.list_clear(func, list);
                String::new()
            }
            hir::ExprKind::ListConcat { lhs, rhs } => {
                let elem = self
                    .types
                    .as_list(expr.ty)
                    .expect("`+` on lists yields a list");
                self.list_concat(func, lhs, rhs, elem)
            }
            hir::ExprKind::Contains {
                haystack,
                needle,
                negated,
            } => self.contains(func, haystack, needle, *negated),

            // -- tuple ----------------------------------------------------
            hir::ExprKind::TupleNew(items) => self.tuple_new(func, expr.ty, items),
            hir::ExprKind::TupleGet { tuple, index } => {
                self.tuple_get(func, tuple, *index, expr.ty)
            }

            // -- class ----------------------------------------------------
            hir::ExprKind::New {
                class,
                fields,
                eval_order,
            } => self.class_new(func, *class, fields, eval_order),
            hir::ExprKind::GetField { obj, class, field } => {
                let obj = self.expr(func, obj);
                let slot = self.field_slot(&obj, *class, *field);
                self.load_slot(&slot, expr.ty, Tbaa::ClassField)
            }
            hir::ExprKind::NoneRef => "null".to_string(),
            hir::ExprKind::IsNone { value, negated } => {
                let value = self.expr(func, value);
                let pred = if *negated { "ne" } else { "eq" };
                self.binop(&format!("icmp {pred}"), "ptr", &value, "null")
            }
            hir::ExprKind::RefEq { lhs, rhs, negated } => {
                let lhs = self.expr(func, lhs);
                let rhs = self.expr(func, rhs);
                let pred = if *negated { "ne" } else { "eq" };
                self.binop(&format!("icmp {pred}"), "ptr", &lhs, &rhs)
            }
            hir::ExprKind::StructEq { op, lhs, rhs } => {
                let ty = lhs.ty;
                let lhs = self.expr(func, lhs);
                let rhs = self.expr(func, rhs);
                let equal = self.values_equal(ty, &lhs, &rhs);
                if *op == hir::CmpOp::Ne {
                    return self.binop("xor", "i1", &equal, "true");
                }
                equal
            }
        }
    }

    /// `%t = <op> <ty> <lhs>, <rhs>`.
    pub(super) fn binop(&mut self, op: &str, ty: &str, lhs: &str, rhs: &str) -> String {
        let out = self.fresh();
        self.emit(&format!("{out} = {op} {ty} {lhs}, {rhs}"));
        out
    }

    fn int_bin(&mut self, op: hir::IntOp, lhs: &str, rhs: &str, constant: Option<i64>) -> String {
        match op {
            // DESIGN section 4.3: `int` wraps, so no `nsw`.
            hir::IntOp::Add => self.binop("add", "i64", lhs, rhs),
            hir::IntOp::Sub => self.binop("sub", "i64", lhs, rhs),
            hir::IntOp::Mul => self.binop("mul", "i64", lhs, rhs),
            hir::IntOp::BitAnd => self.binop("and", "i64", lhs, rhs),
            hir::IntOp::BitOr => self.binop("or", "i64", lhs, rhs),
            hir::IntOp::BitXor => self.binop("xor", "i64", lhs, rhs),
            hir::IntOp::Shl | hir::IntOp::Shr => self.shift(op, lhs, rhs, constant),
            hir::IntOp::Pow => {
                self.decls
                    .insert("declare i64 @ty_int_pow(i64, i64) nounwind");
                let out = self.fresh();
                self.emit(&format!(
                    "{out} = call i64 @ty_int_pow(i64 {lhs}, i64 {rhs})"
                ));
                out
            }
            hir::IntOp::FloorDiv | hir::IntOp::Mod => self.div_mod(op, lhs, rhs, constant),
        }
    }

    /// Flooring `//` and `%` (DESIGN section 4.3).
    ///
    /// `sdiv`/`srem` truncate toward zero, so the quotient is decremented (and
    /// the remainder shifted by the divisor) when the operands have different
    /// signs and the division was not exact. A zero divisor panics, and
    /// `i64::MIN / -1` — which is poison in LLVM — is redirected to a division
    /// by 1, which produces exactly the wrapped result.
    fn div_mod(&mut self, op: hir::IntOp, lhs: &str, rhs: &str, constant: Option<i64>) -> String {
        let want_mod = op == hir::IntOp::Mod;
        match constant {
            Some(0) => {
                self.panic("integer division by zero");
                "0".to_string()
            }
            Some(-1) => {
                self.comment("x // -1 and x % -1 need no division");
                if want_mod {
                    "0".to_string()
                } else {
                    self.binop("sub", "i64", "0", lhs)
                }
            }
            Some(_) => self.div_mod_body(want_mod, lhs, rhs, rhs),
            None => {
                let zero = self.fresh();
                self.emit(&format!("{zero} = icmp eq i64 {rhs}, 0"));
                self.panic_if(&zero, "integer division by zero");
                // Avoid the one case where `sdiv` is poison.
                let is_min = self.binop("icmp eq", "i64", lhs, I64_MIN);
                let is_neg1 = self.binop("icmp eq", "i64", rhs, "-1");
                let overflow = self.binop("and", "i1", &is_min, &is_neg1);
                let safe = self.fresh();
                self.emit(&format!("{safe} = select i1 {overflow}, i64 1, i64 {rhs}"));
                self.div_mod_body(want_mod, lhs, &safe, rhs)
            }
        }
    }

    /// The shared body of `//` and `%`: divide, then correct toward negative
    /// infinity. `divisor` is what is divided by, `sign` the divisor whose sign
    /// decides the correction (they differ only in the `i64::MIN / -1` case).
    fn div_mod_body(&mut self, want_mod: bool, lhs: &str, divisor: &str, sign: &str) -> String {
        let rem = self.binop("srem", "i64", lhs, divisor);
        let non_zero = self.binop("icmp ne", "i64", &rem, "0");
        let rem_neg = self.binop("icmp slt", "i64", &rem, "0");
        let div_neg = self.binop("icmp slt", "i64", sign, "0");
        let differ = self.binop("xor", "i1", &rem_neg, &div_neg);
        let adjust = self.binop("and", "i1", &non_zero, &differ);
        if want_mod {
            let shifted = self.binop("add", "i64", &rem, sign);
            let out = self.fresh();
            self.emit(&format!(
                "{out} = select i1 {adjust}, i64 {shifted}, i64 {rem}"
            ));
            out
        } else {
            let quot = self.binop("sdiv", "i64", lhs, divisor);
            let lowered = self.binop("sub", "i64", &quot, "1");
            let out = self.fresh();
            self.emit(&format!(
                "{out} = select i1 {adjust}, i64 {lowered}, i64 {quot}"
            ));
            out
        }
    }

    /// `<<` and `>>`: Go semantics, a shift of 64 or more yields 0 (or the
    /// sign), a negative shift panics.
    fn shift(&mut self, op: hir::IntOp, lhs: &str, rhs: &str, constant: Option<i64>) -> String {
        let left = op == hir::IntOp::Shl;
        if let Some(amount) = constant {
            if amount < 0 {
                self.panic("negative shift amount");
                return "0".to_string();
            }
            if amount > 63 {
                return if left {
                    "0".to_string()
                } else {
                    self.binop("ashr", "i64", lhs, "63")
                };
            }
            return self.binop(if left { "shl" } else { "ashr" }, "i64", lhs, rhs);
        }
        let negative = self.binop("icmp slt", "i64", rhs, "0");
        self.panic_if(&negative, "negative shift amount");
        let too_big = self.binop("icmp sgt", "i64", rhs, "63");
        if left {
            let masked = self.binop("and", "i64", rhs, "63");
            let shifted = self.binop("shl", "i64", lhs, &masked);
            let out = self.fresh();
            self.emit(&format!(
                "{out} = select i1 {too_big}, i64 0, i64 {shifted}"
            ));
            out
        } else {
            let clamped = self.fresh();
            self.emit(&format!(
                "{clamped} = select i1 {too_big}, i64 63, i64 {rhs}"
            ));
            self.binop("ashr", "i64", lhs, &clamped)
        }
    }

    /// Float arithmetic. `fadd` / `fsub` / `fmul` carry the `contract` flag so
    /// that LLVM may fuse a multiply into an add, which is what Go does on
    /// arm64 (DESIGN section 4.3).
    fn float_bin(&mut self, op: hir::FloatOp, lhs: &str, rhs: &str) -> String {
        match op {
            hir::FloatOp::Add => self.binop("fadd contract", "double", lhs, rhs),
            hir::FloatOp::Sub => self.binop("fsub contract", "double", lhs, rhs),
            hir::FloatOp::Mul => self.binop("fmul contract", "double", lhs, rhs),
            hir::FloatOp::Div => self.binop("fdiv", "double", lhs, rhs),
            hir::FloatOp::FloorDiv => {
                let quotient = self.binop("fdiv", "double", lhs, rhs);
                self.floor(&quotient)
            }
            hir::FloatOp::Mod => {
                // `a - b * floor(a / b)`, deliberately not contracted so that
                // the result does not depend on the target's FMA support.
                let quotient = self.binop("fdiv", "double", lhs, rhs);
                let floored = self.floor(&quotient);
                let scaled = self.binop("fmul", "double", rhs, &floored);
                let rem = self.binop("fsub", "double", lhs, &scaled);
                // A zero remainder takes the sign of the divisor (`4.0 % -2.0`
                // is `-0.0`), which the subtraction above always makes `+0.0`.
                self.decls
                    .insert("declare double @llvm.copysign.f64(double, double)");
                let is_zero = self.binop("fcmp oeq", "double", &rem, "0.0");
                let zero = self.fresh();
                self.emit(&format!(
                    "{zero} = call double @llvm.copysign.f64(double 0.0, double {rhs})"
                ));
                let out = self.fresh();
                self.emit(&format!(
                    "{out} = select i1 {is_zero}, double {zero}, double {rem}"
                ));
                out
            }
            hir::FloatOp::Pow => {
                self.decls
                    .insert("declare double @llvm.pow.f64(double, double)");
                let out = self.fresh();
                self.emit(&format!(
                    "{out} = call double @llvm.pow.f64(double {lhs}, double {rhs})"
                ));
                out
            }
        }
    }

    fn floor(&mut self, value: &str) -> String {
        self.decls.insert("declare double @llvm.floor.f64(double)");
        let out = self.fresh();
        self.emit(&format!(
            "{out} = call double @llvm.floor.f64(double {value})"
        ));
        out
    }

    /// `and` / `or` with the right-hand side evaluated only when needed.
    ///
    /// The result travels through a stack slot rather than a phi node, which
    /// `mem2reg` removes at `-O2` just the same.
    fn short_circuit(
        &mut self,
        func: &'p hir::Function,
        lhs: &hir::Expr,
        rhs: &hir::Expr,
        is_and: bool,
    ) -> String {
        let slot = self.scratch("i1", 1);
        let rhs_label = self.fresh_label(if is_and { "and.rhs" } else { "or.rhs" });
        let end_label = self.fresh_label(if is_and { "and.end" } else { "or.end" });

        let lhs = self.expr(func, lhs);
        self.emit(&format!(
            "store i1 {}, ptr {slot}, align 1",
            if is_and { "false" } else { "true" }
        ));
        if is_and {
            self.cond_br(&lhs, &rhs_label, &end_label);
        } else {
            self.cond_br(&lhs, &end_label, &rhs_label);
        }

        self.start_block(&rhs_label);
        let rhs = self.expr(func, rhs);
        self.emit(&format!("store i1 {rhs}, ptr {slot}, align 1"));
        self.br(&end_label);

        self.start_block(&end_label);
        let out = self.fresh();
        self.emit(&format!("{out} = load i1, ptr {slot}, align 1"));
        out
    }

    /// A call to a user function: arguments are evaluated in source order and
    /// passed in parameter order (DESIGN section 3.2).
    #[allow(clippy::needless_range_loop)]
    fn call(
        &mut self,
        func: &'p hir::Function,
        callee: hir::FuncId,
        args: &[hir::Expr],
        eval_order: &[u32],
    ) -> String {
        let mut values: Vec<Option<String>> = vec![None; args.len()];
        for index in eval_order {
            let index = *index as usize;
            values[index] = Some(self.expr(func, &args[index]));
        }
        // Defaults are constants and were not part of the evaluation order.
        for (index, value) in values.iter_mut().enumerate() {
            if value.is_none() {
                *value = Some(self.expr(func, &args[index]));
            }
        }
        let target = self.program.function(callee);
        let arguments: Vec<String> = args
            .iter()
            .zip(&values)
            .map(|(arg, value)| {
                format!(
                    "{} {}",
                    self.llvm_ty(arg.ty),
                    value.as_deref().expect("filled above")
                )
            })
            .collect();
        let ret = target.ret;
        let symbol = target.symbol.clone();
        if ret == TypeId::UNIT {
            self.emit(&format!("call void @{symbol}({})", arguments.join(", ")));
            String::new()
        } else {
            let out = self.fresh();
            self.emit(&format!(
                "{out} = call {} @{symbol}({})",
                self.llvm_ty(ret),
                arguments.join(", ")
            ));
            out
        }
    }

    /// `print(a, b)`: every argument is evaluated first, then written with a
    /// single space between them and a newline at the end (DESIGN section 4.6).
    pub(super) fn print(&mut self, func: &'p hir::Function, args: &[hir::Expr]) {
        let values: Vec<(TypeId, String)> = args
            .iter()
            .map(|arg| (arg.ty, self.expr(func, arg)))
            .collect();
        for (index, (ty, value)) in values.iter().enumerate() {
            if index > 0 {
                self.decls.insert("declare void @ty_print_sep() nounwind");
                self.emit("call void @ty_print_sep()");
            }
            // Only a top-level `str` prints verbatim; inside a container it
            // prints as its repr (DESIGN §4.6).
            self.print_value(*ty, value, false);
        }
        self.decls.insert("declare void @ty_print_end() nounwind");
        self.emit("call void @ty_print_end()");
    }

    pub(super) fn concat(&mut self, lhs: &str, rhs: &str) -> String {
        self.decls
            .insert("declare noalias ptr @ty_str_concat(ptr nocapture readonly, ptr nocapture readonly) nounwind");
        let out = self.fresh();
        self.emit(&format!(
            "{out} = call ptr @ty_str_concat(ptr {lhs}, ptr {rhs})"
        ));
        out
    }

    /// An f-string: every hole is converted to a `str` and the pieces are
    /// concatenated left to right (DESIGN section 3.7).
    fn fstring(&mut self, func: &'p hir::Function, parts: &[hir::FStringPart]) -> String {
        let mut result: Option<String> = None;
        for part in parts {
            let piece = match part {
                hir::FStringPart::Literal(text) => self.string_literal(text),
                hir::FStringPart::Value { expr, spec } => {
                    let ty = expr.ty;
                    let value = self.expr(func, expr);
                    self.format_value(ty, &value, *spec)
                }
            };
            result = Some(match result {
                Some(left) => self.concat(&left, &piece),
                None => piece,
            });
        }
        result.unwrap_or_else(|| self.string_literal(""))
    }

    pub(super) fn format_value(
        &mut self,
        ty: TypeId,
        value: &str,
        spec: hir::FormatSpec,
    ) -> String {
        if let hir::FormatSpec::Fixed(precision) = spec {
            self.decls
                .insert("declare noalias ptr @ty_str_from_float_fixed(double, i64) nounwind");
            let out = self.fresh();
            self.emit(&format!(
                "{out} = call ptr @ty_str_from_float_fixed(double {value}, i64 {precision})"
            ));
            return out;
        }
        match self.types.get(ty) {
            Type::Str => value.to_string(),
            Type::Int => {
                self.decls
                    .insert("declare noalias ptr @ty_str_from_int(i64) nounwind");
                let out = self.fresh();
                self.emit(&format!("{out} = call ptr @ty_str_from_int(i64 {value})"));
                out
            }
            Type::Float => {
                self.decls
                    .insert("declare noalias ptr @ty_str_from_float(double) nounwind");
                let out = self.fresh();
                self.emit(&format!(
                    "{out} = call ptr @ty_str_from_float(double {value})"
                ));
                out
            }
            Type::Bool => {
                self.decls
                    .insert("declare noalias ptr @ty_str_from_bool(i8) nounwind");
                let byte = self.fresh();
                self.emit(&format!("{byte} = zext i1 {value} to i8"));
                let out = self.fresh();
                self.emit(&format!("{out} = call ptr @ty_str_from_bool(i8 {byte})"));
                out
            }
            _ => self.string_literal(""),
        }
    }
}

/// The literal value of an `int` expression, when it is one.
fn constant_int(expr: &hir::Expr) -> Option<i64> {
    match expr.kind {
        hir::ExprKind::Int(v) => Some(v),
        _ => None,
    }
}

pub(super) fn int_predicate(op: hir::CmpOp) -> &'static str {
    match op {
        hir::CmpOp::Eq => "eq",
        hir::CmpOp::Ne => "ne",
        hir::CmpOp::Lt => "slt",
        hir::CmpOp::Le => "sle",
        hir::CmpOp::Gt => "sgt",
        hir::CmpOp::Ge => "sge",
    }
}

/// Ordered predicates, so that every comparison with a NaN is false — except
/// `!=`, which is the negation of `==` and therefore true (IEEE 754, and what
/// Python does).
fn float_predicate(op: hir::CmpOp) -> &'static str {
    match op {
        hir::CmpOp::Eq => "oeq",
        hir::CmpOp::Ne => "une",
        hir::CmpOp::Lt => "olt",
        hir::CmpOp::Le => "ole",
        hir::CmpOp::Gt => "ogt",
        hir::CmpOp::Ge => "oge",
    }
}
