//! Name resolution, type checking, definite assignment and return analysis.
//!
//! The checker walks the AST once per function, building the typed
//! [`hir`](crate::hir) as it goes. It never bails out on the first error: every
//! problem is pushed into the [`Diagnostics`] sink and the offending node is
//! replaced by a value of type [`TypeId::ERROR`], which every later rule
//! silently accepts. That keeps one mistake to one diagnostic.

use std::collections::{HashMap, HashSet};

use typhoon_ast as ast;
use typhoon_diag::{Diagnostic, Diagnostics, Span};

use crate::hir;
use crate::types::{ClassId, Type, TypeId, TypeTable};

/// A compile-time constant value: what a top-level constant or a default
/// parameter evaluates to (DESIGN §3.2, §3.3).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ConstValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    /// The literal `None`. Only useful as the default of a `C | None` field or
    /// parameter, where [`Checker::coerce`] turns it into a null reference.
    Unit,
}

impl ConstValue {
    pub(crate) fn ty(&self) -> TypeId {
        match self {
            ConstValue::Int(_) => TypeId::INT,
            ConstValue::Float(_) => TypeId::FLOAT,
            ConstValue::Bool(_) => TypeId::BOOL,
            ConstValue::Str(_) => TypeId::STR,
            ConstValue::Unit => TypeId::UNIT,
        }
    }

    /// The literal HIR expression this constant is folded into at every use.
    pub(crate) fn to_expr(&self, span: Span) -> hir::Expr {
        let kind = match self {
            ConstValue::Int(v) => hir::ExprKind::Int(*v),
            ConstValue::Float(v) => hir::ExprKind::Float(*v),
            ConstValue::Bool(v) => hir::ExprKind::Bool(*v),
            ConstValue::Str(v) => hir::ExprKind::Str(v.clone()),
            ConstValue::Unit => hir::ExprKind::Unit,
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
    /// The mangled symbol; methods are namespaced by their class.
    pub(crate) symbol: String,
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

/// A declared class: its id, the type of its instances, and the constant
/// defaults of its fields (one slot per field, in declaration order).
pub(crate) struct ClassEntry {
    pub(crate) id: ClassId,
    pub(crate) ty: TypeId,
    pub(crate) name_span: Span,
    pub(crate) defaults: Vec<Option<ConstValue>>,
}

/// The checker state: program-wide tables plus the state of the function
/// currently being checked.
pub(crate) struct Checker<'a> {
    pub(crate) diags: &'a mut Diagnostics,
    pub(crate) types: TypeTable,

    pub(crate) sigs: Vec<FnSig>,
    pub(crate) fn_index: HashMap<String, hir::FuncId>,
    pub(crate) consts: HashMap<String, ConstEntry>,
    /// Every class by name (DESIGN §3.8).
    pub(crate) classes: HashMap<String, ClassEntry>,
    /// `(class, method name)` to the function the method was lowered into;
    /// a method is an ordinary function whose first parameter is `self`.
    pub(crate) methods: HashMap<(ClassId, String), hir::FuncId>,
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
    /// Locals of type `C | None` that the control flow has proved non-`None`
    /// at the current point (DESIGN §3.8).
    pub(crate) narrowed: HashSet<hir::LocalId>,
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
            classes: HashMap::new(),
            methods: HashMap::new(),
            poisoned: HashSet::new(),
            locals: Vec::new(),
            scope: HashMap::new(),
            assigned: Vec::new(),
            ret_ty: TypeId::UNIT,
            fn_name: String::new(),
            loop_depth: 0,
            narrowed: HashSet::new(),
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
            ast::TypeExprKind::Union(members) => self.resolve_union(members, ty.span),
            ast::TypeExprKind::Named { name, args } => {
                let args: Vec<TypeId> = args.iter().map(|a| self.resolve_type(a)).collect();
                self.resolve_named(name, &args, ty.span)
            }
        }
    }

    /// `C | None`: the only union M2 understands (DESIGN §3.8).
    fn resolve_union(&mut self, members: &[ast::TypeExpr], span: Span) -> TypeId {
        let resolved: Vec<TypeId> = members.iter().map(|m| self.resolve_type(m)).collect();
        if resolved.contains(&TypeId::ERROR) {
            return TypeId::ERROR;
        }
        let nones = resolved.iter().filter(|t| **t == TypeId::UNIT).count();
        let others: Vec<TypeId> = resolved
            .iter()
            .copied()
            .filter(|t| *t != TypeId::UNIT)
            .collect();
        if nones == 1 && others.len() == 1 {
            if let Type::Class(id) = self.types.get(others[0]) {
                return self.types.optional_ty(id);
            }
            let found = self.types.name(others[0]);
            self.error(
                Diagnostic::error(format!(
                    "an optional type (`{found} | None`) is not supported yet (planned for M3)"
                ))
                .with_label(span, "only a class type may be optional in M2")
                .with_help("M2 supports `C | None` for a class `C`, nothing else")
                .with_note("see DESIGN.md section 3.10 for the roadmap"),
            );
            return TypeId::ERROR;
        }
        self.unsupported(span, "a union type", "M3");
        TypeId::ERROR
    }

    fn resolve_named(&mut self, name: &ast::Ident, args: &[TypeId], span: Span) -> TypeId {
        let text = name.as_str();
        let arity_error = |checker: &mut Self, wanted: &str| {
            checker.error(
                Diagnostic::error(format!("`{text}` takes {wanted}"))
                    .with_label(span, "wrong number of type arguments"),
            );
        };
        match text {
            "list" => {
                if args.len() != 1 {
                    arity_error(self, "exactly one type argument, e.g. `list<int>`");
                    return TypeId::ERROR;
                }
                if args[0] == TypeId::ERROR {
                    return TypeId::ERROR;
                }
                if args[0] == TypeId::UNIT {
                    self.error(
                        Diagnostic::error("`list<None>` has no values")
                            .with_label(span, "`None` is not a value type"),
                    );
                    return TypeId::ERROR;
                }
                self.types.list_of(args[0])
            }
            "tuple" => {
                if args.is_empty() {
                    arity_error(self, "at least one type argument, e.g. `tuple<int, str>`");
                    return TypeId::ERROR;
                }
                if args.contains(&TypeId::ERROR) {
                    return TypeId::ERROR;
                }
                if args.contains(&TypeId::UNIT) {
                    self.error(
                        Diagnostic::error("a tuple member cannot have type `None`")
                            .with_label(span, "`None` is not a value type"),
                    );
                    return TypeId::ERROR;
                }
                self.types.tuple_of(args)
            }
            "int" | "float" | "bool" | "str" if !args.is_empty() => {
                self.unsupported(span, "a generic type", "M3");
                TypeId::ERROR
            }
            "int" => TypeId::INT,
            "float" => TypeId::FLOAT,
            "bool" => TypeId::BOOL,
            "str" => TypeId::STR,
            "dict" | "set" => {
                let what = format!("the type `{text}`");
                self.unsupported(span, &what, "M3");
                TypeId::ERROR
            }
            "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "f32" | "f64" => {
                let what = format!("the fixed-width type `{text}`");
                self.unsupported(span, &what, "M3");
                TypeId::ERROR
            }
            _ if !args.is_empty() => {
                self.unsupported(span, "a generic type", "M3");
                TypeId::ERROR
            }
            _ => match self.classes.get(text) {
                Some(entry) => entry.ty,
                None => {
                    if self.poisoned.contains(text) {
                        return TypeId::ERROR;
                    }
                    self.error(
                        Diagnostic::error(format!("cannot find type `{text}` in this scope"))
                            .with_label(span, "not a known type")
                            .with_help(
                                "M2 has `int`, `float`, `bool`, `str`, `list<T>`, \
                                 `tuple<A, B>` and the classes declared in this file",
                            ),
                    );
                    TypeId::ERROR
                }
            },
        }
    }

    // -- assignability -----------------------------------------------------

    /// Adapts `expr` to `expected`, reporting a mismatch when it cannot.
    ///
    /// The only widening M2 performs is `C` to `C | None` (DESIGN §3.8): both
    /// are one pointer, and `None` becomes the null one. Everything else must
    /// match exactly — there are no implicit conversions (DESIGN §4.3).
    pub(crate) fn coerce(&mut self, expr: hir::Expr, expected: TypeId) -> hir::Expr {
        if expected == TypeId::ERROR || expr.ty == TypeId::ERROR || expr.ty == expected {
            return expr;
        }
        if let Type::Optional(want) = self.types.get(expected) {
            match self.types.get(expr.ty) {
                Type::Class(got) if got == want => {
                    return hir::Expr {
                        ty: expected,
                        ..expr
                    };
                }
                Type::Unit => {
                    return hir::Expr {
                        kind: hir::ExprKind::NoneRef,
                        ty: expected,
                        span: expr.span,
                    };
                }
                _ => {}
            }
        }
        self.mismatch(expr.span, expected, expr.ty);
        hir::Expr {
            ty: TypeId::ERROR,
            ..expr
        }
    }

    /// Reports the use of a possibly-`None` reference where a class instance
    /// is required, and how to narrow it.
    pub(crate) fn require_narrowed(&mut self, span: Span, ty: TypeId) {
        let name = self.types.name(ty);
        let class = match self.types.get(ty) {
            Type::Optional(id) => self.types.class(id).name.clone(),
            _ => name.clone(),
        };
        self.error(
            Diagnostic::error(format!("`{name}` may be `None` here"))
                .with_label(span, format!("this is `{name}`, not `{class}`"))
                .with_help(
                    "narrow it first: assign it to a local, then test it in an `if` \
                     with `x is None` / `x is not None`",
                )
                .with_note(
                    "M2 narrows only inside an `if`; it does not flow through `and`, \
                     `or` or a loop condition, see DESIGN.md section 3.8",
                ),
        );
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

    /// Declares a compiler-generated local that no source name can reach.
    ///
    /// Used by tuple unpacking to evaluate the right-hand side exactly once.
    /// The name contains a `.`, which no identifier may, so it never collides.
    pub(crate) fn declare_temp(&mut self, ty: TypeId, span: Span) -> hir::LocalId {
        let id = hir::LocalId(self.locals.len() as u32);
        self.locals.push(hir::Local {
            name: format!("unpack.{}", id.index()),
            ty,
            span,
            is_param: false,
        });
        self.assigned.push(true);
        id
    }

    /// Declares `name` with the error type when it is not already in scope.
    ///
    /// A rejected declaration must still bind its name, so that later reads of
    /// it are silent and one mistake stays one diagnostic.
    fn declare_poisoned_local(&mut self, name: &ast::Ident) {
        if self.scope.contains_key(name.as_str()) {
            return;
        }
        let id = self.declare_local(name.as_str(), TypeId::ERROR, name.span, false);
        self.sync_assigned();
        self.assigned[id.index()] = true;
    }

    /// Grows the definite-assignment vector to cover every declared local.
    fn sync_assigned(&mut self) {
        self.assigned.resize(self.locals.len(), false);
    }

    /// Records that `local` now holds `value`, updating both the
    /// definite-assignment state and the `is None` narrowing.
    fn mark_assigned(&mut self, local: hir::LocalId, value: &hir::Expr) {
        self.sync_assigned();
        self.assigned[local.index()] = true;
        // Storing a `C` into a `C | None` local proves it non-null; storing
        // anything else (including a `C | None`) forgets what we knew.
        let declared = self.locals[local.index()].ty;
        let proven = matches!(self.types.get(declared), Type::Optional(want)
            if self.types.get(value.ty) == Type::Class(want));
        if proven {
            self.narrowed.insert(local);
        } else {
            self.narrowed.remove(&local);
        }
    }

    /// Whether a value of type `found` may be stored where `expected` is
    /// required (DESIGN §4.3: no implicit conversions, except `C` widening to
    /// `C | None`).
    pub(crate) fn assignable(&self, expected: TypeId, found: TypeId) -> bool {
        if expected == found || expected == TypeId::ERROR || found == TypeId::ERROR {
            return true;
        }
        match (self.types.get(expected), self.types.get(found)) {
            (Type::Optional(want), Type::Class(got)) => want == got,
            (Type::Optional(_), Type::Unit) => true,
            _ => false,
        }
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
        match &target.kind {
            ast::ExprKind::Name(name) => self.check_name_assign(target.span, name, value),
            ast::ExprKind::Attribute { base, attr } => self.check_field_assign(base, attr, value),
            ast::ExprKind::Index { base, index } => self.check_index_assign(base, index, value),
            ast::ExprKind::Tuple(items) => self.check_unpack_assign(items, value, target.span),
            // The parser has already reported an invalid target.
            _ => {
                self.check_expr(value);
                None
            }
        }
    }

    /// `name = value`: the first assignment declares the variable and fixes its
    /// type, later ones must agree (DESIGN §3.4).
    fn check_name_assign(
        &mut self,
        target_span: Span,
        name: &ast::Ident,
        value: &ast::Expr,
    ) -> Option<hir::Stmt> {
        if self.consts.contains_key(name.as_str()) {
            self.error(
                Diagnostic::error(format!("cannot assign to the constant `{}`", name.as_str()))
                    .with_label(target_span, "constants are immutable")
                    .with_note("top-level constants are not variables, see DESIGN.md section 3.3"),
            );
            self.check_expr(value);
            return None;
        }

        let existing = self.scope.get(name.as_str()).copied();
        let hint = existing.map(|id| self.locals[id.index()].ty);
        let value = self.check_expr_hint(value, hint);
        match existing {
            Some(id) => {
                let declared = self.locals[id.index()].ty;
                if !self.assignable(declared, value.ty) {
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
                let value = self.coerce(value, declared);
                self.mark_assigned(id, &value);
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
                        .with_help("a variable must hold a value; annotate it to store `None`"),
                    );
                    return None;
                }
                let id = self.declare_local(name.as_str(), value.ty, name.span, false);
                self.mark_assigned(id, &value);
                Some(hir::Stmt::Assign { local: id, value })
            }
        }
    }

    /// `obj.field = value` (DESIGN §3.8).
    fn check_field_assign(
        &mut self,
        base: &ast::Expr,
        attr: &ast::Ident,
        value: &ast::Expr,
    ) -> Option<hir::Stmt> {
        let obj = self.check_expr(base);
        let Some((class, field)) = self.resolve_field(&obj, attr) else {
            self.check_expr(value);
            return None;
        };
        let declared = self.types.class(class).fields[field as usize].ty;
        let value = self.check_expr_hint(value, Some(declared));
        if !self.assignable(declared, value.ty) {
            self.mismatch(value.span, declared, value.ty);
            return None;
        }
        let value = self.coerce(value, declared);
        Some(hir::Stmt::SetField {
            obj,
            class,
            field,
            value,
        })
    }

    /// `xs[i] = value`; `str` is immutable and tuples are values, so only a
    /// `list<T>` can be assigned through (DESIGN §4.2).
    fn check_index_assign(
        &mut self,
        base: &ast::Expr,
        index: &ast::Expr,
        value: &ast::Expr,
    ) -> Option<hir::Stmt> {
        let list = self.check_expr(base);
        let Some(elem) = self.types.as_list(list.ty) else {
            if list.ty != TypeId::ERROR {
                let found = self.types.name(list.ty);
                let mut diag =
                    Diagnostic::error(format!("cannot assign through an index into `{found}`"))
                        .with_label(base.span, format!("this is {}", a_type(&found)));
                diag = match self.types.get(list.ty) {
                    Type::Str => {
                        diag.with_help("`str` is immutable; build a new one with `+` or `replace`")
                    }
                    Type::Tuple(_) => diag.with_help("tuples are values and cannot be mutated"),
                    _ => diag.with_help("only a `list<T>` supports indexed assignment"),
                };
                self.error(diag);
            }
            self.check_expr(index);
            self.check_expr(value);
            return None;
        };
        let index = self.check_expr(index);
        let index = self.expect_ty(index, TypeId::INT);
        let value = self.check_expr_hint(value, Some(elem));
        if !self.assignable(elem, value.ty) {
            self.mismatch(value.span, elem, value.ty);
            return None;
        }
        let value = self.coerce(value, elem);
        Some(hir::Stmt::SetIndex { list, index, value })
    }

    /// `a, b = t`: the right-hand side is evaluated once into a temporary and
    /// then destructured (DESIGN §4.2).
    fn check_unpack_assign(
        &mut self,
        targets: &[ast::Expr],
        value: &ast::Expr,
        span: Span,
    ) -> Option<hir::Stmt> {
        let value = self.check_expr(value);
        // Whatever goes wrong below, the targets are still declared, so that
        // reading them afterwards adds no diagnostic of its own.
        let bind_targets = |checker: &mut Self| {
            for target in targets {
                if let ast::ExprKind::Name(name) = &target.kind {
                    checker.declare_poisoned_local(name);
                }
            }
        };
        if value.ty == TypeId::ERROR {
            bind_targets(self);
            return None;
        }
        let Some(members) = self.types.as_tuple(value.ty).map(<[TypeId]>::to_vec) else {
            let found = self.types.name(value.ty);
            self.error(
                Diagnostic::error(format!("cannot unpack {}", a_type(&found)))
                    .with_label(value.span, format!("this is {}", a_type(&found)))
                    .with_help("only a tuple can be unpacked"),
            );
            bind_targets(self);
            return None;
        };
        if members.len() != targets.len() {
            self.error(
                Diagnostic::error(format!(
                    "expected {} value{} to unpack, found {}",
                    targets.len(),
                    if targets.len() == 1 { "" } else { "s" },
                    members.len()
                ))
                .with_label(span, format!("{} target(s) here", targets.len()))
                .with_secondary(
                    value.span,
                    format!("this is `{}`", self.types.name(value.ty)),
                ),
            );
            bind_targets(self);
            return None;
        }

        let temp = self.declare_temp(value.ty, span);
        let temp_ty = value.ty;
        let mut stmts = vec![hir::Stmt::Assign { local: temp, value }];
        for (index, target) in targets.iter().enumerate() {
            let member = hir::Expr {
                kind: hir::ExprKind::TupleGet {
                    tuple: Box::new(hir::Expr {
                        kind: hir::ExprKind::Local(temp),
                        ty: temp_ty,
                        span: target.span,
                    }),
                    index: index as u32,
                },
                ty: members[index],
                span: target.span,
            };
            if let Some(stmt) = self.assign_checked(target, member) {
                stmts.push(stmt);
            }
        }
        Some(hir::Stmt::Group(hir::Block { stmts }))
    }

    /// Stores an already-checked value into an assignment target.
    fn assign_checked(&mut self, target: &ast::Expr, value: hir::Expr) -> Option<hir::Stmt> {
        match &target.kind {
            ast::ExprKind::Name(name) => {
                if self.consts.contains_key(name.as_str()) {
                    self.error(
                        Diagnostic::error(format!(
                            "cannot assign to the constant `{}`",
                            name.as_str()
                        ))
                        .with_label(target.span, "constants are immutable"),
                    );
                    return None;
                }
                match self.scope.get(name.as_str()).copied() {
                    Some(id) => {
                        let declared = self.locals[id.index()].ty;
                        if !self.assignable(declared, value.ty) {
                            self.mismatch(value.span, declared, value.ty);
                            return None;
                        }
                        let value = self.coerce(value, declared);
                        self.mark_assigned(id, &value);
                        Some(hir::Stmt::Assign { local: id, value })
                    }
                    None => {
                        let id = self.declare_local(name.as_str(), value.ty, name.span, false);
                        self.mark_assigned(id, &value);
                        Some(hir::Stmt::Assign { local: id, value })
                    }
                }
            }
            ast::ExprKind::Attribute { base, attr } => {
                let obj = self.check_expr(base);
                let (class, field) = self.resolve_field(&obj, attr)?;
                let declared = self.types.class(class).fields[field as usize].ty;
                if !self.assignable(declared, value.ty) {
                    self.mismatch(value.span, declared, value.ty);
                    return None;
                }
                let value = self.coerce(value, declared);
                Some(hir::Stmt::SetField {
                    obj,
                    class,
                    field,
                    value,
                })
            }
            ast::ExprKind::Index { base, index } => {
                let list = self.check_expr(base);
                let elem = self.types.as_list(list.ty)?;
                let index = self.check_expr(index);
                let index = self.expect_ty(index, TypeId::INT);
                if !self.assignable(elem, value.ty) {
                    self.mismatch(value.span, elem, value.ty);
                    return None;
                }
                let value = self.coerce(value, elem);
                Some(hir::Stmt::SetIndex { list, index, value })
            }
            _ => None,
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
        let value = self.check_expr_hint(value, Some(declared));

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
        if !self.assignable(declared, value.ty) {
            self.mismatch(value.span, declared, value.ty);
            // Bind the name anyway, so that reading it later is silent.
            let id = self.declare_local(name.as_str(), declared, name.span, false);
            self.sync_assigned();
            self.assigned[id.index()] = true;
            return None;
        }
        let value = self.coerce(value, declared);
        let id = self.declare_local(name.as_str(), declared, name.span, false);
        self.mark_assigned(id, &value);
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
                let value = self.check_expr_hint(expr, Some(ret));
                if !self.assignable(ret, value.ty) {
                    self.mismatch(value.span, ret, value.ty);
                    return None;
                }
                let value = self.coerce(value, ret);
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
        let narrow = self.narrowing_of(cond);
        let cond = self.check_condition(cond);
        let entry = self.assigned.clone();
        let entry_narrow = self.narrowed.clone();

        self.apply_narrowing(narrow, true);
        let (then_block, then_flow) = self.check_block(then);
        self.sync_assigned();
        let then_assigned = std::mem::replace(&mut self.assigned, entry.clone());
        let then_narrow = std::mem::replace(&mut self.narrowed, entry_narrow.clone());
        self.sync_assigned();
        self.apply_narrowing(narrow, false);

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
        let else_narrow = std::mem::replace(&mut self.narrowed, entry_narrow);
        self.sync_assigned();

        self.merge_branches(then_flow, &then_assigned, else_flow, &else_assigned);
        self.narrowed = match (then_flow.diverges(), else_flow.diverges()) {
            (true, true) => HashSet::new(),
            (true, false) => else_narrow,
            (false, true) => then_narrow,
            (false, false) => then_narrow.intersection(&else_narrow).copied().collect(),
        };
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

    /// The narrowing an `if` condition performs when it holds: the local it
    /// talks about, and whether a true condition proves it is *not* `None`.
    ///
    /// Only the two shapes DESIGN §3.8 promises are recognised — `x is None`
    /// and `x is not None`, optionally behind a `not` — because they are the
    /// ones that make `C | None` usable without a general flow analysis.
    fn narrowing_of(&self, cond: &ast::Expr) -> Option<(hir::LocalId, bool)> {
        match &cond.kind {
            ast::ExprKind::Unary {
                op: ast::UnaryOp::Not,
                expr,
                ..
            } => {
                let (id, non_none) = self.narrowing_of(expr)?;
                Some((id, !non_none))
            }
            ast::ExprKind::Compare { left, tail } if tail.len() == 1 => {
                let non_none = match tail[0].op {
                    ast::CmpOp::Is => false,
                    ast::CmpOp::IsNot => true,
                    _ => return None,
                };
                let (name, other) = match (&left.kind, &tail[0].rhs.kind) {
                    (ast::ExprKind::Name(name), other) => (name, other),
                    (other, ast::ExprKind::Name(name)) => (name, other),
                    _ => return None,
                };
                if !matches!(other, ast::ExprKind::None) {
                    return None;
                }
                let id = *self.scope.get(name.as_str())?;
                match self.types.get(self.locals[id.index()].ty) {
                    Type::Optional(_) => Some((id, non_none)),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Applies the narrowing of a condition to the current state; `holds` is
    /// whether the condition is known true on this path.
    fn apply_narrowing(&mut self, narrow: Option<(hir::LocalId, bool)>, holds: bool) {
        if let Some((id, non_none)) = narrow {
            if non_none == holds {
                self.narrowed.insert(id);
            } else {
                self.narrowed.remove(&id);
            }
        }
    }

    /// Forgets what is known about every local the block assigns, because a
    /// loop body runs an unknown number of times.
    fn forget_narrowing_assigned_in(&mut self, body: &ast::Block) {
        let mut names = HashSet::new();
        assigned_names(body, &mut names);
        let drop: Vec<hir::LocalId> = self
            .narrowed
            .iter()
            .copied()
            .filter(|id| names.contains(&self.locals[id.index()].name))
            .collect();
        for id in drop {
            self.narrowed.remove(&id);
        }
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
        let narrow = self.narrowing_of(cond);
        let cond = self.check_condition(cond);
        let entry = self.assigned.clone();
        let entry_narrow = self.narrowed.clone();
        // The condition holds inside the body, but the body may reassign the
        // local, so anything it touches loses its narrowing.
        self.apply_narrowing(narrow, true);
        self.forget_narrowing_assigned_in(body);
        self.loop_depth += 1;
        let (body, _) = self.check_block(body);
        self.loop_depth -= 1;
        // The body may run zero times, so nothing it assigns is definite.
        self.assigned = entry;
        self.narrowed = entry_narrow;
        self.sync_assigned();
        (Some(hir::Stmt::While { cond, body }), Flow::Falls)
    }

    fn check_for(
        &mut self,
        var: &ast::Ident,
        iter: &ast::Expr,
        body: &ast::Block,
    ) -> (Option<hir::Stmt>, Flow) {
        // `range(...)` is a language form, not a value: it compiles to a
        // counting loop and allocates nothing (DESIGN §3.5).
        if self.is_range_call(iter) {
            let Some((start, stop, step)) = self.check_range(iter) else {
                return (self.check_dead_loop_body(var, body), Flow::Falls);
            };
            let Some(var_id) = self.bind_loop_var(var, TypeId::INT) else {
                return (self.check_dead_loop_body(var, body), Flow::Falls);
            };
            let body = self.check_loop_body(body, var_id);
            return (
                Some(hir::Stmt::ForRange {
                    var: var_id,
                    start,
                    stop,
                    step,
                    body,
                }),
                Flow::Falls,
            );
        }

        let iterable = self.check_expr(iter);
        let (over, elem) = match self.types.get(iterable.ty) {
            Type::List(elem) => (hir::IterKind::List, elem),
            Type::Str => (hir::IterKind::Str, TypeId::STR),
            Type::Error => return (self.check_dead_loop_body(var, body), Flow::Falls),
            _ => {
                let found = self.types.name(iterable.ty);
                self.error(
                    Diagnostic::error(format!("cannot iterate over {}", a_type(&found)))
                        .with_label(iter.span, format!("this is {}", a_type(&found)))
                        .with_help("`for` walks a `range(...)`, a `list<T>` or a `str`")
                        .with_note("see DESIGN.md section 3.5"),
                );
                return (self.check_dead_loop_body(var, body), Flow::Falls);
            }
        };
        let Some(var_id) = self.bind_loop_var(var, elem) else {
            return (self.check_dead_loop_body(var, body), Flow::Falls);
        };
        let body = self.check_loop_body(body, var_id);
        (
            Some(hir::Stmt::ForEach {
                var: var_id,
                iter: iterable,
                over,
                body,
            }),
            Flow::Falls,
        )
    }

    /// Declares (or re-uses) the loop variable, which is an ordinary local
    /// (DESIGN §3.4).
    fn bind_loop_var(&mut self, var: &ast::Ident, elem: TypeId) -> Option<hir::LocalId> {
        match self.scope.get(var.as_str()).copied() {
            Some(id) => {
                let declared = self.locals[id.index()].ty;
                if declared != elem {
                    self.mismatch(var.span, declared, elem);
                    return None;
                }
                Some(id)
            }
            None => Some(self.declare_local(var.as_str(), elem, var.span, false)),
        }
    }

    /// Checks a loop body, restoring the entry state afterwards: the body may
    /// run zero times, so nothing it assigns or narrows survives the loop.
    fn check_loop_body(&mut self, body: &ast::Block, var: hir::LocalId) -> hir::Block {
        let entry = self.assigned.clone();
        let entry_narrow = self.narrowed.clone();
        self.sync_assigned();
        self.assigned[var.index()] = true;
        self.forget_narrowing_assigned_in(body);
        self.loop_depth += 1;
        let (body, _) = self.check_block(body);
        self.loop_depth -= 1;
        self.assigned = entry;
        self.narrowed = entry_narrow;
        self.sync_assigned();
        body
    }

    /// Checks the body of a loop whose header was rejected, so that mistakes
    /// inside it are still reported, and drops the result.
    ///
    /// The loop variable is bound with the error type first, so that using it
    /// in the body does not add a second diagnostic about the same mistake.
    fn check_dead_loop_body(&mut self, var: &ast::Ident, body: &ast::Block) -> Option<hir::Stmt> {
        self.declare_poisoned_local(var);
        self.loop_depth += 1;
        let _ = self.check_block(body);
        self.loop_depth -= 1;
        None
    }

    /// Whether `iter` is a call to the builtin `range`.
    fn is_range_call(&self, iter: &ast::Expr) -> bool {
        let ast::ExprKind::Call { callee, .. } = &iter.kind else {
            return false;
        };
        matches!(&callee.kind, ast::ExprKind::Name(n) if n.as_str() == "range")
            && !self.scope.contains_key("range")
    }

    /// Checks the `range(...)` header of a `for` loop (DESIGN §3.5).
    fn check_range(&mut self, iter: &ast::Expr) -> Option<(hir::Expr, hir::Expr, hir::Expr)> {
        let ast::ExprKind::Call {
            type_args, args, ..
        } = &iter.kind
        else {
            return None;
        };
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

/// Collects every name a block assigns, at any depth, so that a loop can
/// forget the narrowing of the locals its body may overwrite.
fn assigned_names(block: &ast::Block, out: &mut std::collections::HashSet<String>) {
    fn target_names(expr: &ast::Expr, out: &mut std::collections::HashSet<String>) {
        match &expr.kind {
            ast::ExprKind::Name(name) => {
                out.insert(name.name.clone());
            }
            ast::ExprKind::Tuple(items) => {
                for item in items {
                    target_names(item, out);
                }
            }
            _ => {}
        }
    }
    for stmt in &block.stmts {
        match &stmt.kind {
            ast::StmtKind::Assign { target, .. } => target_names(target, out),
            ast::StmtKind::AnnAssign { name, .. } => {
                out.insert(name.name.clone());
            }
            ast::StmtKind::If {
                then, elifs, else_, ..
            } => {
                assigned_names(then, out);
                for elif in elifs {
                    assigned_names(&elif.body, out);
                }
                if let Some(else_) = else_ {
                    assigned_names(else_, out);
                }
            }
            ast::StmtKind::While { body, .. } => assigned_names(body, out),
            ast::StmtKind::For { var, body, .. } => {
                out.insert(var.name.clone());
                assigned_names(body, out);
            }
            _ => {}
        }
    }
}
