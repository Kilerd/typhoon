//! The protocols behind `==`, `<`, `hash`, `repr` and `print` on every type
//! (DESIGN §3.11), and the functions the checker writes itself.
//!
//! * [`Checker::ensure_eq`] and friends say whether a type supports a
//!   protocol *structurally*: scalars natively, containers element-wise, a
//!   class when it has the dunder method with the canonical signature
//!   (`__eq__(self, other: C) -> bool`, …). They also record, for codegen,
//!   which `__eq__` / `__repr__` to call for a class inside a container
//!   ([`hir::Protocols`]).
//! * [`Checker::eq_values`], [`Checker::lt_values`], [`Checker::hash_value`]
//!   and [`Checker::repr_value`] lower one operation on already-checked
//!   operands to HIR.
//! * Derived methods and a few helpers (tuple `<`, tuple and optional hashes,
//!   `min` / `max` and `sorted` over an `Ord` type, the probe function of a
//!   class used as a dict key) are written directly as HIR: their signature
//!   is registered when they are first needed, and the body is built by
//!   [`Checker::build_synth`] when the instance queue is drained — so the
//!   codegens never learn that traits exist.
//! * [`Checker::register_class_keys`] runs once every body is checked and
//!   gives every class used as a key its `__hash__` and probe function in
//!   [`hir::Protocols`].

use typhoon_diag::Span;

use crate::check::{Checker, FnSig, ParamSig};
use crate::generics::symbol_safe;
use crate::hir;
use crate::traits::{Builtin, SynthJob, SynthKind};
use crate::types::{ClassId, Type, TypeId};

/// FNV-1a's 64-bit offset basis and prime, the field / member hash mixer.
const HASH_SEED: i64 = 0xcbf2_9ce4_8422_2325_u64 as i64;
const HASH_PRIME: i64 = 0x0000_0100_0000_01b3;

/// Small constructors for synthesized HIR.
fn expr(kind: hir::ExprKind, ty: TypeId, span: Span) -> hir::Expr {
    hir::Expr { kind, ty, span }
}

fn local(id: hir::LocalId, ty: TypeId, span: Span) -> hir::Expr {
    expr(hir::ExprKind::Local(id), ty, span)
}

fn int(value: i64, span: Span) -> hir::Expr {
    expr(hir::ExprKind::Int(value), TypeId::INT, span)
}

fn boolean(value: bool, span: Span) -> hir::Expr {
    expr(hir::ExprKind::Bool(value), TypeId::BOOL, span)
}

fn not(value: hir::Expr) -> hir::Expr {
    let span = value.span;
    expr(hir::ExprKind::Not(Box::new(value)), TypeId::BOOL, span)
}

fn and(lhs: hir::Expr, rhs: hir::Expr) -> hir::Expr {
    let span = lhs.span;
    expr(
        hir::ExprKind::And {
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
        TypeId::BOOL,
        span,
    )
}

fn or(lhs: hir::Expr, rhs: hir::Expr) -> hir::Expr {
    let span = lhs.span;
    expr(
        hir::ExprKind::Or {
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
        TypeId::BOOL,
        span,
    )
}

fn int_cmp(op: hir::CmpOp, lhs: hir::Expr, rhs: hir::Expr) -> hir::Expr {
    let span = lhs.span;
    expr(
        hir::ExprKind::IntCmp {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
        TypeId::BOOL,
        span,
    )
}

fn assign(local: hir::LocalId, value: hir::Expr) -> hir::Stmt {
    hir::Stmt::Assign { local, value }
}

/// Adds a local to a synthesized function. The names are distinct within
/// each function, since the native backend names its stack slots after them.
fn push_local(locals: &mut Vec<hir::Local>, name: &str, ty: TypeId, span: Span) -> hir::LocalId {
    locals.push(hir::Local {
        name: name.to_string(),
        ty,
        span,
        is_param: false,
    });
    hir::LocalId(locals.len() as u32 - 1)
}

/// `(acc ^ value) * PRIME`, wrapping.
fn mix(acc: hir::Expr, value: hir::Expr) -> hir::Expr {
    let span = acc.span;
    let xored = expr(
        hir::ExprKind::IntBin {
            op: hir::IntOp::BitXor,
            lhs: Box::new(acc),
            rhs: Box::new(value),
        },
        TypeId::INT,
        span,
    );
    expr(
        hir::ExprKind::IntBin {
            op: hir::IntOp::Mul,
            lhs: Box::new(xored),
            rhs: Box::new(int(HASH_PRIME, span)),
        },
        TypeId::INT,
        span,
    )
}

fn ret(value: hir::Expr) -> hir::Stmt {
    hir::Stmt::Return(Some(value))
}

/// `if cond: then`.
fn when(cond: hir::Expr, then: Vec<hir::Stmt>) -> hir::Stmt {
    hir::Stmt::If {
        cond,
        then: hir::Block { stmts: then },
        else_: None,
    }
}

impl Checker<'_> {
    // -- method lookup --------------------------------------------------------

    /// The method `name` of `class`, instantiating (or deriving) it when the
    /// class is an instance of a generic class.
    pub(crate) fn class_method(
        &mut self,
        class: ClassId,
        name: &str,
        at: Span,
    ) -> Option<hir::FuncId> {
        if let Some(func) = self.methods.get(&(class, name.to_string())) {
            return Some(*func);
        }
        if self.generics.origins.contains_key(&class) {
            return self.instantiate_method(class, &typhoon_ast::Ident::new(name, at));
        }
        None
    }

    /// The dunder of the builtin trait `builtin` on `class`, when it has the
    /// canonical signature: `(self, other: C) -> bool` for `Eq` / `Ord`,
    /// `(self) -> int` for `Hash`, `(self) -> str` for `Repr`.
    pub(crate) fn proto_method(
        &mut self,
        class: ClassId,
        builtin: Builtin,
        at: Span,
    ) -> Option<hir::FuncId> {
        if self.traits.derive_failed.contains(&(class, builtin)) {
            return None;
        }
        let func = self.class_method(class, builtin.method(), at)?;
        let class_ty = self.types.class_ty(class);
        let want_ret = match builtin {
            Builtin::Eq | Builtin::Ord => TypeId::BOOL,
            Builtin::Hash => TypeId::INT,
            Builtin::Repr => TypeId::STR,
            _ => class_ty,
        };
        let want: &[TypeId] = if builtin.binary() {
            &[class_ty, class_ty]
        } else {
            &[class_ty]
        };
        let sig = &self.sigs[func.index()];
        let params: Vec<TypeId> = sig.params.iter().map(|p| p.ty).collect();
        (params == want && sig.ret == want_ret).then_some(func)
    }

    // -- structural support ---------------------------------------------------

    /// Whether `==` is defined on `ty`, recording the class `__eq__`s codegen
    /// needs inside containers.
    pub(crate) fn ensure_eq(&mut self, ty: TypeId, at: Span) -> bool {
        match self.types.get(ty) {
            Type::Error | Type::Int | Type::Float | Type::Bool | Type::Str => true,
            // Dicts and sets have no `==` yet (DESIGN §4.6).
            Type::Unit | Type::Dict(..) | Type::Set(_) => false,
            Type::List(elem) | Type::Optional(elem) => self.ensure_eq(elem, at),
            Type::Tuple(id) => {
                let members = self.types.tuple_members(id).to_vec();
                members.into_iter().all(|m| self.ensure_eq(m, at))
            }
            Type::Class(class) => match self.proto_method(class, Builtin::Eq, at) {
                Some(func) => {
                    self.traits.eq_fns.insert(class, func);
                    true
                }
                None => false,
            },
        }
    }

    /// Whether `print` can render `ty`, recording the class `__repr__`s
    /// codegen needs inside containers.
    pub(crate) fn ensure_repr(&mut self, ty: TypeId, at: Span) -> bool {
        match self.types.get(ty) {
            Type::Error | Type::Int | Type::Float | Type::Bool | Type::Str => true,
            Type::Unit => false,
            Type::List(elem) | Type::Optional(elem) | Type::Set(elem) => self.ensure_repr(elem, at),
            Type::Dict(key, value) => self.ensure_repr(key, at) && self.ensure_repr(value, at),
            Type::Tuple(id) => {
                let members = self.types.tuple_members(id).to_vec();
                members.into_iter().all(|m| self.ensure_repr(m, at))
            }
            Type::Class(class) => match self.proto_method(class, Builtin::Repr, at) {
                Some(func) => {
                    self.traits.repr_fns.insert(class, func);
                    true
                }
                None => false,
            },
        }
    }

    /// Whether `<` (and with it `==`) is defined on `ty`: lexicographically on
    /// tuples, through `__lt__` and `__eq__` on a class.
    pub(crate) fn ensure_ord(&mut self, ty: TypeId, at: Span) -> bool {
        match self.types.get(ty) {
            Type::Error | Type::Int | Type::Float | Type::Bool | Type::Str => true,
            Type::Tuple(id) => {
                let members = self.types.tuple_members(id).to_vec();
                members
                    .into_iter()
                    .all(|m| self.ensure_ord(m, at) && self.ensure_eq(m, at))
            }
            Type::Class(class) => {
                self.proto_method(class, Builtin::Ord, at).is_some() && self.ensure_eq(ty, at)
            }
            Type::Unit | Type::List(_) | Type::Optional(_) | Type::Dict(..) | Type::Set(_) => false,
        }
    }

    /// Whether `hash(x)` is defined on `ty`: exactly the key types of a dict
    /// in M3 — `int`, `bool`, `str`, flat tuples of those, and a class with
    /// `__hash__` (DESIGN §3.11). A `T | None` and a tuple holding a class
    /// are deferred along with them as keys.
    pub(crate) fn ensure_hash(&mut self, ty: TypeId, at: Span) -> bool {
        match self.types.get(ty) {
            Type::Error | Type::Int | Type::Bool | Type::Str => true,
            Type::Tuple(_) => self.types.is_hashable(ty),
            Type::Class(class) => self.proto_method(class, Builtin::Hash, at).is_some(),
            Type::Unit
            | Type::Float
            | Type::Optional(_)
            | Type::List(_)
            | Type::Dict(..)
            | Type::Set(_) => false,
        }
    }

    // -- lowering ---------------------------------------------------------------

    /// A call of a protocol method (or helper) `func` with `args` in
    /// parameter order, evaluated in `eval_order`.
    pub(crate) fn call_fn(
        &self,
        func: hir::FuncId,
        args: Vec<hir::Expr>,
        eval_order: Vec<u32>,
        span: Span,
    ) -> hir::Expr {
        let ty = self.sigs[func.index()].ret;
        expr(
            hir::ExprKind::Call {
                func,
                args,
                eval_order,
            },
            ty,
            span,
        )
    }

    /// `a == b` for two values of the same type, after
    /// [`Checker::ensure_eq`] accepted it.
    pub(crate) fn eq_values(&mut self, a: hir::Expr, b: hir::Expr, span: Span) -> hir::Expr {
        let op = hir::CmpOp::Eq;
        let (lhs, rhs) = (Box::new(a), Box::new(b));
        let kind = match self.types.get(lhs.ty) {
            Type::Int => hir::ExprKind::IntCmp { op, lhs, rhs },
            Type::Float => hir::ExprKind::FloatCmp { op, lhs, rhs },
            Type::Bool => hir::ExprKind::BoolCmp { op, lhs, rhs },
            Type::Str => hir::ExprKind::StrCmp { op, lhs, rhs },
            Type::Class(class) => {
                return match self.proto_method(class, Builtin::Eq, span) {
                    Some(func) => self.call_fn(func, vec![*lhs, *rhs], vec![0, 1], span),
                    None => boolean(false, span),
                };
            }
            _ => hir::ExprKind::StructEq { op, lhs, rhs },
        };
        expr(kind, TypeId::BOOL, span)
    }

    /// `a < b` for two values of the same type, after
    /// [`Checker::ensure_ord`] accepted it.
    pub(crate) fn lt_values(&mut self, a: hir::Expr, b: hir::Expr, span: Span) -> hir::Expr {
        self.ordered_values(hir::CmpOp::Lt, a, b, span)
    }

    /// `a op b` for an ordering `op` and two values of the same type, after
    /// [`Checker::ensure_ord`] accepted it: natively on scalars, member-wise
    /// on tuples, and through `__lt__` on a class — the only type where
    /// `a <= b` is `not (b < a)` (DESIGN §3.11).
    pub(crate) fn ordered_values(
        &mut self,
        op: hir::CmpOp,
        a: hir::Expr,
        b: hir::Expr,
        span: Span,
    ) -> hir::Expr {
        let ty = a.ty;
        let (lhs, rhs) = (Box::new(a), Box::new(b));
        let kind = match self.types.get(ty) {
            Type::Int => hir::ExprKind::IntCmp { op, lhs, rhs },
            Type::Float => hir::ExprKind::FloatCmp { op, lhs, rhs },
            Type::Bool => hir::ExprKind::BoolCmp { op, lhs, rhs },
            Type::Str => hir::ExprKind::StrCmp { op, lhs, rhs },
            Type::Class(class) => {
                return match self.proto_method(class, Builtin::Ord, span) {
                    Some(func) => self.ordered_call(func, op, *lhs, *rhs, span),
                    None => boolean(false, span),
                };
            }
            _ => {
                let func = self.synth_fn(SynthKind::TupleOrder(op), ty, span);
                return self.call_fn(func, vec![*lhs, *rhs], vec![0, 1], span);
            }
        };
        expr(kind, TypeId::BOOL, span)
    }

    /// `op` (an ordering) through a `<` function `func`: `a > b` is `b < a`,
    /// `a <= b` is `not (b < a)` and `a >= b` is `not (a < b)`; the operands
    /// are still evaluated left to right.
    pub(crate) fn ordered_call(
        &self,
        func: hir::FuncId,
        op: hir::CmpOp,
        lhs: hir::Expr,
        rhs: hir::Expr,
        span: Span,
    ) -> hir::Expr {
        let (args, order) = match op {
            hir::CmpOp::Gt | hir::CmpOp::Le => (vec![rhs, lhs], vec![1, 0]),
            _ => (vec![lhs, rhs], vec![0, 1]),
        };
        let call = self.call_fn(func, args, order, span);
        match op {
            hir::CmpOp::Le | hir::CmpOp::Ge => not(call),
            _ => call,
        }
    }

    /// `hash(v)` as an `int`, after [`Checker::ensure_hash`] accepted it.
    pub(crate) fn hash_value(&mut self, value: hir::Expr, span: Span) -> hir::Expr {
        match self.types.get(value.ty) {
            Type::Int => value,
            Type::Bool | Type::Str => expr(hir::ExprKind::Hash(Box::new(value)), TypeId::INT, span),
            Type::Class(class) => match self.proto_method(class, Builtin::Hash, span) {
                Some(func) => self.call_fn(func, vec![value], vec![0], span),
                None => int(0, span),
            },
            Type::Tuple(_) => {
                let func = self.synth_fn(SynthKind::TupleHash, value.ty, span);
                self.call_fn(func, vec![value], vec![0], span)
            }
            _ => int(0, span),
        }
    }

    /// `repr(v)` as a `str`, after [`Checker::ensure_repr`] accepted it: what
    /// `print` writes for `v` inside a container.
    pub(crate) fn repr_value(&mut self, value: hir::Expr, span: Span) -> hir::Expr {
        match self.types.get(value.ty) {
            Type::Int | Type::Float | Type::Bool => {
                expr(hir::ExprKind::StrFrom(Box::new(value)), TypeId::STR, span)
            }
            Type::Class(class) => match self.proto_method(class, Builtin::Repr, span) {
                Some(func) => self.call_fn(func, vec![value], vec![0], span),
                None => expr(hir::ExprKind::Str(String::new()), TypeId::STR, span),
            },
            _ => expr(hir::ExprKind::Repr(Box::new(value)), TypeId::STR, span),
        }
    }

    // -- synthesized functions ------------------------------------------------

    /// Registers the derived `builtin` method of `class`; its body is built
    /// when the queue is drained.
    pub(crate) fn register_derived(
        &mut self,
        class: ClassId,
        builtin: Builtin,
        span: Span,
    ) -> hir::FuncId {
        let class_ty = self.types.class_ty(class);
        let class_name = self.types.class(class).name.clone();
        let method = builtin.method();
        let mut params = vec![ParamSig {
            name: "self".to_string(),
            ty: class_ty,
            span,
            default: None,
        }];
        if builtin.binary() {
            params.push(ParamSig {
                name: "other".to_string(),
                ty: class_ty,
                span,
                default: None,
            });
        }
        let ret = match builtin {
            Builtin::Eq | Builtin::Ord => TypeId::BOOL,
            Builtin::Hash => TypeId::INT,
            _ => TypeId::STR,
        };
        let id = hir::FuncId(self.sigs.len() as u32);
        self.sigs.push(FnSig {
            name: format!("{class_name}.{method}"),
            symbol: symbol_safe(&format!("ty_user_{class_name}__{method}")),
            params,
            ret,
            name_span: span,
            self_ty: Some(class_ty),
        });
        self.methods.insert((class, method.to_string()), id);
        self.traits.pending.push_back(SynthJob {
            id,
            kind: SynthKind::Derive(builtin),
            ty: class_ty,
            span,
        });
        id
    }

    /// The helper `kind` for operands of type `ty`, registered on first use.
    pub(crate) fn synth_fn(&mut self, kind: SynthKind, ty: TypeId, span: Span) -> hir::FuncId {
        if let Some(func) = self.traits.synth.get(&(kind, ty)) {
            return *func;
        }
        let name = self.types.name(ty);
        let (label, params, ret) = match kind {
            SynthKind::TupleOrder(op) => {
                let label = match op {
                    hir::CmpOp::Le => "le",
                    hir::CmpOp::Gt => "gt",
                    hir::CmpOp::Ge => "ge",
                    _ => "lt",
                };
                (label, vec![("a", ty), ("b", ty)], TypeId::BOOL)
            }
            SynthKind::TupleHash => ("hash", vec![("x", ty)], TypeId::INT),
            SynthKind::Min => ("min", vec![("a", ty), ("b", ty)], ty),
            SynthKind::Max => ("max", vec![("a", ty), ("b", ty)], ty),
            SynthKind::KeyProbe => {
                let table = self.types.set_of(ty);
                (
                    "probe",
                    vec![("table", table), ("key", ty), ("hash", TypeId::INT)],
                    TypeId::INT,
                )
            }
            SynthKind::Sort => {
                let list = self.types.list_of(ty);
                ("sorted", vec![("xs", list)], list)
            }
            SynthKind::Derive(_) => unreachable!("derived methods use `register_derived`"),
        };
        let display = format!("{label}<{name}>");
        let id = hir::FuncId(self.sigs.len() as u32);
        self.sigs.push(FnSig {
            name: display.clone(),
            // `ty_builtin_` never collides with a user symbol (`ty_user_`).
            symbol: symbol_safe(&format!("ty_builtin_{display}")),
            params: params
                .into_iter()
                .map(|(name, ty)| ParamSig {
                    name: name.to_string(),
                    ty,
                    span,
                    default: None,
                })
                .collect(),
            ret,
            name_span: span,
            self_ty: None,
        });
        self.traits.synth.insert((kind, ty), id);
        self.traits
            .pending
            .push_back(SynthJob { id, kind, ty, span });
        id
    }

    /// Builds the body of a synthesized function.
    pub(crate) fn build_synth(&mut self, job: SynthJob) -> hir::Function {
        let span = job.span;
        let sig = &self.sigs[job.id.index()];
        let (name, symbol, ret_ty) = (sig.name.clone(), sig.symbol.clone(), sig.ret);
        let mut locals: Vec<hir::Local> = sig
            .params
            .iter()
            .map(|p| hir::Local {
                name: p.name.clone(),
                ty: p.ty,
                span,
                is_param: true,
            })
            .collect();
        let params: Vec<hir::LocalId> = (0..locals.len() as u32).map(hir::LocalId).collect();
        let ty = job.ty;
        let stmts = match job.kind {
            SynthKind::Derive(builtin) => self.derived_body(builtin, ty, span),
            SynthKind::TupleOrder(op) => {
                let (a, b) = (hir::LocalId(0), hir::LocalId(1));
                let members = self.types.as_tuple(ty).unwrap_or_default().to_vec();
                let pairs = members
                    .iter()
                    .enumerate()
                    .map(|(i, m)| (member(a, ty, i, *m, span), member(b, ty, i, *m, span)))
                    .collect();
                self.lexicographic(op, pairs, span)
            }
            SynthKind::TupleHash => {
                let x = hir::LocalId(0);
                let members = self.types.as_tuple(ty).unwrap_or_default().to_vec();
                let mut acc = int(HASH_SEED, span);
                for (i, m) in members.iter().enumerate() {
                    let value = self.hash_value(member(x, ty, i, *m, span), span);
                    acc = mix(acc, value);
                }
                vec![ret(acc)]
            }
            SynthKind::Min | SynthKind::Max => {
                let a = local(hir::LocalId(0), ty, span);
                let b = local(hir::LocalId(1), ty, span);
                // `max(a, b)` is `b` when `a < b`; `min(a, b)` is `b` when
                // `b < a`; ties keep the first argument, as in Python.
                let cond = if job.kind == SynthKind::Max {
                    self.lt_values(a.clone(), b.clone(), span)
                } else {
                    self.lt_values(b.clone(), a.clone(), span)
                };
                vec![when(cond, vec![ret(b)]), ret(a)]
            }
            SynthKind::KeyProbe => self.key_probe_body(ty, &mut locals, span),
            SynthKind::Sort => self.sort_body(ty, &mut locals, span),
        };
        hir::Function {
            name,
            symbol,
            params,
            ret: ret_ty,
            locals,
            body: hir::Block { stmts },
            span,
        }
    }

    /// `for each pair: if a != b: return a op b`, then whether `op` holds for
    /// equal sequences (`<=` and `>=` do): the lexicographic ordering of
    /// tuples, as Python compares them, and the `<` of derived `Ord`.
    fn lexicographic(
        &mut self,
        op: hir::CmpOp,
        pairs: Vec<(hir::Expr, hir::Expr)>,
        span: Span,
    ) -> Vec<hir::Stmt> {
        let mut stmts = Vec::with_capacity(pairs.len() + 1);
        for (a, b) in pairs {
            let differ = not(self.eq_values(a.clone(), b.clone(), span));
            let decided = self.ordered_values(op, a, b, span);
            stmts.push(when(differ, vec![ret(decided)]));
        }
        let equal = matches!(op, hir::CmpOp::Le | hir::CmpOp::Ge);
        stmts.push(ret(boolean(equal, span)));
        stmts
    }

    /// The probe function of the key class `class_ty` (see
    /// [`hir::Protocols::key_probe`] and `crates/runtime/src/dict.rs`):
    ///
    /// ```text
    /// fn probe(table: set<C>, key: C, hash: int) -> int:
    ///     pos = probe(table, hash, -1)
    ///     while pos >= 0:
    ///         stored = key_at(table, pos)
    ///         if stored is key or stored == key: return pos
    ///         pos = probe(table, hash, pos)
    ///     return -1
    /// ```
    ///
    /// The stored key is the receiver of `__eq__`, and the same instance
    /// matches without calling it, as in CPython.
    fn key_probe_body(
        &mut self,
        class_ty: TypeId,
        locals: &mut Vec<hir::Local>,
        span: Span,
    ) -> Vec<hir::Stmt> {
        let table_ty = locals[0].ty;
        let (table, key, hash) = (hir::LocalId(0), hir::LocalId(1), hir::LocalId(2));
        let pos = push_local(locals, "pos", TypeId::INT, span);
        let stored = push_local(locals, "stored", class_ty, span);
        let probe = |after: hir::Expr| {
            expr(
                hir::ExprKind::TableProbe {
                    table: Box::new(local(table, table_ty, span)),
                    hash: Box::new(local(hash, TypeId::INT, span)),
                    after: Box::new(after),
                },
                TypeId::INT,
                span,
            )
        };
        let key_at = expr(
            hir::ExprKind::TableKey {
                table: Box::new(local(table, table_ty, span)),
                pos: Box::new(local(pos, TypeId::INT, span)),
            },
            class_ty,
            span,
        );
        let same = expr(
            hir::ExprKind::RefEq {
                lhs: Box::new(local(stored, class_ty, span)),
                rhs: Box::new(local(key, class_ty, span)),
                negated: false,
            },
            TypeId::BOOL,
            span,
        );
        let equal = self.eq_values(
            local(stored, class_ty, span),
            local(key, class_ty, span),
            span,
        );
        let matches = or(same, equal);
        let found = int_cmp(hir::CmpOp::Ge, local(pos, TypeId::INT, span), int(0, span));
        vec![
            assign(pos, probe(int(-1, span))),
            hir::Stmt::While {
                cond: found,
                body: hir::Block {
                    stmts: vec![
                        assign(stored, key_at),
                        when(matches, vec![ret(local(pos, TypeId::INT, span))]),
                        assign(pos, probe(local(pos, TypeId::INT, span))),
                    ],
                },
            },
            ret(int(-1, span)),
        ]
    }

    /// `sorted(xs)` over the `Ord` element type `elem` (DESIGN §3.11): a
    /// bottom-up merge sort between two copies of `xs`, which is stable
    /// because a run only yields to the next one when that element is
    /// strictly smaller — the only comparison is `__lt__`, as in Python.
    ///
    /// ```text
    /// n = len(xs); a = xs + []; b = xs + []; width = 1
    /// while width < n:
    ///     i = 0
    ///     while i < n:
    ///         mid = min(i + width, n); hi = min(mid + width, n)
    ///         l = i; r = mid; k = i
    ///         while k < hi:
    ///             if l < mid and (r >= hi or not (a[r] < a[l])):
    ///                 b[k] = a[l]; l = l + 1
    ///             else:
    ///                 b[k] = a[r]; r = r + 1
    ///             k = k + 1
    ///         i = hi
    ///     a, b = b, a; width = width + width
    /// return a
    /// ```
    fn sort_body(
        &mut self,
        elem: TypeId,
        locals: &mut Vec<hir::Local>,
        span: Span,
    ) -> Vec<hir::Stmt> {
        use hir::CmpOp::{Ge, Lt};
        let list_ty = locals[0].ty;
        let xs = hir::LocalId(0);
        let mut int_local = |name: &str| push_local(locals, name, TypeId::INT, span);
        let (n, width, i, mid, hi, l, r, k) = (
            int_local("n"),
            int_local("width"),
            int_local("i"),
            int_local("mid"),
            int_local("hi"),
            int_local("l"),
            int_local("r"),
            int_local("k"),
        );
        let a = push_local(locals, "a", list_ty, span);
        let b = push_local(locals, "b", list_ty, span);
        let tmp = push_local(locals, "tmp", list_ty, span);

        let iv = |id: hir::LocalId| local(id, TypeId::INT, span);
        let lv = |id: hir::LocalId| local(id, list_ty, span);
        let plus = |x: hir::Expr, y: hir::Expr| {
            expr(
                hir::ExprKind::IntBin {
                    op: hir::IntOp::Add,
                    lhs: Box::new(x),
                    rhs: Box::new(y),
                },
                TypeId::INT,
                span,
            )
        };
        let min = |x: hir::Expr, y: hir::Expr| {
            expr(
                hir::ExprKind::IntMinMax {
                    op: hir::MinMax::Min,
                    lhs: Box::new(x),
                    rhs: Box::new(y),
                },
                TypeId::INT,
                span,
            )
        };
        let get = |list: hir::LocalId, index: hir::LocalId| {
            expr(
                hir::ExprKind::ListGet {
                    list: Box::new(lv(list)),
                    index: Box::new(iv(index)),
                },
                elem,
                span,
            )
        };
        let copy = || {
            let empty = expr(
                hir::ExprKind::ListNew {
                    elem,
                    items: Vec::new(),
                },
                list_ty,
                span,
            );
            expr(
                hir::ExprKind::ListConcat {
                    lhs: Box::new(lv(xs)),
                    rhs: Box::new(empty),
                },
                list_ty,
                span,
            )
        };
        let put = |from: hir::LocalId| hir::Stmt::SetIndex {
            list: lv(b),
            index: iv(k),
            value: get(a, from),
        };
        let bump = |id: hir::LocalId| assign(id, plus(iv(id), int(1, span)));

        let right_first = self.lt_values(get(a, r), get(a, l), span);
        let take_left = and(
            int_cmp(Lt, iv(l), iv(mid)),
            or(int_cmp(Ge, iv(r), iv(hi)), not(right_first)),
        );
        let merge = hir::Stmt::While {
            cond: int_cmp(Lt, iv(k), iv(hi)),
            body: hir::Block {
                stmts: vec![
                    hir::Stmt::If {
                        cond: take_left,
                        then: hir::Block {
                            stmts: vec![put(l), bump(l)],
                        },
                        else_: Some(hir::Block {
                            stmts: vec![put(r), bump(r)],
                        }),
                    },
                    bump(k),
                ],
            },
        };
        let pass = hir::Stmt::While {
            cond: int_cmp(Lt, iv(i), iv(n)),
            body: hir::Block {
                stmts: vec![
                    assign(mid, min(plus(iv(i), iv(width)), iv(n))),
                    assign(hi, min(plus(iv(mid), iv(width)), iv(n))),
                    assign(l, iv(i)),
                    assign(r, iv(mid)),
                    assign(k, iv(i)),
                    merge,
                    assign(i, iv(hi)),
                ],
            },
        };
        vec![
            assign(
                n,
                expr(hir::ExprKind::ListLen(Box::new(lv(xs))), TypeId::INT, span),
            ),
            assign(a, copy()),
            assign(b, copy()),
            assign(width, int(1, span)),
            hir::Stmt::While {
                cond: int_cmp(Lt, iv(width), iv(n)),
                body: hir::Block {
                    stmts: vec![
                        assign(i, int(0, span)),
                        pass,
                        assign(tmp, lv(a)),
                        assign(a, lv(b)),
                        assign(b, lv(tmp)),
                        assign(width, plus(iv(width), iv(width))),
                    ],
                },
            },
            ret(lv(a)),
        ]
    }

    // -- class keys -------------------------------------------------------------

    /// Registers, for every class used as a dict key or a set element, the
    /// `__hash__` and the probe function codegen calls for each table
    /// operation (see [`hir::Protocols::key_probe`]), and its `__repr__` when
    /// it has one, for the `key not found` panic.
    ///
    /// Run once every body is checked, over every interned table type: the
    /// definition-time checks have been rolled back by then, so only real
    /// key classes are left.
    pub(crate) fn register_class_keys(&mut self, span: Span) {
        let classes: Vec<ClassId> = (0..self.types.len() as u32)
            .filter_map(|index| match self.types.get(TypeId(index)) {
                Type::Dict(key, _) | Type::Set(key) => match self.types.get(key) {
                    Type::Class(class) => Some(class),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        for class in classes {
            if self.traits.key_probes.contains_key(&class) || self.is_opaque(class) {
                continue;
            }
            let hash = self.proto_method(class, Builtin::Hash, span);
            let eq = self.proto_method(class, Builtin::Eq, span);
            let (Some(hash), Some(_)) = (hash, eq) else {
                // Reported where the table type was written.
                continue;
            };
            self.traits.hash_fns.insert(class, hash);
            // The `key not found` panic shows the key through its `__repr__`:
            // a written one — on a generic class, instantiated now if nothing
            // called it yet — or a derived one whose fields allow it.
            let class_ty = self.types.class_ty(class);
            let repr = Builtin::Repr.method();
            let written = self.methods.contains_key(&(class, repr.to_string()))
                || self
                    .generics
                    .origins
                    .get(&class)
                    .and_then(|(template, _)| self.generics.classes.get(template))
                    .is_some_and(|t| t.methods.iter().any(|m| m.name.as_str() == repr));
            if (written || self.satisfies(class_ty, Builtin::Repr.id()))
                && self.class_method(class, repr, span).is_some()
            {
                self.ensure_repr(class_ty, span);
            }
            let probe = self.synth_fn(SynthKind::KeyProbe, class_ty, span);
            self.traits.key_probes.insert(class, probe);
        }
    }

    /// The body of a derived method of the class `class_ty`.
    fn derived_body(&mut self, builtin: Builtin, class_ty: TypeId, span: Span) -> Vec<hir::Stmt> {
        let Type::Class(class) = self.types.get(class_ty) else {
            return vec![ret(boolean(false, span))];
        };
        let fields = self.types.class(class).fields.clone();
        let this = hir::LocalId(0);
        let other = hir::LocalId(1);
        let field = |obj: hir::LocalId, index: usize, ty: TypeId| {
            expr(
                hir::ExprKind::GetField {
                    obj: Box::new(local(obj, class_ty, span)),
                    class,
                    field: index as u32,
                },
                ty,
                span,
            )
        };
        match builtin {
            Builtin::Eq => {
                let mut acc: Option<hir::Expr> = None;
                for (i, f) in fields.iter().enumerate() {
                    self.ensure_eq(f.ty, span);
                    let eq = self.eq_values(field(this, i, f.ty), field(other, i, f.ty), span);
                    acc = Some(match acc {
                        Some(prev) => and(prev, eq),
                        None => eq,
                    });
                }
                vec![ret(acc.unwrap_or_else(|| boolean(true, span)))]
            }
            Builtin::Ord => {
                let pairs = fields
                    .iter()
                    .enumerate()
                    .map(|(i, f)| {
                        self.ensure_ord(f.ty, span);
                        (field(this, i, f.ty), field(other, i, f.ty))
                    })
                    .collect();
                self.lexicographic(hir::CmpOp::Lt, pairs, span)
            }
            Builtin::Hash => {
                let mut acc = int(HASH_SEED, span);
                for (i, f) in fields.iter().enumerate() {
                    self.ensure_hash(f.ty, span);
                    let value = self.hash_value(field(this, i, f.ty), span);
                    acc = mix(acc, value);
                }
                vec![ret(acc)]
            }
            _ => {
                // `Point(x=1, y=2)`, named after the template for an instance
                // of a generic class, like Python's dataclasses.
                let name = match self.generics.origins.get(&class) {
                    Some((template, _)) => template.clone(),
                    None => self.types.class(class).name.clone(),
                };
                let mut parts = Vec::with_capacity(fields.len() * 2 + 1);
                let mut text = format!("{name}(");
                for (i, f) in fields.iter().enumerate() {
                    if i > 0 {
                        text.push_str(", ");
                    }
                    text.push_str(&f.name);
                    text.push('=');
                    parts.push(hir::FStringPart::Literal(std::mem::take(&mut text)));
                    self.ensure_repr(f.ty, span);
                    let value = self.repr_value(field(this, i, f.ty), span);
                    parts.push(hir::FStringPart::Value {
                        expr: value,
                        spec: hir::FormatSpec::Display,
                    });
                }
                text.push(')');
                parts.push(hir::FStringPart::Literal(text));
                vec![ret(expr(hir::ExprKind::FString(parts), TypeId::STR, span))]
            }
        }
    }
}

/// Member `index` of the tuple local `tuple`.
fn member(
    tuple: hir::LocalId,
    ty: TypeId,
    index: usize,
    member_ty: TypeId,
    span: Span,
) -> hir::Expr {
    expr(
        hir::ExprKind::TupleGet {
            tuple: Box::new(local(tuple, ty, span)),
            index: index as u32,
        },
        member_ty,
        span,
    )
}
