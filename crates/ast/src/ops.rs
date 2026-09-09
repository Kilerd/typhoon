//! Operator enumerations shared by expressions and the pretty-printer.

use std::fmt;

/// Prefix operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    /// `-x`
    Neg,
    /// `+x`
    Pos,
    /// `not x`
    Not,
    /// `~x`
    BitNot,
}

impl UnaryOp {
    /// The operator as it is written in source.
    pub fn as_str(self) -> &'static str {
        match self {
            UnaryOp::Neg => "-",
            UnaryOp::Pos => "+",
            UnaryOp::Not => "not",
            UnaryOp::BitNot => "~",
        }
    }
}

impl fmt::Display for UnaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Arithmetic and bitwise binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/` (int / int yields float, see DESIGN 3.6)
    Div,
    /// `//`
    FloorDiv,
    /// `%`
    Mod,
    /// `**` (right associative)
    Pow,
    /// `&`
    BitAnd,
    /// `|`
    BitOr,
    /// `^`
    BitXor,
    /// `<<`
    Shl,
    /// `>>`
    Shr,
}

impl BinOp {
    /// The operator as it is written in source.
    pub fn as_str(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::FloorDiv => "//",
            BinOp::Mod => "%",
            BinOp::Pow => "**",
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
        }
    }
}

impl fmt::Display for BinOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Short-circuiting logical operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BoolOp {
    /// `and`
    And,
    /// `or`
    Or,
}

impl BoolOp {
    /// The operator as it is written in source.
    pub fn as_str(self) -> &'static str {
        match self {
            BoolOp::And => "and",
            BoolOp::Or => "or",
        }
    }
}

impl fmt::Display for BoolOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Comparison, identity and membership operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CmpOp {
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `is`
    Is,
    /// `is not`
    IsNot,
    /// `in`
    In,
    /// `not in`
    NotIn,
}

impl CmpOp {
    /// The operator as it is written in source.
    pub fn as_str(self) -> &'static str {
        match self {
            CmpOp::Eq => "==",
            CmpOp::Ne => "!=",
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
            CmpOp::Is => "is",
            CmpOp::IsNot => "is not",
            CmpOp::In => "in",
            CmpOp::NotIn => "not in",
        }
    }
}

impl fmt::Display for CmpOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unary_op_strings() {
        assert_eq!(UnaryOp::Neg.to_string(), "-");
        assert_eq!(UnaryOp::Pos.as_str(), "+");
        assert_eq!(UnaryOp::Not.as_str(), "not");
        assert_eq!(UnaryOp::BitNot.as_str(), "~");
    }

    #[test]
    fn bin_op_strings() {
        let all = [
            (BinOp::Add, "+"),
            (BinOp::Sub, "-"),
            (BinOp::Mul, "*"),
            (BinOp::Div, "/"),
            (BinOp::FloorDiv, "//"),
            (BinOp::Mod, "%"),
            (BinOp::Pow, "**"),
            (BinOp::BitAnd, "&"),
            (BinOp::BitOr, "|"),
            (BinOp::BitXor, "^"),
            (BinOp::Shl, "<<"),
            (BinOp::Shr, ">>"),
        ];
        for (op, s) in all {
            assert_eq!(op.as_str(), s);
            assert_eq!(op.to_string(), s);
        }
    }

    #[test]
    fn bool_op_strings() {
        assert_eq!(BoolOp::And.as_str(), "and");
        assert_eq!(BoolOp::Or.to_string(), "or");
    }

    #[test]
    fn cmp_op_strings() {
        let all = [
            (CmpOp::Eq, "=="),
            (CmpOp::Ne, "!="),
            (CmpOp::Lt, "<"),
            (CmpOp::Le, "<="),
            (CmpOp::Gt, ">"),
            (CmpOp::Ge, ">="),
            (CmpOp::Is, "is"),
            (CmpOp::IsNot, "is not"),
            (CmpOp::In, "in"),
            (CmpOp::NotIn, "not in"),
        ];
        for (op, s) in all {
            assert_eq!(op.as_str(), s);
            assert_eq!(op.to_string(), s);
        }
    }
}
