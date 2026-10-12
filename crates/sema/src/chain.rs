//! Comparison chains, `a < b < c` (DESIGN §3.6).
//!
//! A chain means `a < b and b < c`, except that `b` is evaluated exactly once:
//! the links run left to right and the first false one skips the rest. Every
//! link is checked by the ordinary single-comparison rules, so a chain accepts
//! exactly the operators and operand types a lone comparison does — `==`,
//! `!=`, the orderings, `is` / `is not` and `in` / `not in` — and narrows
//! nothing, like `and`.
//!
//! To reuse those rules unchanged, the middle operands are memoised by node
//! address in [`Checker::chain_operands`]: the first link to lower one stores
//! its value into a hidden temporary and yields it (a [`hir::ExprKind::Seq`]),
//! the next link reads the temporary back. A literal or a local needs no
//! temporary; reading it twice is free and has no effect.
//!
//! `a < b > (c)` never reaches this module: the parser reads it as the generic
//! call `a<b>(c)` (DESIGN §3.9 rule 3), and `(a < b) > c` is the comparison.

use typhoon_ast as ast;
use typhoon_diag::Span;

use crate::check::Checker;
use crate::hir;
use crate::types::TypeId;

impl Checker<'_> {
    /// Checks a chain of two or more comparisons.
    pub(crate) fn check_compare_chain(
        &mut self,
        left: &ast::Expr,
        tail: &[ast::CompareTail],
        span: Span,
    ) -> hir::Expr {
        let middles: Vec<*const ast::Expr> = tail[..tail.len() - 1]
            .iter()
            .map(|t| std::ptr::from_ref(&t.rhs))
            .collect();
        for key in &middles {
            self.chain_operands.insert(*key, None);
        }
        let mut links = Vec::with_capacity(tail.len());
        let mut lhs = left;
        for entry in tail {
            let link_span = lhs.span.merge(entry.rhs.span);
            links.push(self.check_compare(lhs, std::slice::from_ref(entry), link_span));
            lhs = &entry.rhs;
        }
        for key in &middles {
            self.chain_operands.remove(key);
        }
        if links.iter().any(|link| link.ty == TypeId::ERROR) {
            return self.error_expr(span);
        }
        // `l1 and (l2 and (… and ln))`: the first false link skips the rest.
        let mut chain = links.pop().expect("a chain has at least two links");
        while let Some(link) = links.pop() {
            chain = hir::Expr {
                kind: hir::ExprKind::And {
                    lhs: Box::new(link),
                    rhs: Box::new(chain),
                },
                ty: TypeId::BOOL,
                span,
            };
        }
        chain
    }

    /// Lowers `expr` when it is a middle operand of the chain being checked,
    /// returning `None` when it is not one.
    ///
    /// The first visit lowers it for real and stashes the value; every later
    /// visit — from the next link — reads the stash.
    pub(crate) fn chain_operand(
        &mut self,
        expr: &ast::Expr,
        expected: Option<TypeId>,
    ) -> Option<hir::Expr> {
        let key = std::ptr::from_ref(expr);
        if let Some(reread) = self.chain_operands.get(&key)? {
            return Some(reread.clone());
        }
        self.chain_operands.remove(&key);
        let value = self.check_expr_hint(expr, expected);
        let (first, reread) = self.stash_chain_operand(value);
        self.chain_operands.insert(key, Some(reread));
        Some(first)
    }

    /// `(first use, later reads)` of a middle operand.
    fn stash_chain_operand(&mut self, value: hir::Expr) -> (hir::Expr, hir::Expr) {
        let pure = matches!(
            value.kind,
            hir::ExprKind::Local(_)
                | hir::ExprKind::Int(_)
                | hir::ExprKind::Float(_)
                | hir::ExprKind::Bool(_)
                | hir::ExprKind::Str(_)
                | hir::ExprKind::Unit
                | hir::ExprKind::NoneRef
        );
        // A unit operand is an error in every link, so it needs no slot.
        if pure || value.ty == TypeId::ERROR || value.ty == TypeId::UNIT {
            return (value.clone(), value);
        }
        let (ty, span) = (value.ty, value.span);
        let temp = self.declare_hidden("chain", ty, span);
        let read = hir::Expr {
            kind: hir::ExprKind::Local(temp),
            ty,
            span,
        };
        let first = hir::Expr {
            kind: hir::ExprKind::Seq {
                stmts: hir::Block {
                    stmts: vec![hir::Stmt::Assign { local: temp, value }],
                },
                value: Box::new(read.clone()),
            },
            ty,
            span,
        };
        (first, read)
    }
}
