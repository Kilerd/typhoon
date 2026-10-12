//! Checking of the M3 containers: `dict<K, V>` and `set<T>` (DESIGN §4.1),
//! `sorted(...)` (DESIGN §4.6) and `str` slicing.
//!
//! The hooks live where the M2 containers are checked (`exprs.rs` for
//! expressions, `check.rs` for statements); the rules live here.
//!
//! * A key (or a set element) is an `int`, a `bool`, a `str` or a flat tuple
//!   of those. A class key would need an eq/hash protocol, which is not
//!   decided yet (DESIGN §9), and a `float` key Python's `-0.0 == 0.0` /
//!   NaN rules; both are rejected where the type is written or inferred.
//! * `{}` is an empty dict and `set()` an empty set; both need an expected
//!   type, exactly like `[]` (DESIGN §3.7, §4.4).
//! * `d[k]` panics on a missing key, `d.get(k, default)` does not; there is no
//!   one-argument `get`, which would return `V | None`.
//! * `for k in d`, `for x in s` and `sorted(d)` walk a snapshot of the keys in
//!   insertion order, so mutating the table inside the loop is well defined.
//! * `s[a:b]` slices a `str` in code points — the unit of `len(s)` and
//!   `s.find(...)` — with Python's negative offsets and clamping.

use typhoon_ast as ast;
use typhoon_diag::{Diagnostic, Span};

use crate::check::{Checker, a_type};
use crate::hir;
use crate::types::{Type, TypeId};

impl Checker<'_> {
    // -- types ---------------------------------------------------------------

    /// Resolves `dict<K, V>` and `set<T>` written in a type position.
    pub(crate) fn resolve_hash_type(&mut self, name: &str, args: &[TypeId], span: Span) -> TypeId {
        let wanted = if name == "dict" { 2 } else { 1 };
        if args.len() != wanted {
            let example = if name == "dict" {
                "exactly two type arguments, e.g. `dict<str, int>`"
            } else {
                "exactly one type argument, e.g. `set<int>`"
            };
            self.error(
                Diagnostic::error(format!("`{name}` takes {example}"))
                    .with_label(span, "wrong number of type arguments"),
            );
            return TypeId::ERROR;
        }
        if args.contains(&TypeId::ERROR) {
            return TypeId::ERROR;
        }
        if !self.require_key_type(args[0], span, name) {
            return TypeId::ERROR;
        }
        if name == "set" {
            return self.types.set_of(args[0]);
        }
        if args[1] == TypeId::UNIT {
            self.error(
                Diagnostic::error("a dict value cannot have type `None`")
                    .with_label(span, "`None` is not a value type"),
            );
            return TypeId::ERROR;
        }
        self.types.dict_of(args[0], args[1])
    }

    /// Reports `ty` when it cannot be a dict key or a set element.
    pub(crate) fn require_key_type(&mut self, ty: TypeId, span: Span, container: &str) -> bool {
        if ty == TypeId::ERROR || self.types.is_hashable(ty) {
            return true;
        }
        let name = self.types.name(ty);
        let what = if container == "set" {
            "a set element"
        } else {
            "a dict key"
        };
        let mut diag = Diagnostic::error(format!("`{name}` cannot be {what}"))
            .with_label(span, format!("`{name}` is not hashable"))
            .with_help("keys must be `int`, `bool`, `str`, or a tuple of those");
        diag = match self.types.get(ty) {
            Type::Class(_) | Type::Optional(_) => diag.with_note(
                "a class key needs an eq/hash protocol, which is not decided yet, \
                 see DESIGN.md section 9",
            ),
            Type::Float => diag.with_note("`float` keys are not supported; use an `int` key"),
            _ => diag,
        };
        self.error(diag);
        false
    }

    // -- literals --------------------------------------------------------------

    /// `{k: v, ...}` and `{}`: the types come from the annotation when there
    /// is one, otherwise from the first entry (DESIGN §4.4).
    pub(crate) fn check_dict_literal(
        &mut self,
        entries: &[(ast::Expr, ast::Expr)],
        expected: Option<TypeId>,
        span: Span,
    ) -> hir::Expr {
        let hint = expected.and_then(|ty| self.types.as_dict(ty));
        if entries.is_empty() {
            if let Some((key, value)) = hint {
                let ty = self.types.dict_of(key, value);
                return hir::Expr {
                    kind: hir::ExprKind::DictNew {
                        key,
                        value,
                        entries: Vec::new(),
                    },
                    ty,
                    span,
                };
            }
            if expected.is_some_and(|ty| self.types.as_set(ty).is_some()) {
                self.error(
                    Diagnostic::error("`{}` is an empty dict, not an empty set")
                        .with_label(span, "this is a `dict` literal")
                        .with_help("write `set()` for an empty set")
                        .with_note("see DESIGN.md section 3.7"),
                );
                return self.error_expr(span);
            }
            if expected == Some(TypeId::ERROR) {
                return self.error_expr(span);
            }
            self.error(
                Diagnostic::error("cannot infer the key and value types of an empty dict")
                    .with_label(span, "this literal has no entries to infer from")
                    .with_help("annotate the variable, e.g. `d: dict<str, int> = {}`")
                    .with_note(
                        "inference is local, so an empty container needs an annotation, \
                         see DESIGN.md section 4.4",
                    ),
            );
            return self.error_expr(span);
        }

        let (mut key_ty, mut val_ty) = match hint {
            Some((k, v)) => (Some(k), Some(v)),
            None => (None, None),
        };
        let mut checked = Vec::with_capacity(entries.len());
        for (k, v) in entries {
            let key = self.check_expr_hint(k, key_ty);
            if key_ty.is_none() && key.ty != TypeId::ERROR {
                key_ty = Some(key.ty);
            }
            let value = self.check_expr_hint(v, val_ty);
            if val_ty.is_none() && value.ty != TypeId::ERROR {
                val_ty = Some(value.ty);
            }
            checked.push((key, value));
        }
        let (Some(key_ty), Some(val_ty)) = (key_ty, val_ty) else {
            return self.error_expr(span);
        };
        if val_ty == TypeId::UNIT {
            self.error(
                Diagnostic::error("a dict value cannot have type `None`")
                    .with_label(span, "these expressions produce no value"),
            );
            return self.error_expr(span);
        }
        if !self.require_key_type(key_ty, checked[0].0.span, "dict") {
            return self.error_expr(span);
        }
        let mut ok = true;
        let mut lowered = Vec::with_capacity(checked.len());
        for (key, value) in checked {
            let key = self.literal_member(key, key_ty, "key of a dict literal");
            let value = self.literal_member(value, val_ty, "value of a dict literal");
            match (key, value) {
                (Some(key), Some(value)) => lowered.push((key, value)),
                _ => ok = false,
            }
        }
        if !ok {
            return self.error_expr(span);
        }
        let ty = self.types.dict_of(key_ty, val_ty);
        hir::Expr {
            kind: hir::ExprKind::DictNew {
                key: key_ty,
                value: val_ty,
                entries: lowered,
            },
            ty,
            span,
        }
    }

    /// `{a, b}`: the element type comes from the annotation or the first
    /// element.
    pub(crate) fn check_set_literal(
        &mut self,
        items: &[ast::Expr],
        expected: Option<TypeId>,
        span: Span,
    ) -> hir::Expr {
        let mut elem = expected.and_then(|ty| self.types.as_set(ty));
        let mut checked = Vec::with_capacity(items.len());
        for item in items {
            let value = self.check_expr_hint(item, elem);
            if elem.is_none() && value.ty != TypeId::ERROR {
                elem = Some(value.ty);
            }
            checked.push(value);
        }
        let Some(elem) = elem else {
            return self.error_expr(span);
        };
        if elem == TypeId::UNIT {
            self.error(
                Diagnostic::error("a set element cannot have type `None`")
                    .with_label(span, "these expressions produce no value"),
            );
            return self.error_expr(span);
        }
        let first = checked.first().map_or(span, |c| c.span);
        if !self.require_key_type(elem, first, "set") {
            return self.error_expr(span);
        }
        let mut ok = true;
        let mut lowered = Vec::with_capacity(checked.len());
        for value in checked {
            match self.literal_member(value, elem, "element of a set literal") {
                Some(value) => lowered.push(value),
                None => ok = false,
            }
        }
        if !ok {
            return self.error_expr(span);
        }
        let ty = self.types.set_of(elem);
        hir::Expr {
            kind: hir::ExprKind::SetNew {
                elem,
                items: lowered,
            },
            ty,
            span,
        }
    }

    /// Checks one member of a container literal against the literal's type.
    fn literal_member(&mut self, value: hir::Expr, want: TypeId, what: &str) -> Option<hir::Expr> {
        if value.ty == TypeId::ERROR {
            return None;
        }
        if !self.assignable(want, value.ty) {
            let (e, f) = (self.types.name(want), self.types.name(value.ty));
            self.error(
                Diagnostic::error(format!("mismatched types: expected `{e}`, found `{f}`"))
                    .with_label(value.span, format!("expected `{e}`, found `{f}`"))
                    .with_note(format!("every {what} must have the same type")),
            );
            return None;
        }
        Some(self.coerce(value, want))
    }

    /// Whether `expr` is a call of the builtin `set` or `dict` (not shadowed
    /// by a variable, a function or a class of that name).
    pub(crate) fn empty_ctor_name<'e>(&self, expr: &'e ast::Expr) -> Option<&'e str> {
        let ast::ExprKind::Call {
            callee,
            type_args: None,
            ..
        } = &expr.kind
        else {
            return None;
        };
        let ast::ExprKind::Name(name) = &callee.kind else {
            return None;
        };
        let name = name.as_str();
        let builtin = matches!(name, "set" | "dict")
            && !self.scope.contains_key(name)
            && !self.fn_index.contains_key(name)
            && !self.classes.contains_key(name);
        builtin.then_some(name)
    }

    /// `set()` and `dict()`: empty containers whose type comes from the
    /// annotation, exactly like `{}`.
    pub(crate) fn check_empty_ctor(
        &mut self,
        name: &str,
        args: &[ast::Arg],
        expected: Option<TypeId>,
        span: Span,
    ) -> hir::Expr {
        if !args.is_empty() {
            for arg in args {
                self.check_expr(&arg.value);
            }
            self.error(
                Diagnostic::error(format!("`{name}(...)` with arguments is not supported yet"))
                    .with_label(span, format!("`{name}()` only builds an empty {name}"))
                    .with_help(if name == "set" {
                        "start from `set()` and `add` the elements, or write a literal `{a, b}`"
                    } else {
                        "write a literal `{k: v}`, or start from `{}` and assign keys"
                    }),
            );
            return self.error_expr(span);
        }
        if name == "dict" {
            return self.check_dict_literal(&[], expected, span);
        }
        match expected.and_then(|ty| self.types.as_set(ty)) {
            Some(elem) => {
                let ty = self.types.set_of(elem);
                hir::Expr {
                    kind: hir::ExprKind::SetNew {
                        elem,
                        items: Vec::new(),
                    },
                    ty,
                    span,
                }
            }
            None if expected == Some(TypeId::ERROR) => self.error_expr(span),
            None => {
                self.error(
                    Diagnostic::error("cannot infer the element type of an empty set")
                        .with_label(span, "`set()` has no elements to infer from")
                        .with_help("annotate the variable, e.g. `s: set<int> = set()`")
                        .with_note(
                            "inference is local, so an empty container needs an annotation, \
                             see DESIGN.md section 4.4",
                        ),
                );
                self.error_expr(span)
            }
        }
    }

    // -- keys ----------------------------------------------------------------

    /// Checks a key expression against the key type `want`.
    fn check_key(&mut self, key: &ast::Expr, want: TypeId) -> Option<hir::Expr> {
        let value = self.check_expr_hint(key, Some(want));
        if value.ty == TypeId::ERROR {
            return None;
        }
        if !self.assignable(want, value.ty) {
            self.mismatch(value.span, want, value.ty);
            return None;
        }
        Some(self.coerce(value, want))
    }

    /// `d[k]`.
    pub(crate) fn check_dict_index(
        &mut self,
        dict: hir::Expr,
        key_ty: TypeId,
        val_ty: TypeId,
        index: &ast::Expr,
        span: Span,
    ) -> hir::Expr {
        let Some(key) = self.check_key(index, key_ty) else {
            return self.error_expr(span);
        };
        hir::Expr {
            kind: hir::ExprKind::DictGet {
                dict: Box::new(dict),
                key: Box::new(key),
            },
            ty: val_ty,
            span,
        }
    }

    /// `d[k] = value` with the value not yet checked.
    pub(crate) fn check_dict_assign(
        &mut self,
        dict: hir::Expr,
        index: &ast::Expr,
        value: &ast::Expr,
    ) -> Option<hir::Stmt> {
        let (key_ty, val_ty) = self.types.as_dict(dict.ty)?;
        let key = self.check_key(index, key_ty);
        let value = self.check_expr_hint(value, Some(val_ty));
        self.dict_store(dict, key?, value)
    }

    /// `d[k] = value` with every part already checked (also the unpacking
    /// form `a, d[k] = t`).
    pub(crate) fn dict_store(
        &mut self,
        dict: hir::Expr,
        key: hir::Expr,
        value: hir::Expr,
    ) -> Option<hir::Stmt> {
        let (_, val_ty) = self.types.as_dict(dict.ty)?;
        if !self.assignable(val_ty, value.ty) {
            self.mismatch(value.span, val_ty, value.ty);
            return None;
        }
        let value = self.coerce(value, val_ty);
        Some(hir::Stmt::DictSet { dict, key, value })
    }

    /// The key of `d[k] = ...` in the unpacking form, checked against `d`.
    pub(crate) fn check_dict_key_of(
        &mut self,
        dict: &hir::Expr,
        index: &ast::Expr,
    ) -> Option<hir::Expr> {
        let (key_ty, _) = self.types.as_dict(dict.ty)?;
        self.check_key(index, key_ty)
    }

    /// `k in d` / `x in s`.
    pub(crate) fn check_hash_in(
        &mut self,
        table: hir::Expr,
        key_ty: TypeId,
        left: &ast::Expr,
        negated: bool,
        span: Span,
    ) -> hir::Expr {
        let Some(key) = self.check_key(left, key_ty) else {
            return self.error_expr(span);
        };
        hir::Expr {
            kind: hir::ExprKind::HashContains {
                table: Box::new(table),
                key: Box::new(key),
                negated,
            },
            ty: TypeId::BOOL,
            span,
        }
    }

    /// A dict or a set as the `list<K>` snapshot of its keys, which is what a
    /// `for` loop and `sorted` walk; any other value is returned unchanged.
    pub(crate) fn keys_snapshot(&mut self, value: hir::Expr) -> hir::Expr {
        let key = match self.types.get(value.ty) {
            Type::Dict(key, _) | Type::Set(key) => key,
            _ => return value,
        };
        let ty = self.types.list_of(key);
        let span = value.span;
        hir::Expr {
            kind: hir::ExprKind::HashKeys(Box::new(value)),
            ty,
            span,
        }
    }

    // -- methods ---------------------------------------------------------------

    /// The methods of `dict<K, V>` (DESIGN §4.6).
    pub(crate) fn check_dict_method(
        &mut self,
        recv: hir::Expr,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let (key_ty, val_ty) = self.types.as_dict(recv.ty).expect("a dict receiver");
        let dict_name = self.types.name(recv.ty);
        let method = name.as_str();
        let (kind, ty) = match method {
            "get" => {
                if args.len() == 1 && args[0].name.is_none() {
                    self.check_expr(&args[0].value);
                    self.error(
                        Diagnostic::error("`dict.get` needs a default value")
                            .with_label(span, "write `d.get(key, default)`")
                            .with_help("use `d[key]` when the key must be present")
                            .with_note(
                                "a one-argument `get` would return `V | None`, which is not \
                                 supported for a dict value yet",
                            ),
                    );
                    return self.error_expr(span);
                }
                let Some(mut checked) =
                    self.builtin_args(&dict_name, "get", args, &[key_ty, val_ty], span)
                else {
                    return self.error_expr(span);
                };
                let default = checked.remove(1);
                let key = checked.remove(0);
                (
                    hir::ExprKind::DictGetOr {
                        dict: Box::new(recv),
                        key: Box::new(key),
                        default: Box::new(default),
                    },
                    val_ty,
                )
            }
            "pop" => {
                let wanted: &[TypeId] = if args.len() == 2 {
                    &[key_ty, val_ty]
                } else {
                    &[key_ty]
                };
                let Some(mut checked) = self.builtin_args(&dict_name, "pop", args, wanted, span)
                else {
                    return self.error_expr(span);
                };
                let default = (checked.len() == 2).then(|| Box::new(checked.remove(1)));
                let key = checked.remove(0);
                (
                    hir::ExprKind::DictPop {
                        dict: Box::new(recv),
                        key: Box::new(key),
                        default,
                    },
                    val_ty,
                )
            }
            "keys" | "values" | "items" => {
                if self
                    .builtin_args(&dict_name, method, args, &[], span)
                    .is_none()
                {
                    return self.error_expr(span);
                }
                match method {
                    "keys" => {
                        let ty = self.types.list_of(key_ty);
                        (hir::ExprKind::HashKeys(Box::new(recv)), ty)
                    }
                    "values" => {
                        let ty = self.types.list_of(val_ty);
                        (hir::ExprKind::DictValues(Box::new(recv)), ty)
                    }
                    _ => {
                        let pair = self.types.tuple_of(&[key_ty, val_ty]);
                        let ty = self.types.list_of(pair);
                        (hir::ExprKind::DictItems(Box::new(recv)), ty)
                    }
                }
            }
            other => {
                for arg in args {
                    self.check_expr(&arg.value);
                }
                self.error(
                    Diagnostic::error(format!("`{dict_name}` has no method `{other}`"))
                        .with_label(name.span, "unknown method")
                        .with_help("`dict` has `get`, `pop`, `keys`, `values` and `items`")
                        .with_note("see DESIGN.md section 4.6"),
                );
                return self.error_expr(span);
            }
        };
        hir::Expr { kind, ty, span }
    }

    /// The methods of `set<T>` (DESIGN §4.6).
    pub(crate) fn check_set_method(
        &mut self,
        recv: hir::Expr,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let elem = self.types.as_set(recv.ty).expect("a set receiver");
        let set_name = self.types.name(recv.ty);
        let method = name.as_str();
        if !matches!(method, "add" | "remove" | "discard") {
            for arg in args {
                self.check_expr(&arg.value);
            }
            self.error(
                Diagnostic::error(format!("`{set_name}` has no method `{method}`"))
                    .with_label(name.span, "unknown method")
                    .with_help("`set` has `add`, `remove` and `discard`")
                    .with_note("see DESIGN.md section 4.6"),
            );
            return self.error_expr(span);
        }
        let Some(mut checked) = self.builtin_args(&set_name, method, args, &[elem], span) else {
            return self.error_expr(span);
        };
        let value = Box::new(checked.remove(0));
        let set = Box::new(recv);
        let kind = match method {
            "add" => hir::ExprKind::SetAdd { set, value },
            _ => hir::ExprKind::SetRemove {
                set,
                value,
                strict: method == "remove",
            },
        };
        hir::Expr {
            kind,
            ty: TypeId::UNIT,
            span,
        }
    }

    // -- builtins ----------------------------------------------------------------

    /// `sorted(xs)`: a sorted copy of a `list<T>`, or the sorted keys of a
    /// dict or a set (DESIGN §4.6).
    pub(crate) fn check_sorted(&mut self, args: &[ast::Arg], span: Span) -> hir::Expr {
        if args.len() != 1 {
            for arg in args {
                self.check_expr(&arg.value);
            }
            self.error(
                Diagnostic::error(format!(
                    "`sorted` takes 1 argument but {} were supplied",
                    args.len()
                ))
                .with_label(span, "wrong number of arguments"),
            );
            return self.error_expr(span);
        }
        let value = self.check_expr(&args[0].value);
        if value.ty == TypeId::ERROR {
            return self.error_expr(span);
        }
        let value = self.keys_snapshot(value);
        let Some(elem) = self.types.as_list(value.ty) else {
            let found = self.types.name(value.ty);
            self.error(
                Diagnostic::error(format!("`sorted` is not defined for `{found}`"))
                    .with_label(value.span, format!("this is {}", a_type(&found)))
                    .with_help("`sorted` takes a `list<T>`, a `set<T>` or a `dict<K, V>`"),
            );
            return self.error_expr(span);
        };
        if !self.types.is_sortable(elem) {
            let name = self.types.name(elem);
            self.error(
                Diagnostic::error(format!("`sorted` cannot order `{name}` values"))
                    .with_label(value.span, format!("the elements are `{name}`"))
                    .with_help(
                        "`sorted` orders `int`, `float`, `bool`, `str` and flat tuples of those",
                    ),
            );
            return self.error_expr(span);
        }
        let ty = value.ty;
        hir::Expr {
            kind: hir::ExprKind::Sorted(Box::new(value)),
            ty,
            span,
        }
    }

    /// `s[a:b]` on a `str`, in code points (see the module docs).
    pub(crate) fn check_slice(
        &mut self,
        base: &ast::Expr,
        start: Option<&ast::Expr>,
        stop: Option<&ast::Expr>,
        span: Span,
    ) -> hir::Expr {
        let value = self.check_expr(base);
        let bound = |checker: &mut Self, expr: Option<&ast::Expr>, default: i64| match expr {
            Some(expr) => {
                let checked = checker.check_expr(expr);
                if checked.ty != TypeId::ERROR && checked.ty != TypeId::INT {
                    let found = checker.types.name(checked.ty);
                    checker.error(
                        Diagnostic::error(format!(
                            "a slice bound must be an `int`, found `{found}`"
                        ))
                        .with_label(checked.span, format!("this is {}", a_type(&found))),
                    );
                    return checker.error_expr(checked.span);
                }
                checked
            }
            None => hir::Expr {
                kind: hir::ExprKind::Int(default),
                ty: TypeId::INT,
                span,
            },
        };
        let start = bound(self, start, 0);
        let stop = bound(self, stop, i64::MAX);
        if value.ty == TypeId::ERROR || start.ty == TypeId::ERROR || stop.ty == TypeId::ERROR {
            return self.error_expr(span);
        }
        if value.ty != TypeId::STR {
            let found = self.types.name(value.ty);
            self.error(
                Diagnostic::error(format!("`{found}` cannot be sliced"))
                    .with_label(base.span, format!("this is {}", a_type(&found)))
                    .with_help("only a `str` supports `[start:stop]` slicing"),
            );
            return self.error_expr(span);
        }
        hir::Expr {
            kind: hir::ExprKind::StrSlice {
                string: Box::new(value),
                start: Box::new(start),
                stop: Box::new(stop),
            },
            ty: TypeId::STR,
            span,
        }
    }
}
