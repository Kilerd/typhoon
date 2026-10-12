//! Comprehensions: `[elem for x in xs if cond]`, `{elem for ...}` and
//! `{key: value for ...}` (DESIGN §3.10).
//!
//! A comprehension lowers to ordinary HIR statements inside a
//! [`hir::ExprKind::Seq`], so neither backend needs to know about it:
//!
//! ```text
//! { acc = list(capacity=n)        # or an empty list / set / dict
//!   for x in xs:                  # one loop per `for` clause, outermost first
//!       if cond:                  # one `if` per `if` clause
//!           acc.append(elem)      # s.add(elem) / d[key] = value
//!   ; acc }
//! ```
//!
//! The clauses accept what the statements accept: a `for` walks a
//! `range(...)` (a counting loop that allocates nothing), a `list<T>`, a
//! `str`, or the keys of a `dict<K, V>` / `set<T>` (a snapshot, as for a `for`
//! statement), and an `if` takes a `bool`. A set element or a dict key must be
//! hashable, exactly as in a literal; a dict comprehension evaluates the key
//! before the value and keeps the last value of a repeated key, as Python
//! does. An `if` clause narrows like an `if`
//! statement, for the clauses after it and the element, so
//! `[x for x in xs if x is not None]` is a `list<T>` (DESIGN §3.8).
//!
//! The comprehension variables are scoped to the comprehension, as in
//! Python 3: they are gone once it ends, and one that reuses the name of a
//! variable (or a constant) in scope hides it only inside the comprehension —
//! the outer binding is untouched, so this is the one place a name may be
//! bound twice in a function (DESIGN §3.4, §3.10).
//!
//! When the final length is known before the loop runs — a single `for`
//! clause and no `if`, over a `list`, a `str` or a `range` with a literal
//! step — the list is allocated at that capacity, so the appends never regrow
//! it; the iterable (and the bounds of a `range`) are then evaluated once,
//! into hidden temporaries, before the allocation.

use typhoon_ast as ast;
use typhoon_diag::{Diagnostic, Span};

use crate::check::{Checker, a_type};
use crate::hir;
use crate::types::{Type, TypeId};

/// What a comprehension builds.
#[derive(Clone, Copy)]
enum CompKind<'e> {
    /// `[elem for ...]`.
    List,
    /// `{elem for ...}`.
    Set,
    /// `{key: value for ...}`; the element expression is the key and this is
    /// the value.
    Dict(&'e ast::Expr),
}

/// State threaded through the clauses of one comprehension.
struct Comp<'e> {
    /// The hidden local the container is built in.
    acc: hir::LocalId,
    /// What is built.
    kind: CompKind<'e>,
    /// The element (or key) type the context expects, from an annotation.
    hint: Option<TypeId>,
    /// The value type the context expects, for a dict comprehension.
    value_hint: Option<TypeId>,
    /// The element (or key) type, once the element has been checked.
    elem: Option<TypeId>,
    /// The value type of a dict comprehension, once the value has been
    /// checked.
    value: Option<TypeId>,
    /// Every comprehension variable bound so far, with the binding it hides
    /// until the comprehension ends.
    bound: Vec<(String, Option<hir::LocalId>)>,
    /// Statements run before the list is allocated: the temporaries a
    /// pre-sized loop evaluates its iterable into.
    prelude: Vec<hir::Stmt>,
    /// The capacity to allocate, when the length is known up front.
    capacity: Option<hir::Expr>,
}

impl Checker<'_> {
    /// `[elem for … ]`; `hint` is the `list<T>` the context expects, if any.
    pub(crate) fn check_list_comp(
        &mut self,
        elem: &ast::Expr,
        clauses: &[ast::CompClause],
        hint: Option<TypeId>,
        span: Span,
    ) -> hir::Expr {
        self.check_comp(CompKind::List, elem, clauses, hint, span)
    }

    /// `{elem for … }`; `hint` is the `set<T>` the context expects, if any.
    pub(crate) fn check_set_comp(
        &mut self,
        elem: &ast::Expr,
        clauses: &[ast::CompClause],
        hint: Option<TypeId>,
        span: Span,
    ) -> hir::Expr {
        self.check_comp(CompKind::Set, elem, clauses, hint, span)
    }

    /// `{key: value for … }`; `hint` is the `dict<K, V>` the context expects,
    /// if any.
    pub(crate) fn check_dict_comp(
        &mut self,
        key: &ast::Expr,
        value: &ast::Expr,
        clauses: &[ast::CompClause],
        hint: Option<TypeId>,
        span: Span,
    ) -> hir::Expr {
        self.check_comp(CompKind::Dict(value), key, clauses, hint, span)
    }

    fn check_comp(
        &mut self,
        kind: CompKind<'_>,
        elem: &ast::Expr,
        clauses: &[ast::CompClause],
        hint: Option<TypeId>,
        span: Span,
    ) -> hir::Expr {
        let narrowed = self.narrowed.clone();
        let acc = self.declare_hidden("comp", TypeId::ERROR, span);
        let (elem_hint, value_hint) = match kind {
            CompKind::List => (hint.and_then(|ty| self.types.as_list(ty)), None),
            CompKind::Set => (hint.and_then(|ty| self.types.as_set(ty)), None),
            CompKind::Dict(_) => match hint.and_then(|ty| self.types.as_dict(ty)) {
                Some((key, value)) => (Some(key), Some(value)),
                None => (None, None),
            },
        };
        let mut comp = Comp {
            acc,
            kind,
            hint: elem_hint,
            value_hint,
            elem: None,
            value: None,
            bound: Vec::new(),
            prelude: Vec::new(),
            capacity: None,
        };
        // Only a lone `for` has a length known before the loop runs, and only
        // a list is allocated at a capacity.
        let presize =
            matches!(kind, CompKind::List) && matches!(clauses, [ast::CompClause::For { .. }]);
        let body = self.comp_clauses(clauses, elem, &mut comp, presize);

        // The comprehension's scope ends here.
        for (name, previous) in comp.bound.drain(..).rev() {
            match previous {
                Some(id) => self.scope.insert(name, id),
                None => self.scope.remove(&name),
            };
        }
        self.narrowed = narrowed;

        let (Some(body), Some(elem)) = (body, comp.elem) else {
            return self.error_expr(span);
        };
        let (ty, alloc) = match kind {
            CompKind::List => {
                let alloc = match comp.capacity {
                    Some(cap) => hir::ExprKind::ListWithCapacity {
                        elem,
                        cap: Box::new(cap),
                    },
                    None => hir::ExprKind::ListNew {
                        elem,
                        items: Vec::new(),
                    },
                };
                (self.types.list_of(elem), alloc)
            }
            CompKind::Set => (
                self.types.set_of(elem),
                hir::ExprKind::SetNew {
                    elem,
                    items: Vec::new(),
                },
            ),
            CompKind::Dict(_) => {
                let Some(value) = comp.value else {
                    return self.error_expr(span);
                };
                (
                    self.types.dict_of(elem, value),
                    hir::ExprKind::DictNew {
                        key: elem,
                        value,
                        entries: Vec::new(),
                    },
                )
            }
        };
        let mut stmts = comp.prelude;
        stmts.push(hir::Stmt::Assign {
            local: acc,
            value: hir::Expr {
                kind: alloc,
                ty,
                span,
            },
        });
        stmts.push(body);
        hir::Expr {
            kind: hir::ExprKind::Seq {
                stmts: hir::Block { stmts },
                value: Box::new(hir::Expr {
                    kind: hir::ExprKind::Local(acc),
                    ty,
                    span,
                }),
            },
            ty,
            span,
        }
    }

    /// Lowers `clauses` (and, innermost, the append of `elem`) into one
    /// statement, or `None` after an error. Every clause is checked even after
    /// an error, so that each mistake is reported.
    fn comp_clauses(
        &mut self,
        clauses: &[ast::CompClause],
        elem: &ast::Expr,
        comp: &mut Comp<'_>,
        presize: bool,
    ) -> Option<hir::Stmt> {
        let Some((clause, rest)) = clauses.split_first() else {
            return self.comp_append(elem, comp);
        };
        match clause {
            ast::CompClause::If { cond, .. } => {
                let narrow = self.narrowing_of(cond);
                let cond = self.check_condition(cond);
                self.apply_narrowing(narrow, true);
                let then = self.comp_clauses(rest, elem, comp, false)?;
                if cond.ty == TypeId::ERROR {
                    return None;
                }
                Some(hir::Stmt::If {
                    cond,
                    then: hir::Block { stmts: vec![then] },
                    else_: None,
                })
            }
            ast::CompClause::For { var, iter, .. } => {
                if self.is_range_call(iter) {
                    let header = self.check_range(iter);
                    let id = self.bind_comp_var(var, TypeId::INT, comp);
                    let body = self.comp_clauses(rest, elem, comp, false);
                    let (start, stop, step) = header?;
                    let (start, stop) = if presize {
                        self.presize_range(start, stop, &step, comp)
                    } else {
                        (start, stop)
                    };
                    return Some(hir::Stmt::ForRange {
                        var: id?,
                        start,
                        stop,
                        step,
                        body: hir::Block { stmts: vec![body?] },
                    });
                }
                let iterable = self.check_expr(iter);
                // A dict or a set is walked through a snapshot of its keys.
                let iterable = self.keys_snapshot(iterable);
                let walk = match self.types.get(iterable.ty) {
                    Type::List(elem) => Some((hir::IterKind::List, elem)),
                    Type::Str => Some((hir::IterKind::Str, TypeId::STR)),
                    Type::Error => None,
                    Type::Optional(_) => {
                        self.require_narrowed(iterable.span, iterable.ty);
                        None
                    }
                    _ => {
                        let found = self.types.name(iterable.ty);
                        self.error(
                            Diagnostic::error(format!("cannot iterate over {}", a_type(&found)))
                                .with_label(iter.span, format!("this is {}", a_type(&found)))
                                .with_help(
                                    "a comprehension walks a `range(...)`, a `list<T>` or a `str`, \
                                     or the keys of a `dict<K, V>` or a `set<T>`",
                                )
                                .with_note("see DESIGN.md section 3.10"),
                        );
                        None
                    }
                };
                let var_ty = walk.map_or(TypeId::ERROR, |(_, elem)| elem);
                let id = self.bind_comp_var(var, var_ty, comp);
                let body = self.comp_clauses(rest, elem, comp, false);
                let (over, _) = walk?;
                let iterable = if presize {
                    self.presize_iterable(iterable, over, comp)
                } else {
                    iterable
                };
                Some(hir::Stmt::ForEach {
                    var: id?,
                    iter: iterable,
                    over,
                    body: hir::Block { stmts: vec![body?] },
                })
            }
        }
    }

    /// The innermost statement: `acc.append(elem)`. Fixes the element type,
    /// from the annotation when there is one (so `list<int | None>` widens),
    /// otherwise from the element itself.
    fn comp_append(&mut self, elem: &ast::Expr, comp: &mut Comp<'_>) -> Option<hir::Stmt> {
        if !matches!(comp.kind, CompKind::List) {
            return self.comp_insert(elem, comp);
        }
        let value = self.check_expr_hint(elem, comp.hint);
        if value.ty == TypeId::ERROR {
            return None;
        }
        let elem_ty = match comp.hint {
            Some(hint) if hint != TypeId::ERROR => {
                if !self.assignable(hint, value.ty) {
                    self.mismatch(value.span, hint, value.ty);
                    return None;
                }
                hint
            }
            _ => value.ty,
        };
        if elem_ty == TypeId::UNIT {
            self.error(
                Diagnostic::error("a list element cannot have type `None`")
                    .with_label(value.span, "this expression produces no value"),
            );
            return None;
        }
        let value = self.coerce(value, elem_ty);
        let list_ty = self.types.list_of(elem_ty);
        self.locals[comp.acc.index()].ty = list_ty;
        comp.elem = Some(elem_ty);
        let span = value.span;
        Some(hir::Stmt::Expr(hir::Expr {
            kind: hir::ExprKind::ListAppend {
                list: Box::new(hir::Expr {
                    kind: hir::ExprKind::Local(comp.acc),
                    ty: list_ty,
                    span,
                }),
                value: Box::new(value),
            },
            ty: TypeId::UNIT,
            span,
        }))
    }

    /// The innermost statement of a set or dict comprehension: `acc.add(elem)`
    /// or `acc[key] = value`, fixing the element (key) and value types from
    /// the annotation when there is one, otherwise from the expressions.
    fn comp_insert(&mut self, elem: &ast::Expr, comp: &mut Comp<'_>) -> Option<hir::Stmt> {
        let (container, what) = match comp.kind {
            CompKind::Dict(_) => ("dict", "a dict key"),
            _ => ("set", "a set element"),
        };
        let key = self.comp_member(elem, comp.hint, what);
        let value = match comp.kind {
            CompKind::Dict(value) => Some(self.comp_member(value, comp.value_hint, "a dict value")),
            _ => None,
        };
        let key = key?;
        if !self.require_key_type(key.ty, key.span, container) {
            return None;
        }
        let span = key.span;
        let elem_ty = key.ty;
        comp.elem = Some(elem_ty);
        let local = |ty| hir::Expr {
            kind: hir::ExprKind::Local(comp.acc),
            ty,
            span,
        };
        match value {
            None => {
                let set_ty = self.types.set_of(elem_ty);
                self.locals[comp.acc.index()].ty = set_ty;
                Some(hir::Stmt::Expr(hir::Expr {
                    kind: hir::ExprKind::SetAdd {
                        set: Box::new(local(set_ty)),
                        value: Box::new(key),
                    },
                    ty: TypeId::UNIT,
                    span,
                }))
            }
            Some(value) => {
                let value = value?;
                let dict_ty = self.types.dict_of(elem_ty, value.ty);
                self.locals[comp.acc.index()].ty = dict_ty;
                comp.value = Some(value.ty);
                Some(hir::Stmt::DictSet {
                    dict: local(dict_ty),
                    key,
                    value,
                })
            }
        }
    }

    /// Checks a set element, a dict key or a dict value against the type the
    /// annotation gives it, if any, and coerces it to that type.
    fn comp_member(
        &mut self,
        expr: &ast::Expr,
        hint: Option<TypeId>,
        what: &str,
    ) -> Option<hir::Expr> {
        let value = self.check_expr_hint(expr, hint);
        if value.ty == TypeId::ERROR {
            return None;
        }
        let ty = match hint {
            Some(hint) if hint != TypeId::ERROR => {
                if !self.assignable(hint, value.ty) {
                    self.mismatch(value.span, hint, value.ty);
                    return None;
                }
                hint
            }
            _ => value.ty,
        };
        if ty == TypeId::UNIT {
            self.error(
                Diagnostic::error(format!("{what} cannot have type `None`"))
                    .with_label(value.span, "this expression produces no value"),
            );
            return None;
        }
        Some(self.coerce(value, ty))
    }

    /// Binds a comprehension variable for the rest of the comprehension,
    /// hiding any variable of the same name until the comprehension ends.
    ///
    /// The HIR local is a fresh one named `<name>.<index>`: the outer variable
    /// keeps its own slot and value, and the same name may be bound again by a
    /// later comprehension of the same function.
    fn bind_comp_var(
        &mut self,
        var: &ast::Ident,
        ty: TypeId,
        comp: &mut Comp<'_>,
    ) -> Option<hir::LocalId> {
        let name = var.as_str();
        let id = self.declare_hidden(name, ty, var.span);
        let previous = self.scope.insert(name.to_string(), id);
        comp.bound.push((name.to_string(), previous));
        (ty != TypeId::ERROR).then_some(id)
    }

    /// Records the capacity of a lone `for` over a list or a `str`, which is
    /// the length of the iterable, evaluating the iterable once into a
    /// temporary when reading it twice would not be free.
    fn presize_iterable(
        &mut self,
        iterable: hir::Expr,
        over: hir::IterKind,
        comp: &mut Comp<'_>,
    ) -> hir::Expr {
        let iterable = self.comp_temp(iterable, comp);
        let len = match over {
            hir::IterKind::List => hir::ExprKind::ListLen(Box::new(iterable.clone())),
            hir::IterKind::Str => hir::ExprKind::StrLen(Box::new(iterable.clone())),
        };
        comp.capacity = Some(hir::Expr {
            kind: len,
            ty: TypeId::INT,
            span: iterable.span,
        });
        iterable
    }

    /// Records the capacity of a lone `for` over `range(start, stop, step)`
    /// with a literal step: `ceil((stop - start) / step)`, clamped at zero by
    /// the allocation. A step only known at run time is not pre-sized.
    fn presize_range(
        &mut self,
        start: hir::Expr,
        stop: hir::Expr,
        step: &hir::Expr,
        comp: &mut Comp<'_>,
    ) -> (hir::Expr, hir::Expr) {
        let step = match step.kind {
            hir::ExprKind::Int(v) if v != 0 && v != i64::MIN => v,
            _ => return (start, stop),
        };
        let start = self.comp_temp(start, comp);
        let stop = self.comp_temp(stop, comp);
        let span = stop.span;
        let int = |kind| hir::Expr {
            kind,
            ty: TypeId::INT,
            span,
        };
        let bin = |op, lhs: hir::Expr, rhs: hir::Expr| {
            int(hir::ExprKind::IntBin {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            })
        };
        // The distance covered, in the direction of the step.
        let (distance, stride) = if step > 0 {
            (bin(hir::IntOp::Sub, stop.clone(), start.clone()), step)
        } else {
            (bin(hir::IntOp::Sub, start.clone(), stop.clone()), -step)
        };
        let cap = if stride == 1 {
            distance
        } else {
            let rounded = bin(
                hir::IntOp::Add,
                distance,
                int(hir::ExprKind::Int(stride - 1)),
            );
            bin(
                hir::IntOp::FloorDiv,
                rounded,
                int(hir::ExprKind::Int(stride)),
            )
        };
        comp.capacity = Some(cap);
        (start, stop)
    }

    /// `value` itself when reading it is free (a literal or a local),
    /// otherwise a read of a hidden temporary that the prelude assigns it to.
    fn comp_temp(&mut self, value: hir::Expr, comp: &mut Comp<'_>) -> hir::Expr {
        if matches!(
            value.kind,
            hir::ExprKind::Local(_) | hir::ExprKind::Int(_) | hir::ExprKind::Str(_)
        ) {
            return value;
        }
        let (ty, span) = (value.ty, value.span);
        let temp = self.declare_hidden("iter", ty, span);
        comp.prelude.push(hir::Stmt::Assign { local: temp, value });
        hir::Expr {
            kind: hir::ExprKind::Local(temp),
            ty,
            span,
        }
    }
}
