//! Name resolution, type checking, definite assignment and return analysis.
//!
//! The checker walks the AST once per function, building the typed
//! [`hir`](crate::hir) as it goes. It never bails out on the first error: every
//! problem is pushed into the [`Diagnostics`] sink and the offending node is
//! replaced by a value of type [`TypeId::ERROR`], which every later rule
//! silently accepts. That keeps one mistake to one diagnostic.

use std::collections::HashMap;

use typhoon_ast as ast;
use typhoon_diag::{Diagnostic, Diagnostics, Span};

use crate::hir;
use crate::types::{Type, TypeId, TypeTable};

/// A compile-time constant value: what a top-level constant or a default
/// parameter evaluates to (DESIGN §3.2, §3.3).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ConstValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
}

impl ConstValue {
    pub(crate) fn ty(&self) -> TypeId {
        match self {
            ConstValue::Int(_) => TypeId::INT,
            ConstValue::Float(_) => TypeId::FLOAT,
            ConstValue::Bool(_) => TypeId::BOOL,
            ConstValue::Str(_) => TypeId::STR,
        }
    }

    /// The literal HIR expression this constant is folded into at every use.
    pub(crate) fn to_expr(&self, span: Span) -> hir::Expr {
        let kind = match self {
            ConstValue::Int(v) => hir::ExprKind::Int(*v),
            ConstValue::Float(v) => hir::ExprKind::Float(*v),
            ConstValue::Bool(v) => hir::ExprKind::Bool(*v),
            ConstValue::Str(v) => hir::ExprKind::Str(v.clone()),
        };
        hir::Expr {
            kind,
            ty: self.ty(),
            span,
        }
    }
}

/// `a `float`` / `an `int``: the article a type name needs in a sentence.
pub(crate) fn a_type(name: &str) -> String {
    let article = if name.starts_with(['i', 'a', 'e', 'o', 'u']) {
        "an"
    } else {
        "a"
    };
    format!("{article} `{name}`")
}

/// A checked function signature, available before any body is checked so that
/// functions can call each other in any order.
pub(crate) struct FnSig {
    pub(crate) name: String,
    pub(crate) params: Vec<ParamSig>,
    pub(crate) ret: TypeId,
    pub(crate) name_span: Span,
}

/// One parameter of a [`FnSig`].
pub(crate) struct ParamSig {
    pub(crate) name: String,
    pub(crate) ty: TypeId,
    pub(crate) span: Span,
    pub(crate) default: Option<ConstValue>,
}

/// A top-level constant.
pub(crate) struct ConstEntry {
    pub(crate) value: ConstValue,
    pub(crate) span: Span,
}

/// The checker state: program-wide tables plus the state of the function
/// currently being checked.
pub(crate) struct Checker<'a> {
    pub(crate) diags: &'a mut Diagnostics,
    pub(crate) types: TypeTable,

    pub(crate) sigs: Vec<FnSig>,
    pub(crate) fn_index: HashMap<String, hir::FuncId>,
    pub(crate) consts: HashMap<String, ConstEntry>,
    /// Names whose declaration was rejected. Reading one is silent: the
    /// diagnostic that killed the declaration is the only one worth having.
    pub(crate) poisoned: std::collections::HashSet<String>,

    // -- state of the function being checked ------------------------------
    pub(crate) locals: Vec<hir::Local>,
    pub(crate) scope: HashMap<String, hir::LocalId>,
    /// Definite assignment: `assigned[i]` is true when local `i` is known to
    /// hold a value on every path reaching the current point (DESIGN §3.4).
    pub(crate) assigned: Vec<bool>,
    pub(crate) ret_ty: TypeId,
    pub(crate) fn_name: String,
    pub(crate) loop_depth: u32,
}

/// Whether control can fall out of the bottom of a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flow {
    /// Execution may continue after the block.
    Falls,
    /// Every path out of the block is a `return`, `break` or `continue`.
    Diverges,
}

impl Flow {
    fn diverges(self) -> bool {
        self == Flow::Diverges
    }
}

impl<'a> Checker<'a> {
    pub(crate) fn new(diags: &'a mut Diagnostics) -> Checker<'a> {
        Checker {
            diags,
            types: TypeTable::new(),
            sigs: Vec::new(),
            fn_index: HashMap::new(),
            consts: HashMap::new(),
            poisoned: std::collections::HashSet::new(),
            locals: Vec::new(),
            scope: HashMap::new(),
            assigned: Vec::new(),
            ret_ty: TypeId::UNIT,
            fn_name: String::new(),
            loop_depth: 0,
        }
    }

    // -- diagnostics ------------------------------------------------------

    pub(crate) fn error(&mut self, diag: Diagnostic) {
        self.diags.push(diag);
    }

    /// Reports a feature that parses but has no meaning before `milestone`.
    pub(crate) fn unsupported(&mut self, span: Span, what: &str, milestone: &str) {
        self.error(
            Diagnostic::error(format!(
                "{what} is not supported yet (planned for {milestone})"
            ))
            .with_label(span, "not supported yet")
            .with_note("see DESIGN.md section 3.10 for the roadmap"),
        );
    }

    /// Reports `expected <a>, found <b>` at `span`.
    pub(crate) fn mismatch(&mut self, span: Span, expected: TypeId, found: TypeId) {
        if expected == TypeId::ERROR || found == TypeId::ERROR {
            return;
        }
        let (e, f) = (self.types.name(expected), self.types.name(found));
        let mut diag = Diagnostic::error(format!("mismatched types: expected `{e}`, found `{f}`"))
            .with_label(span, format!("expected `{e}`, found `{f}`"));
        diag = match (self.types.get(expected), self.types.get(found)) {
            (Type::Float, Type::Int) => diag.with_help("use `float(x)` to convert an `int`"),
            (Type::Int, Type::Float) => diag.with_help("use `int(x)` to convert a `float`"),
            (Type::Bool, _) => diag.with_help("compare explicitly, e.g. `x != 0`"),
            _ => diag,
        };
        self.error(diag);
    }

    /// Reports a mismatch unless `expr` already has type `expected`.
    pub(crate) fn expect_ty(&mut self, expr: hir::Expr, expected: TypeId) -> hir::Expr {
        if expr.ty != expected && expr.ty != TypeId::ERROR && expected != TypeId::ERROR {
            self.mismatch(expr.span, expected, expr.ty);
        }
        expr
    }

    /// An expression node standing in for one that failed to check.
    pub(crate) fn error_expr(&self, span: Span) -> hir::Expr {
        hir::Expr {
            kind: hir::ExprKind::Unit,
            ty: TypeId::ERROR,
            span,
        }
    }

    // -- types ------------------------------------------------------------

    /// Resolves a written type to a [`TypeId`], reporting unknown and
    /// not-yet-supported types.
    pub(crate) fn resolve_type(&mut self, ty: &ast::TypeExpr) -> TypeId {
        match &ty.kind {
            ast::TypeExprKind::Error => TypeId::ERROR,
            ast::TypeExprKind::None => TypeId::UNIT,
            ast::TypeExprKind::Union(_) => {
                self.unsupported(ty.span, "an optional type (`T | None`)", "M3");
                TypeId::ERROR
            }
            ast::TypeExprKind::Named { name, args } => match name.as_str() {
                "int" | "float" | "bool" | "str" if !args.is_empty() => {
                    self.unsupported(ty.span, "a generic type", "M3");
                    TypeId::ERROR
                }
                "int" => TypeId::INT,
                "float" => TypeId::FLOAT,
                "bool" => TypeId::BOOL,
                "str" => TypeId::STR,
                "list" | "dict" | "set" | "tuple" => {
                    let what = format!("the type `{}`", name.as_str());
                    let milestone = if name.as_str() == "dict" || name.as_str() == "set" {
                        "M3"
                    } else {
                        "M2"
                    };
                    self.unsupported(ty.span, &what, milestone);
                    TypeId::ERROR
                }
                "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "f32" | "f64" => {
                    let what = format!("the fixed-width type `{}`", name.as_str());
                    self.unsupported(ty.span, &what, "M2");
                    TypeId::ERROR
                }
                _ if !args.is_empty() => {
                    self.unsupported(ty.span, "a generic type", "M3");
                    TypeId::ERROR
                }
                other => {
                    self.error(
                        Diagnostic::error(format!("cannot find type `{other}` in this scope"))
                            .with_label(ty.span, "not a known type")
                            .with_help("the types of M1 are `int`, `float`, `bool` and `str`"),
                    );
                    TypeId::ERROR
                }
            },
        }
    }

    // -- locals -----------------------------------------------------------

    pub(crate) fn declare_local(
        &mut self,
        name: &str,
        ty: TypeId,
        span: Span,
        is_param: bool,
    ) -> hir::LocalId {
        let id = hir::LocalId(self.locals.len() as u32);
        self.locals.push(hir::Local {
            name: name.to_string(),
            ty,
            span,
            is_param,
        });
        self.assigned.push(is_param);
        self.scope.insert(name.to_string(), id);
        id
    }

    /// Grows the definite-assignment vector to cover every declared local.
    fn sync_assigned(&mut self) {
        self.assigned.resize(self.locals.len(), false);
    }

    // -- statements -------------------------------------------------------

    pub(crate) fn check_block(&mut self, block: &ast::Block) -> (hir::Block, Flow) {
        let mut out = hir::Block::default();
        let mut flow = Flow::Falls;
        for stmt in &block.stmts {
            let (lowered, stmt_flow) = self.check_stmt(stmt);
            // Statements after a `return` are dead but still type-checked.
            if let Some(lowered) = lowered
                && flow == Flow::Falls
            {
                out.stmts.push(lowered);
            }
            if stmt_flow.diverges() {
                flow = Flow::Diverges;
            }
        }
        (out, flow)
    }

    fn check_stmt(&mut self, stmt: &ast::Stmt) -> (Option<hir::Stmt>, Flow) {
        match &stmt.kind {
            ast::StmtKind::Pass => (None, Flow::Falls),
            ast::StmtKind::Expr(expr) => {
                if matches!(expr.kind, ast::ExprKind::Error) {
                    return (None, Flow::Falls);
                }
                let value = self.check_expr(expr);
                (Some(hir::Stmt::Expr(value)), Flow::Falls)
            }
            ast::StmtKind::Assign { target, value } => {
                (self.check_assign(target, value), Flow::Falls)
            }
            ast::StmtKind::AnnAssign { name, ty, value } => {
                (self.check_ann_assign(name, ty, value.as_ref()), Flow::Falls)
            }
            ast::StmtKind::Return(value) => {
                (self.check_return(stmt.span, value.as_ref()), Flow::Diverges)
            }
            ast::StmtKind::If {
                cond,
                then,
                elifs,
                else_,
            } => self.check_if(cond, then, elifs, else_.as_ref()),
            ast::StmtKind::While { cond, body } => self.check_while(cond, body),
            ast::StmtKind::For { var, iter, body } => self.check_for(var, iter, body),
            ast::StmtKind::Break => {
                if self.loop_depth == 0 {
                    self.error(
                        Diagnostic::error("`break` outside of a loop")
                            .with_label(stmt.span, "not inside a `while` or `for` loop"),
                    );
                    return (None, Flow::Falls);
                }
                (Some(hir::Stmt::Break), Flow::Diverges)
            }
            ast::StmtKind::Continue => {
                if self.loop_depth == 0 {
                    self.error(
                        Diagnostic::error("`continue` outside of a loop")
                            .with_label(stmt.span, "not inside a `while` or `for` loop"),
                    );
                    return (None, Flow::Falls);
                }
                (Some(hir::Stmt::Continue), Flow::Diverges)
            }
        }
    }

    fn check_assign(&mut self, target: &ast::Expr, value: &ast::Expr) -> Option<hir::Stmt> {
        let name = match &target.kind {
            ast::ExprKind::Name(name) => name,
            ast::ExprKind::Attribute { .. } => {
                self.unsupported(target.span, "attribute access", "M2");
                self.check_expr(value);
                return None;
            }
            ast::ExprKind::Index { .. } => {
                self.unsupported(target.span, "indexing", "M2");
                self.check_expr(value);
                return None;
            }
            // The parser has already reported an invalid target.
            _ => {
                self.check_expr(value);
                return None;
            }
        };

        if self.consts.contains_key(name.as_str()) {
            self.error(
                Diagnostic::error(format!("cannot assign to the constant `{}`", name.as_str()))
                    .with_label(target.span, "constants are immutable")
                    .with_note("top-level constants are not variables, see DESIGN.md section 3.3"),
            );
            self.check_expr(value);
            return None;
        }

        let value = self.check_expr(value);
        match self.scope.get(name.as_str()).copied() {
            Some(id) => {
                let declared = self.locals[id.index()].ty;
                if value.ty != declared && value.ty != TypeId::ERROR && declared != TypeId::ERROR {
                    let span = value.span;
                    let decl_span = self.locals[id.index()].span;
                    let (d, v) = (self.types.name(declared), self.types.name(value.ty));
                    self.error(
                        Diagnostic::error(format!("mismatched types: expected `{d}`, found `{v}`"))
                            .with_label(span, format!("expected `{d}`, found `{v}`"))
                            .with_secondary(
                                decl_span,
                                format!("`{}` was declared with type `{d}` here", name.as_str()),
                            )
                            .with_note(
                                "a variable's type is fixed by its first assignment, \
                                 see DESIGN.md section 3.4",
                            ),
                    );
                    return None;
                }
                self.sync_assigned();
                self.assigned[id.index()] = true;
                Some(hir::Stmt::Assign { local: id, value })
            }
            None => {
                if value.ty == TypeId::ERROR {
                    // Declare it anyway, with the error type: reads of it are
                    // silent, so one bad initializer stays one diagnostic.
                    let id = self.declare_local(name.as_str(), TypeId::ERROR, name.span, false);
                    self.sync_assigned();
                    self.assigned[id.index()] = true;
                    return None;
                }
                if value.ty == TypeId::UNIT {
                    self.error(
                        Diagnostic::error(format!(
                            "cannot declare `{}` with type `None`",
                            name.as_str()
                        ))
                        .with_label(value.span, "this expression produces no value")
                        .with_help(
                            "a variable must hold a value of type `int`, `float`, `bool` or `str`",
                        ),
                    );
                    return None;
                }
                let id = self.declare_local(name.as_str(), value.ty, name.span, false);
                self.sync_assigned();
                self.assigned[id.index()] = true;
                Some(hir::Stmt::Assign { local: id, value })
            }
        }
    }

    fn check_ann_assign(
        &mut self,
        name: &ast::Ident,
        ty: &ast::TypeExpr,
        value: Option<&ast::Expr>,
    ) -> Option<hir::Stmt> {
        let declared = self.resolve_type(ty);
        // The parser has already reported a declaration without initializer.
        let value = value?;
        let value = self.check_expr(value);

        if let Some(existing) = self.scope.get(name.as_str()).copied() {
            let decl_span = self.locals[existing.index()].span;
            self.error(
                Diagnostic::error(format!(
                    "the variable `{}` is already declared in this function",
                    name.as_str()
                ))
                .with_label(name.span, "declared a second time here")
                .with_secondary(decl_span, "first declared here")
                .with_note("typhoon has no shadowing, see DESIGN.md section 3.4"),
            );
            return None;
        }
        if declared == TypeId::UNIT {
            self.error(
                Diagnostic::error(format!(
                    "cannot declare `{}` with type `None`",
                    name.as_str()
                ))
                .with_label(ty.span, "`None` is not a value type"),
            );
            return None;
        }
        let value = self.expect_ty(value, declared);
        let id = self.declare_local(name.as_str(), declared, name.span, false);
        self.sync_assigned();
        self.assigned[id.index()] = true;
        if value.ty == TypeId::ERROR || declared == TypeId::ERROR {
            return None;
        }
        Some(hir::Stmt::Assign { local: id, value })
    }

    fn check_return(&mut self, span: Span, value: Option<&ast::Expr>) -> Option<hir::Stmt> {
        match (value, self.ret_ty) {
            (None, TypeId::UNIT) => Some(hir::Stmt::Return(None)),
            (None, ret) => {
                let name = self.types.name(ret);
                let fn_name = self.fn_name.clone();
                self.error(
                    Diagnostic::error(format!(
                        "`return` with no value in function `{fn_name}` returning `{name}`"
                    ))
                    .with_label(span, format!("expected a `{name}` value"))
                    .with_help(format!("write `return <{name}>`")),
                );
                None
            }
            (Some(expr), TypeId::UNIT) => {
                let value = self.check_expr(expr);
                if value.ty != TypeId::ERROR && value.ty != TypeId::UNIT {
                    let fn_name = self.fn_name.clone();
                    let ty = self.types.name(value.ty);
                    self.error(
                        Diagnostic::error(format!(
                            "cannot return a value from function `{fn_name}`, which returns no value"
                        ))
                        .with_label(expr.span, format!("this is a `{ty}` value"))
                        .with_help(format!("add `-> {ty}` to the signature of `{fn_name}`")),
                    );
                    return None;
                }
                Some(hir::Stmt::Return(None))
            }
            (Some(expr), ret) => {
                let value = self.check_expr(expr);
                let value = self.expect_ty(value, ret);
                Some(hir::Stmt::Return(Some(value)))
            }
        }
    }

    fn check_if(
        &mut self,
        cond: &ast::Expr,
        then: &ast::Block,
        elifs: &[ast::ElifBranch],
        else_: Option<&ast::Block>,
    ) -> (Option<hir::Stmt>, Flow) {
        let cond = self.check_condition(cond);
        let entry = self.assigned.clone();

        let (then_block, then_flow) = self.check_block(then);
        self.sync_assigned();
        let then_assigned = std::mem::replace(&mut self.assigned, entry.clone());
        self.sync_assigned();

        // `elif` chains lower to a nested `if` in the `else` branch.
        let (else_block, else_flow) = match elifs.split_first() {
            Some((head, rest)) => {
                let (stmt, flow) = self.check_if(&head.cond, &head.body, rest, else_);
                let block = hir::Block {
                    stmts: stmt.into_iter().collect(),
                };
                (Some(block), flow)
            }
            None => match else_ {
                Some(body) => {
                    let (block, flow) = self.check_block(body);
                    (Some(block), flow)
                }
                None => (None, Flow::Falls),
            },
        };
        self.sync_assigned();
        let else_assigned = std::mem::replace(&mut self.assigned, entry);
        self.sync_assigned();

        self.merge_branches(then_flow, &then_assigned, else_flow, &else_assigned);
        let flow = if then_flow.diverges() && else_flow.diverges() && else_block.is_some() {
            Flow::Diverges
        } else {
            Flow::Falls
        };
        (
            Some(hir::Stmt::If {
                cond,
                then: then_block,
                else_: else_block,
            }),
            flow,
        )
    }

    /// Intersects the definite-assignment state of two branches, ignoring a
    /// branch that cannot fall through.
    fn merge_branches(&mut self, a_flow: Flow, a: &[bool], b_flow: Flow, b: &[bool]) {
        let n = self.locals.len();
        let get = |v: &[bool], i: usize| v.get(i).copied().unwrap_or(false);
        self.assigned = (0..n)
            .map(|i| match (a_flow.diverges(), b_flow.diverges()) {
                (true, true) => false,
                (true, false) => get(b, i),
                (false, true) => get(a, i),
                (false, false) => get(a, i) && get(b, i),
            })
            .collect();
    }

    fn check_while(&mut self, cond: &ast::Expr, body: &ast::Block) -> (Option<hir::Stmt>, Flow) {
        let cond = self.check_condition(cond);
        let entry = self.assigned.clone();
        self.loop_depth += 1;
        let (body, _) = self.check_block(body);
        self.loop_depth -= 1;
        // The body may run zero times, so nothing it assigns is definite.
        self.assigned = entry;
        self.sync_assigned();
        (Some(hir::Stmt::While { cond, body }), Flow::Falls)
    }

    fn check_for(
        &mut self,
        var: &ast::Ident,
        iter: &ast::Expr,
        body: &ast::Block,
    ) -> (Option<hir::Stmt>, Flow) {
        let Some((start, stop, step)) = self.check_range(iter) else {
            self.loop_depth += 1;
            let _ = self.check_block(body);
            self.loop_depth -= 1;
            return (None, Flow::Falls);
        };

        let entry = self.assigned.clone();
        // The loop variable is an ordinary local (DESIGN §3.4): if it already
        // exists it must be an `int`, otherwise it is declared here.
        let var_id = match self.scope.get(var.as_str()).copied() {
            Some(id) => {
                if self.locals[id.index()].ty != TypeId::INT {
                    let declared = self.locals[id.index()].ty;
                    self.mismatch(var.span, declared, TypeId::INT);
                    return (None, Flow::Falls);
                }
                id
            }
            None => self.declare_local(var.as_str(), TypeId::INT, var.span, false),
        };
        self.sync_assigned();
        self.assigned[var_id.index()] = true;

        self.loop_depth += 1;
        let (body, _) = self.check_block(body);
        self.loop_depth -= 1;
        // The body may run zero times.
        self.assigned = entry;
        self.sync_assigned();

        (
            Some(hir::Stmt::ForRange {
                var: var_id,
                start,
                stop,
                step,
                body,
            }),
            Flow::Falls,
        )
    }

    /// Checks the `range(...)` header of a `for` loop, the only iterable of M1.
    fn check_range(&mut self, iter: &ast::Expr) -> Option<(hir::Expr, hir::Expr, hir::Expr)> {
        let ast::ExprKind::Call {
            callee,
            type_args,
            args,
        } = &iter.kind
        else {
            if !matches!(iter.kind, ast::ExprKind::Error) {
                self.unsupported(iter.span, "iterating over anything but `range(...)`", "M2");
            }
            return None;
        };
        let is_range = matches!(&callee.kind, ast::ExprKind::Name(n) if n.as_str() == "range")
            && !self.scope.contains_key("range");
        if !is_range {
            self.unsupported(iter.span, "iterating over anything but `range(...)`", "M2");
            return None;
        }
        if type_args.is_some() {
            self.unsupported(iter.span, "a generic call", "M3");
            return None;
        }
        if let Some(kw) = args.iter().find(|a| a.name.is_some()) {
            self.error(
                Diagnostic::error("`range` does not take keyword arguments")
                    .with_label(kw.span, "unexpected keyword argument"),
            );
            return None;
        }
        if args.is_empty() || args.len() > 3 {
            self.error(
                Diagnostic::error(format!(
                    "`range` takes 1, 2 or 3 arguments, but {} were supplied",
                    args.len()
                ))
                .with_label(iter.span, "wrong number of arguments")
                .with_help("`range(stop)`, `range(start, stop)` or `range(start, stop, step)`"),
            );
            return None;
        }
        let mut checked = Vec::with_capacity(args.len());
        for arg in args {
            let e = self.check_expr(&arg.value);
            let e = self.expect_ty(e, TypeId::INT);
            checked.push(e);
        }
        let span = iter.span;
        let int = |v: i64| hir::Expr {
            kind: hir::ExprKind::Int(v),
            ty: TypeId::INT,
            span,
        };
        let mut it = checked.into_iter();
        let (start, stop, step) = match args.len() {
            1 => (int(0), it.next().unwrap(), int(1)),
            2 => (it.next().unwrap(), it.next().unwrap(), int(1)),
            _ => (it.next().unwrap(), it.next().unwrap(), it.next().unwrap()),
        };
        Some((start, stop, step))
    }

    /// Checks a condition: it must be a `bool`, there is no truthiness
    /// (DESIGN §3.6).
    pub(crate) fn check_condition(&mut self, cond: &ast::Expr) -> hir::Expr {
        let expr = self.check_expr(cond);
        if expr.ty != TypeId::BOOL && expr.ty != TypeId::ERROR {
            let found = self.types.name(expr.ty);
            self.error(
                Diagnostic::error(format!(
                    "mismatched types: expected `bool`, found `{found}`"
                ))
                .with_label(expr.span, format!("expected `bool`, found `{found}`"))
                .with_help("compare explicitly, e.g. `x != 0`")
                .with_note("typhoon has no truthiness, see DESIGN.md section 3.6"),
            );
            return self.error_expr(expr.span);
        }
        expr
    }
}
