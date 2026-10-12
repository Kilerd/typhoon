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
//!   `min` / `max` over an `Ord` type) are written directly as HIR: their
//!   signature is registered when they are first needed, and the body is
//!   built by [`Checker::build_synth`] when the instance queue is drained —
//!   so the codegens never learn that traits exist.

use typhoon_diag::Span;

use crate::check::{Checker, FnSig, ParamSig};
use crate::generics::symbol_safe;
use crate::hir;
use crate::traits::{Builtin, SynthJob, SynthKind};
use crate::types::{ClassId, Type, TypeId};

/// FNV-1a's 64-bit offset basis and prime, the field / member hash mixer.
const HASH_SEED: i64 = 0xcbf2_9ce4_8422_2325_u64 as i64;
const HASH_PRIME: i64 = 0x0000_0100_0000_01b3;
/// The hash of `None` in a `T | None`.
const HASH_NONE: i64 = 0x9e37_79b9_7f4a_7c15_u64 as i64;

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

    /// Whether `hash(x)` is defined on `ty`.
    pub(crate) fn ensure_hash(&mut self, ty: TypeId, at: Span) -> bool {
        match self.types.get(ty) {
            Type::Error | Type::Int | Type::Bool | Type::Str => true,
            Type::Optional(inner) => self.ensure_hash(inner, at),
            Type::Tuple(id) => {
                let members = self.types.tuple_members(id).to_vec();
                members.into_iter().all(|m| self.ensure_hash(m, at))
            }
            Type::Class(class) => self.proto_method(class, Builtin::Hash, at).is_some(),
            Type::Unit | Type::Float | Type::List(_) | Type::Dict(..) | Type::Set(_) => false,
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
        let op = hir::CmpOp::Lt;
        let ty = a.ty;
        let (lhs, rhs) = (Box::new(a), Box::new(b));
        let kind = match self.types.get(ty) {
            Type::Int => hir::ExprKind::IntCmp { op, lhs, rhs },
            Type::Float => hir::ExprKind::FloatCmp { op, lhs, rhs },
            Type::Bool => hir::ExprKind::BoolCmp { op, lhs, rhs },
            Type::Str => hir::ExprKind::StrCmp { op, lhs, rhs },
            Type::Class(class) => {
                return match self.proto_method(class, Builtin::Ord, span) {
                    Some(func) => self.call_fn(func, vec![*lhs, *rhs], vec![0, 1], span),
                    None => boolean(false, span),
                };
            }
            _ => {
                let func = self.synth_fn(SynthKind::TupleLt, ty, span);
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
            Type::Optional(_) => {
                let func = self.synth_fn(SynthKind::OptionalHash, value.ty, span);
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
            SynthKind::TupleLt => ("lt", vec![("a", ty), ("b", ty)], TypeId::BOOL),
            SynthKind::TupleHash | SynthKind::OptionalHash => {
                ("hash", vec![("x", ty)], TypeId::INT)
            }
            SynthKind::Min => ("min", vec![("a", ty), ("b", ty)], ty),
            SynthKind::Max => ("max", vec![("a", ty), ("b", ty)], ty),
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
        let locals: Vec<hir::Local> = sig
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
            SynthKind::TupleLt => {
                let (a, b) = (hir::LocalId(0), hir::LocalId(1));
                let members = self.types.as_tuple(ty).unwrap_or_default().to_vec();
                let pairs = members
                    .iter()
                    .enumerate()
                    .map(|(i, m)| (member(a, ty, i, *m, span), member(b, ty, i, *m, span)))
                    .collect();
                self.lexicographic(pairs, span)
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
            SynthKind::OptionalHash => {
                let x = local(hir::LocalId(0), ty, span);
                let inner = self.types.as_optional(ty).unwrap_or(TypeId::ERROR);
                let is_none = expr(
                    hir::ExprKind::IsNone {
                        value: Box::new(x.clone()),
                        negated: false,
                    },
                    TypeId::BOOL,
                    span,
                );
                let payload = if self.types.is_tagged_optional(ty) {
                    expr(hir::ExprKind::OptionalValue(Box::new(x)), inner, span)
                } else {
                    hir::Expr { ty: inner, ..x }
                };
                let hash = self.hash_value(payload, span);
                vec![when(is_none, vec![ret(int(HASH_NONE, span))]), ret(hash)]
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

    /// `for each pair: if a != b: return a < b`, then `return False`: the
    /// lexicographic `<` of tuples and of derived `Ord`.
    fn lexicographic(&mut self, pairs: Vec<(hir::Expr, hir::Expr)>, span: Span) -> Vec<hir::Stmt> {
        let mut stmts = Vec::with_capacity(pairs.len() + 1);
        for (a, b) in pairs {
            let differ = not(self.eq_values(a.clone(), b.clone(), span));
            let less = self.lt_values(a, b, span);
            stmts.push(when(differ, vec![ret(less)]));
        }
        stmts.push(ret(boolean(false, span)));
        stmts
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
                self.lexicographic(pairs, span)
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
