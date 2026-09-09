//! The typhoon abstract syntax tree.
//!
//! Pure data: every node derives `Debug + Clone + PartialEq` and carries a
//! [`Span`](typhoon_diag::Span). The tree covers the whole surface syntax of
//! DESIGN.md section 3, including constructs that are only scheduled for later
//! milestones (`T | None`, generics, f-strings, keyword arguments, defaults):
//! the parser accepts them today so that sema can reject them with a proper
//! diagnostic instead of a syntax error.
//!
//! [`dump`] renders a tree in a compact, deterministic form for snapshot tests.
//!
//! ```
//! use typhoon_ast::{Expr, ExprKind, dump_expr};
//! use typhoon_diag::{FileId, Span};
//!
//! let span = Span::new(FileId(0), 0, 2);
//! assert_eq!(dump_expr(&Expr::new(ExprKind::Int(42), span)), "Int 42 @0..2");
//! ```

#![warn(missing_docs)]

mod dump;
mod nodes;
mod ops;

pub use dump::{dump, dump_expr, dump_stmt, dump_type};
pub use nodes::{
    Arg, Block, ClassDecl, CompareTail, ConstDecl, ElifBranch, Expr, ExprKind, FStringPart, Field,
    FnDecl, GenericParam, Ident, Item, Module, Param, Stmt, StmtKind, TypeExpr, TypeExprKind,
};
pub use ops::{BinOp, BoolOp, CmpOp, UnaryOp};
