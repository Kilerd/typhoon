//! Expression lowering: every [`hir::ExprKind`] to the wasm instructions that
//! compute it, leaving the result on the operand stack.
//!
//! A scalar leaves one value, a `tuple` leaves one per member and a unit
//! expression leaves none (see the module comment of [`super`]).

use typhoon_sema::hir;
use typhoon_sema::types::{Type, TypeId};
use wasm_encoder::{BlockType, Instruction, ValType};

use super::{Emitter, USER_CALL_BASE};
use crate::rt::Rt;

impl<'p> Emitter<'p> {
    /// Emits `expr`, leaving its value on the operand stack.
    pub(crate) fn expr(&mut self, func: &'p hir::Function, expr: &hir::Expr) {
        match &expr.kind {
            hir::ExprKind::Int(v) => self.ins(Instruction::I64Const(*v)),
            hir::ExprKind::Float(v) => self.ins(Instruction::F64Const((*v).into())),
            hir::ExprKind::Bool(v) => self.ins(Instruction::I32Const(i32::from(*v))),
            hir::ExprKind::Str(v) => {
                let global = self.string_literal(v);
                self.ins(Instruction::GlobalGet(global));
            }
            hir::ExprKind::Unit => {}
            hir::ExprKind::Local(id) => self.load_local(*id),

            // -- unary --------------------------------------------------
            hir::ExprKind::IntNeg(value) => {
                self.ins(Instruction::I64Const(0));
                self.expr(func, value);
                self.ins(Instruction::I64Sub);
            }
            hir::ExprKind::IntNot(value) => {
                self.expr(func, value);
                self.ins(Instruction::I64Const(-1));
                self.ins(Instruction::I64Xor);
            }
            hir::ExprKind::FloatNeg(value) => {
                self.expr(func, value);
                self.ins(Instruction::F64Neg);
            }
            hir::ExprKind::Not(value) => {
                self.expr(func, value);
                self.ins(Instruction::I32Eqz);
            }

            // -- int arithmetic -----------------------------------------
            hir::ExprKind::IntBin { op, lhs, rhs } => {
                let constant = constant_int(rhs);
                self.int_bin(func, *op, lhs, rhs, constant);
            }
            hir::ExprKind::IntSquare(value) => {
                self.expr(func, value);
                let slot = self.scratch(ValType::I64);
                self.ins(Instruction::LocalTee(slot));
                self.ins(Instruction::LocalGet(slot));
                self.ins(Instruction::I64Mul);
            }
            hir::ExprKind::IntDiv { lhs, rhs } => {
                let constant = constant_int(rhs);
                self.expr(func, lhs);
                let lhs_slot = self.scratch(ValType::I64);
                self.ins(Instruction::LocalSet(lhs_slot));
                self.expr(func, rhs);
                let rhs_slot = self.scratch(ValType::I64);
                self.ins(Instruction::LocalSet(rhs_slot));
                match constant {
                    // A literal zero divisor always panics.
                    Some(0) => {
                        self.panic("integer division by zero");
                        self.ins(Instruction::F64Const(0.0.into()));
                        return;
                    }
                    Some(_) => {}
                    None => {
                        self.ins(Instruction::LocalGet(rhs_slot));
                        self.ins(Instruction::I64Eqz);
                        self.panic_if("integer division by zero");
                    }
                }
                self.ins(Instruction::LocalGet(lhs_slot));
                self.ins(Instruction::F64ConvertI64S);
                self.ins(Instruction::LocalGet(rhs_slot));
                self.ins(Instruction::F64ConvertI64S);
                self.ins(Instruction::F64Div);
            }

            // -- float arithmetic ---------------------------------------
            hir::ExprKind::FloatBin { op, lhs, rhs } => {
                self.expr(func, lhs);
                self.expr(func, rhs);
                self.float_bin(*op);
            }
            hir::ExprKind::FloatSqrt(value) => {
                self.expr(func, value);
                self.ins(Instruction::F64Sqrt);
            }
            hir::ExprKind::FloatSquare(value) => {
                self.expr(func, value);
                let slot = self.scratch(ValType::F64);
                self.ins(Instruction::LocalTee(slot));
                self.ins(Instruction::LocalGet(slot));
                self.ins(Instruction::F64Mul);
            }

            // -- comparisons --------------------------------------------
            hir::ExprKind::IntCmp { op, lhs, rhs } => {
                self.expr(func, lhs);
                self.expr(func, rhs);
                self.ins(int_compare(*op));
            }
            hir::ExprKind::BoolCmp { op, lhs, rhs } => {
                self.expr(func, lhs);
                self.expr(func, rhs);
                self.ins(match op {
                    hir::CmpOp::Eq => Instruction::I32Eq,
                    _ => Instruction::I32Ne,
                });
            }
            hir::ExprKind::FloatCmp { op, lhs, rhs } => {
                self.expr(func, lhs);
                self.expr(func, rhs);
                self.ins(float_compare(*op));
            }
            hir::ExprKind::StrCmp { op, lhs, rhs } => {
                self.expr(func, lhs);
                self.expr(func, rhs);
                match op {
                    hir::CmpOp::Eq => self.call_rt(Rt::StrEq),
                    hir::CmpOp::Ne => {
                        self.call_rt(Rt::StrEq);
                        self.ins(Instruction::I32Eqz);
                    }
                    // Byte-wise order, which for UTF-8 is code point order.
                    _ => {
                        self.call_rt(Rt::StrCmp);
                        self.ins(Instruction::I64Const(0));
                        self.ins(int_compare(*op));
                    }
                }
            }
            hir::ExprKind::StrConcat { lhs, rhs } => {
                self.expr(func, lhs);
                self.expr(func, rhs);
                self.call_rt(Rt::StrConcat);
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
                self.expr(func, value);
                self.ins(Instruction::F64ConvertI64S);
            }
            // wasm's saturating conversion has exactly the semantics of
            // `llvm.fptosi.sat.i64.f64`: NaN becomes 0 and an out-of-range
            // value clamps to `i64::MIN` / `i64::MAX`.
            hir::ExprKind::FloatToInt(value) => {
                self.expr(func, value);
                self.ins(Instruction::I64TruncSatF64S);
            }
            // An explicit `float(x)` on a `float` is a contraction barrier for
            // LLVM (DESIGN §4.3). wasm has no fused multiply-add at all, so
            // nothing can be contracted and the conversion is the identity.
            hir::ExprKind::FloatFence(value) => self.expr(func, value),
            hir::ExprKind::IntAbs(value) => {
                self.expr(func, value);
                let slot = self.scratch(ValType::I64);
                self.ins(Instruction::LocalSet(slot));
                // Wrapping, like `llvm.abs(x, false)`: `abs(i64::MIN)` is
                // `i64::MIN` rather than poison.
                self.ins(Instruction::I64Const(0));
                self.ins(Instruction::LocalGet(slot));
                self.ins(Instruction::I64Sub);
                self.ins(Instruction::LocalGet(slot));
                self.ins(Instruction::LocalGet(slot));
                self.ins(Instruction::I64Const(0));
                self.ins(Instruction::I64LtS);
                self.ins(Instruction::Select);
            }
            hir::ExprKind::FloatAbs(value) => {
                self.expr(func, value);
                self.ins(Instruction::F64Abs);
            }
            hir::ExprKind::IntMinMax { op, lhs, rhs } => {
                let a = self.scratch(ValType::I64);
                let b = self.scratch(ValType::I64);
                self.expr(func, lhs);
                self.ins(Instruction::LocalSet(a));
                self.expr(func, rhs);
                self.ins(Instruction::LocalSet(b));
                self.ins(Instruction::LocalGet(b));
                self.ins(Instruction::LocalGet(a));
                self.ins(Instruction::LocalGet(b));
                self.ins(Instruction::LocalGet(a));
                self.ins(if *op == hir::MinMax::Min {
                    Instruction::I64LtS
                } else {
                    Instruction::I64GtS
                });
                self.ins(Instruction::Select);
            }
            // Not `f64.min` / `f64.max`: those propagate NaN, while the native
            // backend selects on an ordered comparison and therefore returns
            // the left operand when either side is NaN.
            hir::ExprKind::FloatMinMax { op, lhs, rhs } => {
                let a = self.scratch(ValType::F64);
                let b = self.scratch(ValType::F64);
                self.expr(func, lhs);
                self.ins(Instruction::LocalSet(a));
                self.expr(func, rhs);
                self.ins(Instruction::LocalSet(b));
                self.ins(Instruction::LocalGet(b));
                self.ins(Instruction::LocalGet(a));
                self.ins(Instruction::LocalGet(b));
                self.ins(Instruction::LocalGet(a));
                self.ins(if *op == hir::MinMax::Min {
                    Instruction::F64Lt
                } else {
                    Instruction::F64Gt
                });
                self.ins(Instruction::Select);
            }

            // -- output --------------------------------------------------
            hir::ExprKind::Print(args) => self.print(func, args),
            hir::ExprKind::FString(parts) => self.fstring(func, parts),

            // -- str ------------------------------------------------------
            hir::ExprKind::StrLen(value) => {
                self.expr(func, value);
                self.str_char_len();
            }
            hir::ExprKind::StrMethod { op, recv, args } => self.str_method(func, *op, recv, args),
            hir::ExprKind::StrFrom(value) => {
                let ty = value.ty;
                self.expr(func, value);
                self.format_value(ty, hir::FormatSpec::Display);
            }

            // -- list -----------------------------------------------------
            hir::ExprKind::ListNew { elem, items } => self.list_new(func, *elem, items),
            hir::ExprKind::ListGet { list, index } => self.list_get(func, list, index, expr.ty),
            hir::ExprKind::ListLen(list) => {
                self.expr(func, list);
                self.list_len_on_stack();
            }
            hir::ExprKind::ListAppend { list, value } => self.list_append(func, list, value),
            hir::ExprKind::ListPop(list) => self.list_pop(func, list, expr.ty),
            hir::ExprKind::ListInsert { list, index, value } => {
                self.list_insert(func, list, index, value)
            }
            hir::ExprKind::ListClear(list) => self.list_clear(func, list),
            hir::ExprKind::ListConcat { lhs, rhs } => {
                let elem = self
                    .types
                    .as_list(expr.ty)
                    .expect("`+` on lists yields a list");
                self.list_concat(func, lhs, rhs, elem);
            }
            hir::ExprKind::Contains {
                haystack,
                needle,
                negated,
            } => self.contains(func, haystack, needle, *negated),

            // -- tuple ----------------------------------------------------
            // A tuple literal is exactly its members, one after the other: the
            // operand stack already *is* the aggregate.
            hir::ExprKind::TupleNew(items) => {
                for item in items {
                    self.expr(func, item);
                }
            }
            hir::ExprKind::TupleGet { tuple, index } => {
                let members = self.layout.tuple_members(tuple.ty);
                self.expr(func, tuple);
                let spilled = self.spill(tuple.ty);
                let mut start = 0;
                for member in members.iter().take(*index as usize) {
                    start += self.flat(*member).len();
                }
                let width = self.flat(members[*index as usize]).len();
                let wanted: Vec<u32> = spilled[start..start + width].to_vec();
                self.push_locals(&wanted);
            }

            // -- class ----------------------------------------------------
            hir::ExprKind::New {
                class,
                fields,
                eval_order,
            } => self.class_new(func, *class, fields, eval_order),
            hir::ExprKind::GetField { obj, class, field } => {
                self.expr(func, obj);
                let address = self.scratch(ValType::I32);
                self.ins(Instruction::LocalSet(address));
                let offset = self.layout.class_offsets(*class)[*field as usize];
                self.load_value(address, expr.ty, offset);
            }
            hir::ExprKind::NoneRef => self.ins(Instruction::I32Const(0)),
            hir::ExprKind::IsNone { value, negated } => {
                self.expr(func, value);
                if *negated {
                    self.ins(Instruction::I32Const(0));
                    self.ins(Instruction::I32Ne);
                } else {
                    self.ins(Instruction::I32Eqz);
                }
            }
            hir::ExprKind::RefEq { lhs, rhs, negated } => {
                self.expr(func, lhs);
                self.expr(func, rhs);
                self.ins(if *negated {
                    Instruction::I32Ne
                } else {
                    Instruction::I32Eq
                });
            }
            hir::ExprKind::StructEq { op, lhs, rhs } => {
                let ty = lhs.ty;
                self.expr(func, lhs);
                let left = self.spill(ty);
                self.expr(func, rhs);
                let right = self.spill(ty);
                self.values_equal(ty, &left, &right);
                if *op == hir::CmpOp::Ne {
                    self.ins(Instruction::I32Eqz);
                }
            }
        }
    }

    /// Emits an expression and pops its value into fresh locals.
    pub(crate) fn spill_expr(&mut self, func: &'p hir::Function, expr: &hir::Expr) -> Vec<u32> {
        self.expr(func, expr);
        self.spill(expr.ty)
    }

    // -- integers -------------------------------------------------------------

    fn int_bin(
        &mut self,
        func: &'p hir::Function,
        op: hir::IntOp,
        lhs: &hir::Expr,
        rhs: &hir::Expr,
        constant: Option<i64>,
    ) {
        match op {
            // DESIGN §4.3: `int` wraps, which is what wasm does anyway.
            hir::IntOp::Add
            | hir::IntOp::Sub
            | hir::IntOp::Mul
            | hir::IntOp::BitAnd
            | hir::IntOp::BitOr
            | hir::IntOp::BitXor
            | hir::IntOp::Pow => {
                self.expr(func, lhs);
                self.expr(func, rhs);
                match op {
                    hir::IntOp::Add => self.ins(Instruction::I64Add),
                    hir::IntOp::Sub => self.ins(Instruction::I64Sub),
                    hir::IntOp::Mul => self.ins(Instruction::I64Mul),
                    hir::IntOp::BitAnd => self.ins(Instruction::I64And),
                    hir::IntOp::BitOr => self.ins(Instruction::I64Or),
                    hir::IntOp::BitXor => self.ins(Instruction::I64Xor),
                    _ => self.call_rt(Rt::IntPow),
                }
            }
            hir::IntOp::Shl | hir::IntOp::Shr => {
                let a = self.spill_expr(func, lhs)[0];
                let b = self.spill_expr(func, rhs)[0];
                self.shift(op, a, b, constant);
            }
            hir::IntOp::FloorDiv | hir::IntOp::Mod => {
                let a = self.spill_expr(func, lhs)[0];
                let b = self.spill_expr(func, rhs)[0];
                self.div_mod(op, a, b, constant);
            }
        }
    }

    /// Flooring `//` and `%` (DESIGN §4.3).
    ///
    /// `i64.div_s` / `i64.rem_s` truncate toward zero, so the quotient is
    /// decremented (and the remainder shifted by the divisor) when the operands
    /// have different signs and the division was not exact. A zero divisor
    /// panics before any division happens, and `i64::MIN / -1` — which *traps*
    /// in wasm, where it is merely poison in LLVM — is redirected to a division
    /// by 1, which produces exactly the wrapped result.
    fn div_mod(&mut self, op: hir::IntOp, lhs: u32, rhs: u32, constant: Option<i64>) {
        let want_mod = op == hir::IntOp::Mod;
        match constant {
            Some(0) => {
                self.panic("integer division by zero");
                self.ins(Instruction::I64Const(0));
            }
            Some(-1) => {
                // `x // -1` is `-x` and `x % -1` is 0, neither of which needs a
                // division (and `i64::MIN / -1` would trap).
                if want_mod {
                    self.ins(Instruction::I64Const(0));
                } else {
                    self.ins(Instruction::I64Const(0));
                    self.ins(Instruction::LocalGet(lhs));
                    self.ins(Instruction::I64Sub);
                }
            }
            Some(_) => self.div_mod_body(want_mod, lhs, rhs, rhs),
            None => {
                self.ins(Instruction::LocalGet(rhs));
                self.ins(Instruction::I64Eqz);
                self.panic_if("integer division by zero");

                // safe = (lhs == i64::MIN && rhs == -1) ? 1 : rhs
                let safe = self.scratch(ValType::I64);
                self.ins(Instruction::I64Const(1));
                self.ins(Instruction::LocalGet(rhs));
                self.ins(Instruction::LocalGet(lhs));
                self.ins(Instruction::I64Const(i64::MIN));
                self.ins(Instruction::I64Eq);
                self.ins(Instruction::LocalGet(rhs));
                self.ins(Instruction::I64Const(-1));
                self.ins(Instruction::I64Eq);
                self.ins(Instruction::I32And);
                self.ins(Instruction::Select);
                self.ins(Instruction::LocalSet(safe));

                self.div_mod_body(want_mod, lhs, safe, rhs);
            }
        }
    }

    /// The shared body of `//` and `%`: divide, then correct toward negative
    /// infinity. `divisor` is what is divided by, `sign` the divisor whose sign
    /// decides the correction (they differ only in the `i64::MIN / -1` case).
    fn div_mod_body(&mut self, want_mod: bool, lhs: u32, divisor: u32, sign: u32) {
        let rem = self.scratch(ValType::I64);
        self.ins(Instruction::LocalGet(lhs));
        self.ins(Instruction::LocalGet(divisor));
        self.ins(Instruction::I64RemS);
        self.ins(Instruction::LocalSet(rem));

        let adjust = self.scratch(ValType::I32);
        // rem != 0
        self.ins(Instruction::LocalGet(rem));
        self.ins(Instruction::I64Eqz);
        self.ins(Instruction::I32Eqz);
        // (rem < 0) != (sign < 0)
        self.ins(Instruction::LocalGet(rem));
        self.ins(Instruction::I64Const(0));
        self.ins(Instruction::I64LtS);
        self.ins(Instruction::LocalGet(sign));
        self.ins(Instruction::I64Const(0));
        self.ins(Instruction::I64LtS);
        self.ins(Instruction::I32Xor);
        self.ins(Instruction::I32And);
        self.ins(Instruction::LocalSet(adjust));

        if want_mod {
            self.ins(Instruction::LocalGet(rem));
            self.ins(Instruction::LocalGet(sign));
            self.ins(Instruction::I64Add);
            self.ins(Instruction::LocalGet(rem));
        } else {
            let quot = self.scratch(ValType::I64);
            self.ins(Instruction::LocalGet(lhs));
            self.ins(Instruction::LocalGet(divisor));
            self.ins(Instruction::I64DivS);
            self.ins(Instruction::LocalTee(quot));
            self.ins(Instruction::I64Const(1));
            self.ins(Instruction::I64Sub);
            self.ins(Instruction::LocalGet(quot));
        }
        self.ins(Instruction::LocalGet(adjust));
        self.ins(Instruction::Select);
    }

    /// `<<` and `>>`: Go semantics, a shift of 64 or more yields 0 (or the
    /// sign), a negative shift panics.
    fn shift(&mut self, op: hir::IntOp, lhs: u32, rhs: u32, constant: Option<i64>) {
        let left = op == hir::IntOp::Shl;
        if let Some(amount) = constant {
            if amount < 0 {
                self.panic("negative shift amount");
                self.ins(Instruction::I64Const(0));
                return;
            }
            if amount > 63 {
                if left {
                    self.ins(Instruction::I64Const(0));
                } else {
                    self.ins(Instruction::LocalGet(lhs));
                    self.ins(Instruction::I64Const(63));
                    self.ins(Instruction::I64ShrS);
                }
                return;
            }
            self.ins(Instruction::LocalGet(lhs));
            self.ins(Instruction::LocalGet(rhs));
            self.ins(if left {
                Instruction::I64Shl
            } else {
                Instruction::I64ShrS
            });
            return;
        }

        self.ins(Instruction::LocalGet(rhs));
        self.ins(Instruction::I64Const(0));
        self.ins(Instruction::I64LtS);
        self.panic_if("negative shift amount");

        let too_big = self.scratch(ValType::I32);
        self.ins(Instruction::LocalGet(rhs));
        self.ins(Instruction::I64Const(63));
        self.ins(Instruction::I64GtS);
        self.ins(Instruction::LocalSet(too_big));

        if left {
            self.ins(Instruction::I64Const(0));
            self.ins(Instruction::LocalGet(lhs));
            self.ins(Instruction::LocalGet(rhs));
            self.ins(Instruction::I64Const(63));
            self.ins(Instruction::I64And);
            self.ins(Instruction::I64Shl);
            self.ins(Instruction::LocalGet(too_big));
            self.ins(Instruction::Select);
        } else {
            self.ins(Instruction::LocalGet(lhs));
            self.ins(Instruction::I64Const(63));
            self.ins(Instruction::LocalGet(rhs));
            self.ins(Instruction::LocalGet(too_big));
            self.ins(Instruction::Select);
            self.ins(Instruction::I64ShrS);
        }
    }

    // -- floats ---------------------------------------------------------------

    /// Float arithmetic, with both operands already on the stack.
    ///
    /// The native backend marks `+`, `-` and `*` `contract`, which lets LLVM
    /// fuse a multiply into an add on targets that have FMA. wasm has no FMA
    /// instruction, so every operation here is a plain IEEE-754 one; a program
    /// whose output depends on contraction is the one place the two backends
    /// can legitimately disagree, and such a case carries `# wasm: skip`.
    fn float_bin(&mut self, op: hir::FloatOp) {
        match op {
            hir::FloatOp::Add => self.ins(Instruction::F64Add),
            hir::FloatOp::Sub => self.ins(Instruction::F64Sub),
            hir::FloatOp::Mul => self.ins(Instruction::F64Mul),
            hir::FloatOp::Div => self.ins(Instruction::F64Div),
            hir::FloatOp::FloorDiv => {
                self.ins(Instruction::F64Div);
                self.ins(Instruction::F64Floor);
            }
            hir::FloatOp::Mod => {
                // `a - b * floor(a / b)`.
                let b = self.scratch(ValType::F64);
                let a = self.scratch(ValType::F64);
                self.ins(Instruction::LocalSet(b));
                self.ins(Instruction::LocalSet(a));

                let rem = self.scratch(ValType::F64);
                self.ins(Instruction::LocalGet(a));
                self.ins(Instruction::LocalGet(b));
                self.ins(Instruction::LocalGet(a));
                self.ins(Instruction::LocalGet(b));
                self.ins(Instruction::F64Div);
                self.ins(Instruction::F64Floor);
                self.ins(Instruction::F64Mul);
                self.ins(Instruction::F64Sub);
                self.ins(Instruction::LocalSet(rem));

                // A zero remainder takes the sign of the divisor (`4.0 % -2.0`
                // is `-0.0`), which the subtraction above always makes `+0.0`.
                self.ins(Instruction::F64Const(0.0.into()));
                self.ins(Instruction::LocalGet(b));
                self.ins(Instruction::F64Copysign);
                self.ins(Instruction::LocalGet(rem));
                self.ins(Instruction::LocalGet(rem));
                self.ins(Instruction::F64Const(0.0.into()));
                self.ins(Instruction::F64Eq);
                self.ins(Instruction::Select);
            }
            hir::FloatOp::Pow => self.call_rt(Rt::FloatPow),
        }
    }

    // -- control ---------------------------------------------------------------

    /// `and` / `or` with the right-hand side evaluated only when needed.
    fn short_circuit(
        &mut self,
        func: &'p hir::Function,
        lhs: &hir::Expr,
        rhs: &hir::Expr,
        is_and: bool,
    ) {
        self.expr(func, lhs);
        self.open_if(BlockType::Result(ValType::I32));
        if is_and {
            self.expr(func, rhs);
            self.else_();
            self.ins(Instruction::I32Const(0));
        } else {
            self.ins(Instruction::I32Const(1));
            self.else_();
            self.expr(func, rhs);
        }
        self.end();
    }

    /// A call to a user function: arguments are evaluated in source order and
    /// passed in parameter order (DESIGN §3.2).
    fn call(
        &mut self,
        func: &'p hir::Function,
        callee: hir::FuncId,
        args: &[hir::Expr],
        eval_order: &[u32],
    ) {
        let in_order = eval_order
            .iter()
            .enumerate()
            .all(|(index, arg)| *arg as usize == index);
        if in_order {
            // Source order and parameter order agree, so the arguments can go
            // straight onto the operand stack. Defaults are constants and were
            // not part of the evaluation order; they come last either way.
            for arg in args {
                self.expr(func, arg);
            }
        } else {
            let mut values: Vec<Option<Vec<u32>>> = vec![None; args.len()];
            for index in eval_order {
                let index = *index as usize;
                values[index] = Some(self.spill_expr(func, &args[index]));
            }
            for (index, value) in values.iter_mut().enumerate() {
                if value.is_none() {
                    let arg = &args[index];
                    self.expr(func, arg);
                    *value = Some(self.spill(arg.ty));
                }
            }
            for value in &values {
                let slots = value.clone().expect("filled above");
                self.push_locals(&slots);
            }
        }
        self.ins(Instruction::Call(USER_CALL_BASE + callee.index() as u32));
    }

    // -- output -----------------------------------------------------------------

    /// `print(a, b)`: every argument is evaluated first, then written with a
    /// single space between them and a newline at the end (DESIGN §4.6).
    fn print(&mut self, func: &'p hir::Function, args: &[hir::Expr]) {
        let values: Vec<(TypeId, Vec<u32>)> = args
            .iter()
            .map(|arg| (arg.ty, self.spill_expr(func, arg)))
            .collect();
        for (index, (ty, slots)) in values.iter().enumerate() {
            if index > 0 {
                self.call_rt(Rt::PrintSep);
            }
            // Only a top-level `str` prints verbatim; inside a container it
            // prints as its repr (DESIGN §4.6).
            self.print_value(*ty, slots, false);
        }
        self.call_rt(Rt::PrintEnd);
    }

    /// An f-string: every hole is converted to a `str` and the pieces are
    /// concatenated left to right (DESIGN §3.7).
    fn fstring(&mut self, func: &'p hir::Function, parts: &[hir::FStringPart]) {
        let mut started = false;
        for part in parts {
            match part {
                hir::FStringPart::Literal(text) => {
                    let global = self.string_literal(text);
                    self.ins(Instruction::GlobalGet(global));
                }
                hir::FStringPart::Value { expr, spec } => {
                    let ty = expr.ty;
                    self.expr(func, expr);
                    self.format_value(ty, *spec);
                }
            }
            if started {
                self.call_rt(Rt::StrConcat);
            }
            started = true;
        }
        if !started {
            let empty = self.string_literal("");
            self.ins(Instruction::GlobalGet(empty));
        }
    }

    /// Converts the scalar on top of the stack to a `str`, the way `print` and
    /// f-strings render it.
    pub(crate) fn format_value(&mut self, ty: TypeId, spec: hir::FormatSpec) {
        if let hir::FormatSpec::Fixed(precision) = spec {
            self.ins(Instruction::I64Const(i64::from(precision)));
            self.call_rt(Rt::StrFromFloatFixed);
            return;
        }
        match self.types.get(ty) {
            Type::Str => {}
            Type::Int => self.call_rt(Rt::StrFromInt),
            Type::Float => self.call_rt(Rt::StrFromFloat),
            Type::Bool => self.call_rt(Rt::StrFromBool),
            // Not reachable: sema only lets a `str`, `int`, `float` or `bool`
            // into a hole. Mirror the native backend, which renders anything
            // else as the empty string.
            _ => {
                self.drop_value(ty);
                let empty = self.string_literal("");
                self.ins(Instruction::GlobalGet(empty));
            }
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

/// Signed integer comparison.
pub(crate) fn int_compare(op: hir::CmpOp) -> Instruction<'static> {
    match op {
        hir::CmpOp::Eq => Instruction::I64Eq,
        hir::CmpOp::Ne => Instruction::I64Ne,
        hir::CmpOp::Lt => Instruction::I64LtS,
        hir::CmpOp::Le => Instruction::I64LeS,
        hir::CmpOp::Gt => Instruction::I64GtS,
        hir::CmpOp::Ge => Instruction::I64GeS,
    }
}

/// Float comparison. wasm's comparisons are the ordered ones, and `f64.ne` is
/// the negation of `f64.eq` — exactly LLVM's `oeq` / `une` / `olt` family, so
/// every comparison with a NaN is false except `!=`.
fn float_compare(op: hir::CmpOp) -> Instruction<'static> {
    match op {
        hir::CmpOp::Eq => Instruction::F64Eq,
        hir::CmpOp::Ne => Instruction::F64Ne,
        hir::CmpOp::Lt => Instruction::F64Lt,
        hir::CmpOp::Le => Instruction::F64Le,
        hir::CmpOp::Gt => Instruction::F64Gt,
        hir::CmpOp::Ge => Instruction::F64Ge,
    }
}
