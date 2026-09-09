//! Constant-expression evaluation.
//!
//! Top-level constants (DESIGN §3.3) and default parameter values
//! (DESIGN §3.2) must be compile-time constants. They are folded here and
//! substituted at every use site, so the generated IR sees literals.
//!
//! The supported forms are literals, references to constants declared earlier
//! in the file, the unary operators, the arithmetic, bitwise and comparison
//! operators, and `and` / `or` / `not`. Anything else — in particular any
//! function call — is rejected.

use typhoon_ast as ast;
use typhoon_diag::{Diagnostic, Span};

use crate::check::{Checker, ConstValue};

impl Checker<'_> {
    /// Evaluates `expr` as a constant, reporting why it is not one.
    pub(crate) fn eval_const(&mut self, expr: &ast::Expr) -> Option<ConstValue> {
        match &expr.kind {
            ast::ExprKind::Error => None,
            // `None` is only a usable constant as the default of a `C | None`
            // field or parameter; every other use is rejected by the type check
            // that follows.
            ast::ExprKind::None => Some(ConstValue::Unit),
            ast::ExprKind::Int(v) => Some(ConstValue::Int(*v)),
            ast::ExprKind::Float(v) => Some(ConstValue::Float(*v)),
            ast::ExprKind::Bool(v) => Some(ConstValue::Bool(*v)),
            ast::ExprKind::Str(v) => Some(ConstValue::Str(v.clone())),
            ast::ExprKind::Name(name) => match self.consts.get(name.as_str()) {
                Some(entry) => Some(entry.value.clone()),
                None => {
                    self.not_constant(expr.span, "only constants declared earlier may be used");
                    None
                }
            },
            ast::ExprKind::Unary {
                op, expr: inner, ..
            } => {
                let value = self.eval_const(inner)?;
                match (op, &value) {
                    (ast::UnaryOp::Neg, ConstValue::Int(v)) => {
                        Some(ConstValue::Int(v.wrapping_neg()))
                    }
                    (ast::UnaryOp::Neg, ConstValue::Float(v)) => Some(ConstValue::Float(-v)),
                    (ast::UnaryOp::Pos, ConstValue::Int(_) | ConstValue::Float(_)) => Some(value),
                    (ast::UnaryOp::BitNot, ConstValue::Int(v)) => Some(ConstValue::Int(!v)),
                    (ast::UnaryOp::Not, ConstValue::Bool(v)) => Some(ConstValue::Bool(!v)),
                    _ => {
                        self.const_type_error(expr.span, op.as_str(), &value, &value);
                        None
                    }
                }
            }
            ast::ExprKind::Binary { op, lhs, rhs, .. } => {
                let a = self.eval_const(lhs)?;
                let b = self.eval_const(rhs)?;
                self.const_binary(*op, &a, &b, expr.span)
            }
            ast::ExprKind::BoolOp { op, lhs, rhs, .. } => {
                let a = self.eval_const(lhs)?;
                let b = self.eval_const(rhs)?;
                match (&a, &b) {
                    (ConstValue::Bool(x), ConstValue::Bool(y)) => {
                        Some(ConstValue::Bool(match op {
                            ast::BoolOp::And => *x && *y,
                            ast::BoolOp::Or => *x || *y,
                        }))
                    }
                    _ => {
                        self.const_type_error(expr.span, op.as_str(), &a, &b);
                        None
                    }
                }
            }
            ast::ExprKind::Compare { left, tail } => {
                if tail.len() != 1 {
                    self.unsupported(expr.span, "a comparison chain", "M3");
                    return None;
                }
                let a = self.eval_const(left)?;
                let b = self.eval_const(&tail[0].rhs)?;
                self.const_compare(tail[0].op, &a, &b, expr.span)
            }
            _ => {
                self.not_constant(expr.span, "this is not a constant expression");
                None
            }
        }
    }

    fn not_constant(&mut self, span: Span, label: &str) {
        self.error(
            Diagnostic::error("expected a constant expression")
                .with_label(span, label)
                .with_note(
                    "constants and default arguments are evaluated at compile time, \
                     see DESIGN.md section 3.3",
                ),
        );
    }

    fn const_type_error(&mut self, span: Span, op: &str, a: &ConstValue, b: &ConstValue) {
        let (x, y) = (const_type_name(a), const_type_name(b));
        self.error(
            Diagnostic::error(format!("`{op}` cannot be applied to `{x}` and `{y}`"))
                .with_label(span, "invalid constant expression"),
        );
    }

    fn const_binary(
        &mut self,
        op: ast::BinOp,
        a: &ConstValue,
        b: &ConstValue,
        span: Span,
    ) -> Option<ConstValue> {
        match (a, b) {
            (ConstValue::Int(x), ConstValue::Int(y)) => {
                let (x, y) = (*x, *y);
                let value = match op {
                    ast::BinOp::Add => x.wrapping_add(y),
                    ast::BinOp::Sub => x.wrapping_sub(y),
                    ast::BinOp::Mul => x.wrapping_mul(y),
                    ast::BinOp::Div => {
                        if y == 0 {
                            self.const_div_by_zero(span);
                            return None;
                        }
                        return Some(ConstValue::Float(x as f64 / y as f64));
                    }
                    ast::BinOp::FloorDiv => {
                        if y == 0 {
                            self.const_div_by_zero(span);
                            return None;
                        }
                        floor_div(x, y)
                    }
                    ast::BinOp::Mod => {
                        if y == 0 {
                            self.const_div_by_zero(span);
                            return None;
                        }
                        floor_mod(x, y)
                    }
                    ast::BinOp::Pow => {
                        if y < 0 {
                            self.error(
                                Diagnostic::error("negative exponent in an integer power")
                                    .with_label(span, "`int ** int` needs a non-negative exponent")
                                    .with_help(
                                        "use `float(x) ** float(y)` for a negative exponent",
                                    ),
                            );
                            return None;
                        }
                        int_pow(x, y)
                    }
                    ast::BinOp::BitAnd => x & y,
                    ast::BinOp::BitOr => x | y,
                    ast::BinOp::BitXor => x ^ y,
                    ast::BinOp::Shl | ast::BinOp::Shr => {
                        if y < 0 {
                            self.error(
                                Diagnostic::error("negative shift amount")
                                    .with_label(span, "a shift amount must not be negative"),
                            );
                            return None;
                        }
                        let amount = y.min(63) as u32;
                        if op == ast::BinOp::Shl {
                            if y > 63 { 0 } else { x.wrapping_shl(amount) }
                        } else {
                            x >> amount
                        }
                    }
                };
                Some(ConstValue::Int(value))
            }
            (ConstValue::Float(x), ConstValue::Float(y)) => {
                let (x, y) = (*x, *y);
                let value = match op {
                    ast::BinOp::Add => x + y,
                    ast::BinOp::Sub => x - y,
                    ast::BinOp::Mul => x * y,
                    ast::BinOp::Div => x / y,
                    ast::BinOp::FloorDiv => (x / y).floor(),
                    ast::BinOp::Mod => x - y * (x / y).floor(),
                    ast::BinOp::Pow => x.powf(y),
                    _ => {
                        self.const_type_error(span, op.as_str(), a, b);
                        return None;
                    }
                };
                Some(ConstValue::Float(value))
            }
            (ConstValue::Str(x), ConstValue::Str(y)) if op == ast::BinOp::Add => {
                Some(ConstValue::Str(format!("{x}{y}")))
            }
            _ => {
                self.const_type_error(span, op.as_str(), a, b);
                None
            }
        }
    }

    fn const_div_by_zero(&mut self, span: Span) {
        self.error(
            Diagnostic::error("division by zero in a constant expression")
                .with_label(span, "this divides by zero"),
        );
    }

    fn const_compare(
        &mut self,
        op: ast::CmpOp,
        a: &ConstValue,
        b: &ConstValue,
        span: Span,
    ) -> Option<ConstValue> {
        let ordering = match (a, b) {
            (ConstValue::Int(x), ConstValue::Int(y)) => x.partial_cmp(y),
            (ConstValue::Float(x), ConstValue::Float(y)) => x.partial_cmp(y),
            (ConstValue::Str(x), ConstValue::Str(y)) => x.partial_cmp(y),
            (ConstValue::Bool(x), ConstValue::Bool(y)) => x.partial_cmp(y),
            _ => {
                self.const_type_error(span, op.as_str(), a, b);
                return None;
            }
        };
        use std::cmp::Ordering;
        let value = match (op, ordering) {
            (ast::CmpOp::Eq, o) => o == Some(Ordering::Equal),
            (ast::CmpOp::Ne, o) => o != Some(Ordering::Equal),
            (ast::CmpOp::Lt, Some(o)) => o == Ordering::Less,
            (ast::CmpOp::Le, Some(o)) => o != Ordering::Greater,
            (ast::CmpOp::Gt, Some(o)) => o == Ordering::Greater,
            (ast::CmpOp::Ge, Some(o)) => o != Ordering::Less,
            (_, None) => false,
            _ => {
                self.unsupported(span, "the `is` operator", "M3");
                return None;
            }
        };
        Some(ConstValue::Bool(value))
    }
}

/// The name of a constant's type, for diagnostics.
fn const_type_name(value: &ConstValue) -> &'static str {
    match value {
        ConstValue::Int(_) => "int",
        ConstValue::Float(_) => "float",
        ConstValue::Bool(_) => "bool",
        ConstValue::Str(_) => "str",
        ConstValue::Unit => "None",
    }
}

/// Flooring integer division, matching the runtime lowering (DESIGN §4.3).
pub(crate) fn floor_div(a: i64, b: i64) -> i64 {
    let q = a.wrapping_div(b);
    let r = a.wrapping_rem(b);
    if r != 0 && ((r < 0) != (b < 0)) {
        q.wrapping_sub(1)
    } else {
        q
    }
}

/// Flooring integer remainder, matching the runtime lowering (DESIGN §4.3).
pub(crate) fn floor_mod(a: i64, b: i64) -> i64 {
    let r = a.wrapping_rem(b);
    if r != 0 && ((r < 0) != (b < 0)) {
        r.wrapping_add(b)
    } else {
        r
    }
}

/// Wrapping exponentiation by squaring, matching `ty_int_pow`.
pub(crate) fn int_pow(mut base: i64, mut exp: i64) -> i64 {
    let mut acc: i64 = 1;
    while exp > 0 {
        if exp & 1 == 1 {
            acc = acc.wrapping_mul(base);
        }
        exp >>= 1;
        if exp > 0 {
            base = base.wrapping_mul(base);
        }
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_division_matches_python() {
        assert_eq!(floor_div(7, 2), 3);
        assert_eq!(floor_div(-7, 2), -4);
        assert_eq!(floor_div(7, -2), -4);
        assert_eq!(floor_div(-7, -2), 3);
        assert_eq!(floor_div(6, 3), 2);
        assert_eq!(floor_div(-6, 3), -2);
        assert_eq!(floor_div(i64::MIN, -1), i64::MIN);
    }

    #[test]
    fn floor_modulo_matches_python() {
        assert_eq!(floor_mod(7, 2), 1);
        assert_eq!(floor_mod(-7, 2), 1);
        assert_eq!(floor_mod(7, -2), -1);
        assert_eq!(floor_mod(-7, -2), -1);
        assert_eq!(floor_mod(6, 3), 0);
        assert_eq!(floor_mod(i64::MIN, -1), 0);
    }

    #[test]
    fn integer_power_wraps() {
        assert_eq!(int_pow(2, 10), 1024);
        assert_eq!(int_pow(3, 0), 1);
        assert_eq!(int_pow(-2, 3), -8);
        assert_eq!(int_pow(0, 0), 1);
        assert_eq!(int_pow(2, 63), i64::MIN);
        assert_eq!(int_pow(10, 19), 10i64.wrapping_pow(19));
    }
}
