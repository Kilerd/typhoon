//! Nominal traits and trait bounds (DESIGN §3.11).
//!
//! A type implements a trait only when it says so: `class C(T1, T2):` lists
//! the traits of `C`, and the checker verifies that every required method is
//! there with the trait's signature (`Self` standing for `C`). Default
//! methods of user traits are copied into the class unless it overrides them
//! ([`expand_trait_defaults`]); the builtin traits `Eq` / `Ord` / `Hash` /
//! `Repr` are *derived* field by field when declared without the method (the
//! bodies are synthesized HIR, see `protocols.rs`).
//!
//! # Checked and template generics
//!
//! A generic item with at least one bounded type parameter is checked **once,
//! at its definition**, with every type parameter opaque: a parameter is a
//! fieldless pseudo-class whose methods are exactly the methods of its bounds
//! (an unbounded parameter has none), so `x.area()` on a `T: Shape`, `a < b`
//! on a `T: Ord` and `print(x)` on a `T: Repr` resolve exactly like the same
//! code on a class that declares those traits, and anything else is an error
//! at the definition. Everything that check creates — the pseudo-classes,
//! instances of other generics, signatures — is rolled back afterwards
//! ([`Checker::check_definitions`]), so the definition check never reaches
//! codegen. Call sites then only check that the type arguments implement the
//! bounds ([`Checker::check_type_bounds`]), and the item is monomorphized
//! like any other generic. Items without a single bound keep the template
//! semantics of DESIGN §3.9.

use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

use typhoon_ast as ast;
use typhoon_diag::{Diagnostic, Span};

use crate::check::{Checker, FnSig, ParamSig};
use crate::generics::{Frame, GenericsMark, bind};
use crate::hir;
use crate::types::{ClassId, Type, TypeId, TypeTable};

/// The builtin traits (DESIGN §3.11), in the order they are registered: the
/// [`TraitId`] of a builtin is its discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Builtin {
    /// `__eq__(self, other: Self) -> bool`: `==`, `!=`, `in`.
    Eq = 0,
    /// `__lt__(self, other: Self) -> bool`: `<` `<=` `>` `>=`, `min`, `max`.
    Ord,
    /// `__hash__(self) -> int`: `hash(x)`.
    Hash,
    /// `__repr__(self) -> str`: `print`, f-strings, `str`, `repr`.
    Repr,
    /// `__add__(self, other: Self) -> Self`: `+`.
    Add,
    /// `__sub__(self, other: Self) -> Self`: `-`.
    Sub,
    /// `__mul__(self, other: Self) -> Self`: `*`.
    Mul,
    /// `__truediv__(self, other: Self) -> Self`: `/`.
    Div,
    /// `__floordiv__(self, other: Self) -> Self`: `//`.
    FloorDiv,
    /// `__mod__(self, other: Self) -> Self`: `%`.
    Mod,
    /// `__neg__(self) -> Self`: unary `-`.
    Neg,
}

impl Builtin {
    /// Every builtin trait, in registration order.
    pub(crate) const ALL: [Builtin; 11] = [
        Builtin::Eq,
        Builtin::Ord,
        Builtin::Hash,
        Builtin::Repr,
        Builtin::Add,
        Builtin::Sub,
        Builtin::Mul,
        Builtin::Div,
        Builtin::FloorDiv,
        Builtin::Mod,
        Builtin::Neg,
    ];

    /// The trait's id in [`Traits`].
    pub(crate) fn id(self) -> TraitId {
        TraitId(self as usize)
    }

    /// The trait name as written in source.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Builtin::Eq => "Eq",
            Builtin::Ord => "Ord",
            Builtin::Hash => "Hash",
            Builtin::Repr => "Repr",
            Builtin::Add => "Add",
            Builtin::Sub => "Sub",
            Builtin::Mul => "Mul",
            Builtin::Div => "Div",
            Builtin::FloorDiv => "FloorDiv",
            Builtin::Mod => "Mod",
            Builtin::Neg => "Neg",
        }
    }

    /// The dunder method the trait consists of.
    pub(crate) fn method(self) -> &'static str {
        match self {
            Builtin::Eq => "__eq__",
            Builtin::Ord => "__lt__",
            Builtin::Hash => "__hash__",
            Builtin::Repr => "__repr__",
            Builtin::Add => "__add__",
            Builtin::Sub => "__sub__",
            Builtin::Mul => "__mul__",
            Builtin::Div => "__truediv__",
            Builtin::FloorDiv => "__floordiv__",
            Builtin::Mod => "__mod__",
            Builtin::Neg => "__neg__",
        }
    }

    /// The builtin trait whose method is `name`.
    pub(crate) fn from_method(name: &str) -> Option<Builtin> {
        Builtin::ALL.into_iter().find(|b| b.method() == name)
    }

    /// The trait behind an overloadable binary operator.
    pub(crate) fn of_binop(op: ast::BinOp) -> Option<Builtin> {
        Some(match op {
            ast::BinOp::Add => Builtin::Add,
            ast::BinOp::Sub => Builtin::Sub,
            ast::BinOp::Mul => Builtin::Mul,
            ast::BinOp::Div => Builtin::Div,
            ast::BinOp::FloorDiv => Builtin::FloorDiv,
            ast::BinOp::Mod => Builtin::Mod,
            _ => return None,
        })
    }

    /// Whether the method takes an `other` besides `self`.
    pub(crate) fn binary(self) -> bool {
        !matches!(self, Builtin::Hash | Builtin::Repr | Builtin::Neg)
    }

    /// The return type as written in the trait.
    fn ret(self) -> &'static str {
        match self {
            Builtin::Eq | Builtin::Ord => "bool",
            Builtin::Hash => "int",
            Builtin::Repr => "str",
            _ => "Self",
        }
    }

    /// Whether a class that declares the trait without the method gets a
    /// field-wise implementation.
    pub(crate) fn derivable(self) -> bool {
        matches!(
            self,
            Builtin::Eq | Builtin::Ord | Builtin::Hash | Builtin::Repr
        )
    }

    /// Whether declaring the trait also requires declaring `Eq`.
    fn requires_eq(self) -> bool {
        matches!(self, Builtin::Ord | Builtin::Hash)
    }

    /// The builtin trait with id `id`, if it is one.
    fn of_id(id: TraitId) -> Option<Builtin> {
        Builtin::ALL.get(id.0).copied()
    }
}

/// A handle to one trait in [`Traits`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TraitId(pub(crate) usize);

/// One declared trait.
pub(crate) struct TraitInfo {
    pub(crate) name: String,
    /// Where it is declared; a dummy span for a builtin trait.
    span: Span,
    builtin: Option<Builtin>,
    methods: Vec<TraitMethodInfo>,
    /// A method signature was rejected; the trait checks nothing.
    poisoned: bool,
}

/// One method of a trait.
struct TraitMethodInfo {
    decl: Rc<ast::FnDecl>,
    has_body: bool,
    /// The default body failed its definition-time check, so its copies in
    /// classes are not checked again.
    poisoned: bool,
}

/// A function the checker writes itself (DESIGN §3.11), built once its
/// signature is registered: a derived method or a helper behind an operator.
pub(crate) struct SynthJob {
    pub(crate) id: hir::FuncId,
    pub(crate) kind: SynthKind,
    /// The class for a derived method, the operand type for a helper.
    pub(crate) ty: TypeId,
    pub(crate) span: Span,
}

/// What a [`SynthJob`] builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SynthKind {
    /// The field-wise `__eq__` / `__lt__` / `__hash__` / `__repr__` of a class.
    Derive(Builtin),
    /// A lexicographic ordering (`<`, `<=`, `>` or `>=`) of two tuples, as
    /// Python compares them: the first members that differ decide.
    TupleOrder(hir::CmpOp),
    /// The hash of a flat tuple of `int`, `bool` and `str`.
    TupleHash,
    /// `min(a, b)` on an `Ord` type without a native minimum.
    Min,
    /// `max(a, b)` on an `Ord` type without a native maximum.
    Max,
    /// The probe function of a class used as a dict key or a set element
    /// (see [`hir::Protocols::key_probe`]); the operand type is the class.
    KeyProbe,
    /// `sorted(xs)` over an `Ord` element type the runtime cannot order
    /// itself: a stable merge sort calling `__lt__`; the operand type is the
    /// element type.
    Sort,
}

/// Everything the checker knows about traits.
#[derive(Default)]
pub(crate) struct Traits {
    list: Vec<TraitInfo>,
    by_name: HashMap<String, TraitId>,
    /// The traits each non-generic class declares, with the span of the name
    /// in its header.
    declared: HashMap<ClassId, Vec<(TraitId, Span)>>,
    /// The traits each generic class template declares, by template name.
    template_declared: HashMap<String, Vec<(TraitId, Span)>>,
    /// The pseudo-classes standing for type parameters during a
    /// definition-time check, with their bounds (closed under `Eq`).
    opaque: HashMap<ClassId, Vec<TraitId>>,
    /// The name span of every default method of a user trait, which is also
    /// the name span of its copies in classes.
    defaults_by_span: HashMap<Span, (TraitId, usize)>,
    /// The `__eq__` / `__repr__` codegen calls inside containers.
    pub(crate) eq_fns: HashMap<ClassId, hir::FuncId>,
    pub(crate) repr_fns: HashMap<ClassId, hir::FuncId>,
    /// The `__hash__` and the probe function of every class used as a key,
    /// filled in once checking is done ([`Checker::register_class_keys`]).
    pub(crate) hash_fns: HashMap<ClassId, hir::FuncId>,
    pub(crate) key_probes: HashMap<ClassId, hir::FuncId>,
    /// Synthesized helpers, memoized per kind and operand type.
    pub(crate) synth: HashMap<(SynthKind, TypeId), hir::FuncId>,
    /// Synthesized functions whose bodies are still to be built.
    pub(crate) pending: VecDeque<SynthJob>,
    /// Classes a trait could not be derived for; the error was reported.
    pub(crate) derive_failed: HashSet<(ClassId, Builtin)>,
    /// Generic class instances whose conformance is still to be checked, with
    /// the instantiation stack that created them.
    conformance: VecDeque<(ClassId, Vec<Frame>)>,
    /// The `(class, trait)` pairs [`Checker::satisfies`] is deciding, so that
    /// a self-referential class does not recurse forever.
    satisfying: HashSet<(ClassId, Builtin)>,
}

/// The state [`Checker::check_definitions`] rolls back to. The queues are
/// set aside rather than measured, so that the check drains only its own work.
struct Mark {
    types: TypeTable,
    sigs: usize,
    classes: usize,
    generics: GenericsMark,
    synth_pending: VecDeque<SynthJob>,
    conformance: VecDeque<(ClassId, Vec<Frame>)>,
}

/// Copies the default methods of user traits into the classes that declare
/// the trait and do not define a method of that name (DESIGN §3.11). Purely
/// syntactic; returns `None` when there is nothing to copy.
pub(crate) fn expand_trait_defaults(module: &ast::Module) -> Option<ast::Module> {
    let mut traits: HashMap<&str, &ast::TraitDecl> = HashMap::new();
    for item in &module.items {
        if let ast::Item::Trait(decl) = item {
            traits.entry(decl.name.as_str()).or_insert(decl);
        }
    }
    if traits.is_empty() {
        return None;
    }
    let mut expanded = module.clone();
    let mut changed = false;
    for item in &mut expanded.items {
        let ast::Item::Class(class) = item else {
            continue;
        };
        for name in class.traits.clone() {
            let Some(decl) = traits.get(name.as_str()) else {
                continue;
            };
            for method in decl.methods.iter().filter(|m| m.has_body) {
                let defined = class
                    .methods
                    .iter()
                    .any(|m| m.name.name == method.decl.name.name);
                if !defined {
                    class.methods.push(method.decl.clone());
                    changed = true;
                }
            }
        }
    }
    changed.then_some(expanded)
}

/// `fn name(self, other: T) -> R`, for a diagnostic.
fn signature_text(name: &str, params: &[(String, String)], ret: &str) -> String {
    let mut text = format!("fn {name}(self");
    for (param, ty) in params {
        text.push_str(&format!(", {param}: {ty}"));
    }
    text.push(')');
    if ret != "None" {
        text.push_str(&format!(" -> {ret}"));
    }
    text
}

/// The AST of a builtin trait's method signature.
fn builtin_decl(builtin: Builtin) -> ast::FnDecl {
    let span = Span::dummy();
    let named = |name: &str| {
        ast::TypeExpr::new(
            ast::TypeExprKind::Named {
                name: ast::Ident::new(name, span),
                args: Vec::new(),
            },
            span,
        )
    };
    let mut params = vec![ast::Param {
        name: ast::Ident::new("self", span),
        ty: None,
        default: None,
        is_self: true,
        span,
    }];
    if builtin.binary() {
        params.push(ast::Param {
            name: ast::Ident::new("other", span),
            ty: Some(named("Self")),
            default: None,
            is_self: false,
            span,
        });
    }
    ast::FnDecl {
        name: ast::Ident::new(builtin.method(), span),
        generics: Vec::new(),
        params,
        ret: Some(named(builtin.ret())),
        body: ast::Block {
            stmts: Vec::new(),
            span,
        },
        span,
    }
}

impl Checker<'_> {
    // -- declarations -----------------------------------------------------

    /// Registers the builtin traits and the name of every user trait; the
    /// signatures are validated by [`Checker::validate_traits`] once class
    /// names are known.
    pub(crate) fn register_traits(&mut self, module: &ast::Module) {
        for builtin in Builtin::ALL {
            let id = TraitId(self.traits.list.len());
            debug_assert_eq!(id, builtin.id());
            self.traits.list.push(TraitInfo {
                name: builtin.name().to_string(),
                span: Span::dummy(),
                builtin: Some(builtin),
                methods: vec![TraitMethodInfo {
                    decl: Rc::new(builtin_decl(builtin)),
                    has_body: false,
                    poisoned: false,
                }],
                poisoned: false,
            });
            self.traits.by_name.insert(builtin.name().to_string(), id);
        }
        for item in &module.items {
            let ast::Item::Trait(decl) = item else {
                continue;
            };
            let name = decl.name.as_str();
            if let Some(existing) = self.traits.by_name.get(name).copied() {
                let previous = &self.traits.list[existing.0];
                let diag = if previous.builtin.is_some() {
                    Diagnostic::error(format!("`{name}` is a builtin trait"))
                        .with_label(decl.name.span, "a builtin trait cannot be redeclared")
                        .with_help("pick another name")
                        .with_note("see DESIGN.md section 3.11")
                } else {
                    Diagnostic::error(format!("the name `{name}` is defined multiple times"))
                        .with_label(decl.name.span, format!("`{name}` redefined here"))
                        .with_secondary(previous.span, "previous definition here")
                };
                self.error(diag);
                continue;
            }
            let id = TraitId(self.traits.list.len());
            let mut methods = Vec::with_capacity(decl.methods.len());
            for (index, method) in decl.methods.iter().enumerate() {
                if method.has_body {
                    self.traits
                        .defaults_by_span
                        .insert(method.decl.name.span, (id, index));
                }
                methods.push(TraitMethodInfo {
                    decl: Rc::new(method.decl.clone()),
                    has_body: method.has_body,
                    poisoned: false,
                });
            }
            self.traits.list.push(TraitInfo {
                name: name.to_string(),
                span: decl.name.span,
                builtin: None,
                methods,
                poisoned: false,
            });
            self.traits.by_name.insert(name.to_string(), id);
        }
    }

    /// Validates the method signatures of every user trait, with `Self`
    /// unknown.
    pub(crate) fn validate_traits(&mut self) {
        for index in Builtin::ALL.len()..self.traits.list.len() {
            let trait_name = self.traits.list[index].name.clone();
            let decls: Vec<Rc<ast::FnDecl>> = self.traits.list[index]
                .methods
                .iter()
                .map(|m| Rc::clone(&m.decl))
                .collect();
            let before = self.generics.errors;
            let saved = std::mem::replace(
                &mut self.generics.env,
                vec![("Self".to_string(), TypeId::ERROR)],
            );
            let mut seen: Vec<&str> = Vec::new();
            for decl in &decls {
                let name = decl.name.as_str();
                if !decl.generics.is_empty() {
                    self.reject_generic_method(decl);
                }
                if !decl.is_method() {
                    let span = decl.params.first().map_or(decl.name.span, |p| p.span);
                    self.error(
                        Diagnostic::error(format!(
                            "the trait method `{name}` must take `self` as its first parameter"
                        ))
                        .with_label(span, "expected `self` here")
                        .with_note("see DESIGN.md section 3.11"),
                    );
                }
                if seen.contains(&name) {
                    self.error(
                        Diagnostic::error(format!(
                            "the method `{name}` is declared twice in the trait `{trait_name}`"
                        ))
                        .with_label(decl.name.span, "duplicate method"),
                    );
                }
                seen.push(name);
                if name == "__ne__" {
                    self.reject_ne(decl.name.span);
                }
                for param in decl.params.iter().skip(1) {
                    if let Some(ty) = &param.ty {
                        self.resolve_type(ty);
                    }
                }
                if let Some(ret) = &decl.ret {
                    self.resolve_type(ret);
                }
            }
            self.generics.env = saved;
            if self.generics.errors != before {
                self.traits.list[index].poisoned = true;
            }
        }
    }

    /// Reports a `__ne__` method: `!=` is always `not __eq__`.
    fn reject_ne(&mut self, span: Span) {
        self.error(
            Diagnostic::error("`__ne__` cannot be defined")
                .with_label(span, "`!=` is always `not __eq__`")
                .with_help("define `__eq__` instead; `a != b` calls it and negates the result")
                .with_note("see DESIGN.md section 3.11"),
        );
    }

    /// Resolves the trait list of a class header, `class C(T1, T2):`.
    pub(crate) fn collect_class_traits(&mut self, decl: &ast::ClassDecl) {
        for method in &decl.methods {
            if method.name.as_str() == "__ne__" {
                self.reject_ne(method.name.span);
            }
        }
        let mut list: Vec<(TraitId, Span)> = Vec::new();
        for name in &decl.traits {
            let text = name.as_str();
            if let Some(id) = self.traits.by_name.get(text).copied() {
                if list.iter().any(|(t, _)| *t == id) {
                    self.error(
                        Diagnostic::error(format!("the trait `{text}` is listed twice"))
                            .with_label(name.span, "duplicate trait"),
                    );
                    continue;
                }
                list.push((id, name.span));
            } else if self.classes.contains_key(text) || self.generics.has_class(text) {
                self.error(
                    Diagnostic::error("inheritance is not supported")
                        .with_label(name.span, format!("`{text}` is a class, not a trait"))
                        .with_help("compose instead: give the class a field of the other type")
                        .with_note(
                            "the parentheses after a class name list the traits it \
                             implements, see DESIGN.md section 3.11",
                        ),
                );
            } else if !self.poisoned.contains(text) {
                self.error(
                    Diagnostic::error(format!("cannot find trait `{text}` in this scope"))
                        .with_label(name.span, "not a known trait")
                        .with_help(
                            "the builtin traits are `Eq`, `Ord`, `Hash`, `Repr`, `Add`, `Sub`, \
                             `Mul`, `Div`, `FloorDiv`, `Mod` and `Neg`",
                        ),
                );
            }
        }
        let has_eq = list.iter().any(|(t, _)| *t == Builtin::Eq.id());
        for (id, span) in &list {
            if let Some(builtin) = Builtin::of_id(*id)
                && builtin.requires_eq()
                && !has_eq
            {
                let class = decl.name.as_str();
                let name = builtin.name();
                self.error(
                    Diagnostic::error(format!("the trait `{name}` requires `Eq`"))
                        .with_label(*span, format!("`{class}` declares `{name}` but not `Eq`"))
                        .with_help(format!("declare both: `class {class}(Eq, {name}):`"))
                        .with_note("see DESIGN.md section 3.11"),
                );
            }
        }
        if list.is_empty() {
            return;
        }
        if !decl.generics.is_empty() {
            self.traits
                .template_declared
                .insert(decl.name.name.clone(), list);
        } else if let Some(entry) = self.classes.get(decl.name.as_str()) {
            self.traits.declared.insert(entry.id, list);
        }
    }

    /// Where the trait `name` is declared, for a duplicate-name error; a
    /// dummy span for a builtin trait.
    pub(crate) fn trait_name_span(&self, name: &str) -> Option<Span> {
        let id = self.traits.by_name.get(name)?;
        Some(self.traits.list[id.0].span)
    }

    /// Reports a trait written where a type is expected, returning whether
    /// `name` is a trait: traits are only bounds (DESIGN §3.11 rule 4).
    pub(crate) fn trait_as_type(&mut self, name: &str, span: Span) -> bool {
        if !self.traits.by_name.contains_key(name) {
            return false;
        }
        self.error(
            Diagnostic::error(format!("the trait `{name}` cannot be used as a type"))
                .with_label(span, "a trait is not a type")
                .with_help(format!(
                    "take a type parameter bounded by it instead: `fn f<T: {name}>(x: T)`"
                ))
                .with_note(
                    "traits are dispatched statically and have no values of their own, \
                     see DESIGN.md section 3.11",
                ),
        );
        true
    }

    /// Whether `decl` is the copy of a trait default that failed its own
    /// definition-time check.
    pub(crate) fn is_poisoned_default(&self, decl: &ast::FnDecl) -> bool {
        self.traits
            .defaults_by_span
            .get(&decl.name.span)
            .is_some_and(|(t, m)| self.traits.list[t.0].methods[*m].poisoned)
    }

    /// The traits `class` declares: its own list, or its template's.
    fn declared_of(&self, class: ClassId) -> Vec<(TraitId, Span)> {
        if let Some(list) = self.traits.declared.get(&class) {
            return list.clone();
        }
        self.generics
            .origins
            .get(&class)
            .and_then(|(template, _)| self.traits.template_declared.get(template))
            .cloned()
            .unwrap_or_default()
    }

    /// Whether `class` is the pseudo-class of a type parameter.
    pub(crate) fn is_opaque(&self, class: ClassId) -> bool {
        self.traits.opaque.contains_key(&class)
    }

    /// The protocol tables codegen reads (DESIGN §3.11).
    pub(crate) fn protocols(&self) -> hir::Protocols {
        hir::Protocols {
            eq: self.traits.eq_fns.clone(),
            repr: self.traits.repr_fns.clone(),
            hash: self.traits.hash_fns.clone(),
            key_probe: self.traits.key_probes.clone(),
        }
    }

    // -- conformance ------------------------------------------------------

    /// Checks the trait conformance of every non-generic class and registers
    /// its derived methods; generic class instances are checked as they are
    /// created ([`Checker::queue_conformance`]).
    pub(crate) fn check_conformance(&mut self, module: &ast::Module) {
        for item in &module.items {
            let ast::Item::Class(decl) = item else {
                continue;
            };
            if !decl.generics.is_empty() {
                continue;
            }
            let Some(class) = self.classes.get(decl.name.as_str()).map(|c| c.id) else {
                continue;
            };
            let list = self.declared_of(class);
            self.check_class_conformance(class, &list);
        }
    }

    /// Queues the conformance check of a new instance of `template`.
    pub(crate) fn queue_conformance(&mut self, class: ClassId, template: &str) {
        if self.traits.template_declared.contains_key(template) {
            let stack = self.generics.stack.clone();
            self.traits.conformance.push_back((class, stack));
        }
    }

    /// Runs the queued conformance checks of generic class instances.
    pub(crate) fn run_conformance_queue(&mut self) {
        while let Some((class, stack)) = self.traits.conformance.pop_front() {
            let saved = std::mem::replace(&mut self.generics.stack, stack);
            let list = self.declared_of(class);
            self.check_class_conformance(class, &list);
            self.generics.stack = saved;
        }
    }

    /// Whether a conformance check is still queued.
    pub(crate) fn has_queued_conformance(&self) -> bool {
        !self.traits.conformance.is_empty()
    }

    /// Checks that `class` has every method of every trait in `list`,
    /// deriving the builtin ones it leaves out.
    fn check_class_conformance(&mut self, class: ClassId, list: &[(TraitId, Span)]) {
        let generic = self.generics.origins.contains_key(&class);
        // The derivable builtin traits go first, so that a user trait
        // requiring the same dunder finds the derived method whatever the
        // order of the header: `class C(MyEq, Eq)` is `class C(Eq, MyEq)`.
        let derives = |id: &TraitId| Builtin::of_id(*id).is_some_and(Builtin::derivable);
        let ordered: Vec<(TraitId, Span)> = list
            .iter()
            .filter(|(id, _)| derives(id))
            .chain(list.iter().filter(|(id, _)| !derives(id)))
            .copied()
            .collect();
        for (id, span) in ordered {
            if self.traits.list[id.0].poisoned {
                continue;
            }
            let builtin = self.traits.list[id.0].builtin;
            let decls: Vec<Rc<ast::FnDecl>> = self.traits.list[id.0]
                .methods
                .iter()
                .map(|m| Rc::clone(&m.decl))
                .collect();
            for decl in decls {
                let name = decl.name.name.clone();
                // A generic class derives per instance and only when the
                // method is used, so `Pair<Inner>` is fine until it is
                // printed (DESIGN §3.11: template semantics).
                if generic && builtin.is_some_and(|b| self.derived_lazily(class, b)) {
                    continue;
                }
                let found = match self.methods.get(&(class, name.clone())).copied() {
                    Some(func) => Some(func),
                    // A generic instance instantiates it now.
                    None if generic => {
                        self.instantiate_method(class, &ast::Ident::new(name.clone(), span))
                    }
                    None => None,
                };
                match (found, builtin) {
                    (Some(func), _) => self.compare_with_trait(class, id, &decl, func),
                    (None, Some(b)) if b.derivable() => {
                        if generic || self.traits.derive_failed.contains(&(class, b)) {
                            // Reported by the instance's derivation already.
                            continue;
                        }
                        if self.derive_fields_ok(class, b, span) {
                            self.register_derived(class, b, span);
                        } else {
                            self.traits.derive_failed.insert((class, b));
                        }
                    }
                    (None, _) => self.missing_method(class, id, &decl, span),
                }
            }
        }
    }

    /// The parameter types (after `self`) and return type of a trait method
    /// with `Self` bound to `self_ty`.
    pub(crate) fn trait_method_sig(
        &mut self,
        decl: &ast::FnDecl,
        self_ty: TypeId,
    ) -> (Vec<TypeId>, TypeId) {
        let saved = std::mem::replace(&mut self.generics.env, vec![("Self".to_string(), self_ty)]);
        let params = decl
            .params
            .iter()
            .skip(1)
            .map(|p| match &p.ty {
                Some(ty) => self.resolve_type(ty),
                None => TypeId::ERROR,
            })
            .collect();
        let ret = match &decl.ret {
            Some(ty) => self.resolve_type(ty),
            None => TypeId::UNIT,
        };
        self.generics.env = saved;
        (params, ret)
    }

    /// Compares the signature of a class's method with the trait's.
    fn compare_with_trait(
        &mut self,
        class: ClassId,
        id: TraitId,
        decl: &ast::FnDecl,
        func: hir::FuncId,
    ) {
        let class_ty = self.types.class_ty(class);
        let (want, want_ret) = self.trait_method_sig(decl, class_ty);
        let sig = &self.sigs[func.index()];
        let got: Vec<TypeId> = sig.params.iter().skip(1).map(|p| p.ty).collect();
        let got_ret = sig.ret;
        if got == want && got_ret == want_ret {
            return;
        }
        if got.contains(&TypeId::ERROR)
            || want.contains(&TypeId::ERROR)
            || got_ret == TypeId::ERROR
            || want_ret == TypeId::ERROR
        {
            return;
        }
        let name = decl.name.as_str();
        let class_name = self.types.class(class).name.clone();
        let trait_name = self.traits.list[id.0].name.clone();
        let want_params: Vec<(String, String)> = decl
            .params
            .iter()
            .skip(1)
            .zip(&want)
            .map(|(p, t)| (p.name.name.clone(), self.types.name(*t)))
            .collect();
        let got_params: Vec<(String, String)> = sig
            .params
            .iter()
            .skip(1)
            .map(|p| (p.name.clone(), self.types.name(p.ty)))
            .collect();
        let expected = signature_text(name, &want_params, &self.types.name(want_ret));
        let found = signature_text(name, &got_params, &self.types.name(got_ret));
        let name_span = sig.name_span;
        let mut diag = Diagnostic::error(format!(
            "the method `{name}` of `{class_name}` does not match the trait `{trait_name}`"
        ))
        .with_label(name_span, format!("expected `{expected}`"))
        .with_note(format!("found `{found}`"));
        if !decl.name.span.is_dummy() {
            diag = diag.with_secondary(decl.name.span, format!("`{trait_name}` declares it here"));
        }
        self.error(diag.with_note("`Self` in a trait stands for the implementing class"));
    }

    /// Reports a required method a class does not define.
    fn missing_method(&mut self, class: ClassId, id: TraitId, decl: &ast::FnDecl, span: Span) {
        let class_ty = self.types.class_ty(class);
        let (want, want_ret) = self.trait_method_sig(decl, class_ty);
        let name = decl.name.as_str();
        let class_name = self.types.class(class).name.clone();
        let trait_name = self.traits.list[id.0].name.clone();
        let params: Vec<(String, String)> = decl
            .params
            .iter()
            .skip(1)
            .zip(&want)
            .map(|(p, t)| (p.name.name.clone(), self.types.name(*t)))
            .collect();
        let expected = signature_text(name, &params, &self.types.name(want_ret));
        let mut diag = Diagnostic::error(format!(
            "`{class_name}` does not implement the method `{name}` required by the trait \
             `{trait_name}`"
        ))
        .with_label(span, format!("`{name}` is missing"))
        .with_help(format!("add `{expected}` to `{class_name}`"));
        if !decl.name.span.is_dummy() {
            diag = diag.with_secondary(decl.name.span, "required by this declaration");
        }
        self.error(diag);
    }

    /// Checks that every field of `class` implements `builtin`, which a
    /// field-wise derivation needs.
    fn derive_fields_ok(&mut self, class: ClassId, builtin: Builtin, span: Span) -> bool {
        let fields = self.types.class(class).fields.clone();
        let class_name = self.types.class(class).name.clone();
        let trait_name = builtin.name();
        let mut ok = true;
        for field in fields {
            if self.satisfies(field.ty, builtin.id()) {
                continue;
            }
            let ty = self.types.name(field.ty);
            let help = match self.types.get(field.ty) {
                Type::Class(_) => format!(
                    "declare `{trait_name}` on `{ty}`, or write `{}` for `{class_name}` by hand",
                    builtin.method()
                ),
                _ => format!("write `{}` for `{class_name}` by hand", builtin.method()),
            };
            self.error(
                Diagnostic::error(format!(
                    "cannot derive `{trait_name}` for `{class_name}`: the field `{}` has type \
                     `{ty}`, which does not implement `{trait_name}`",
                    field.name
                ))
                .with_label(span, format!("`{trait_name}` is derived field by field"))
                .with_help(help)
                .with_note("see DESIGN.md section 3.11"),
            );
            ok = false;
        }
        ok
    }

    /// The derived method `name` of an instance of the generic class
    /// `template`, when the template declares the trait without defining it.
    pub(crate) fn derive_for_instance(
        &mut self,
        class: ClassId,
        template: &str,
        name: &ast::Ident,
    ) -> Option<hir::FuncId> {
        let builtin = Builtin::from_method(name.as_str()).filter(|b| b.derivable())?;
        let (_, span) = self
            .traits
            .template_declared
            .get(template)?
            .iter()
            .find(|(t, _)| *t == builtin.id())
            .copied()?;
        if self.traits.derive_failed.contains(&(class, builtin)) {
            return None;
        }
        if !self.derive_fields_ok(class, builtin, span) {
            self.traits.derive_failed.insert((class, builtin));
            return None;
        }
        Some(self.register_derived(class, builtin, span))
    }

    /// Whether the generic class `template` derives the method `name`.
    pub(crate) fn template_derives(&self, template: &str, name: &str) -> bool {
        let Some(builtin) = Builtin::from_method(name).filter(|b| b.derivable()) else {
            return false;
        };
        self.traits
            .template_declared
            .get(template)
            .is_some_and(|list| list.iter().any(|(t, _)| *t == builtin.id()))
    }

    // -- bounds -------------------------------------------------------------

    /// Checks that every bound of a type parameter names a trait.
    pub(crate) fn check_bound_names(&mut self, param: &ast::GenericParam) -> bool {
        let mut ok = true;
        for bound in &param.bounds {
            let text = bound.as_str();
            if self.traits.by_name.contains_key(text) {
                continue;
            }
            ok = false;
            if self.poisoned.contains(text) {
                continue;
            }
            let diag = if self.classes.contains_key(text) || self.generics.has_class(text) {
                Diagnostic::error(format!("`{text}` is a class, not a trait"))
                    .with_label(bound.span, "a bound must be a trait")
                    .with_help(format!(
                        "declare a trait for what `{}` needs, and implement it on `{text}`",
                        param.name.as_str()
                    ))
            } else {
                Diagnostic::error(format!("cannot find trait `{text}` in this scope"))
                    .with_label(bound.span, "not a known trait")
            };
            self.error(diag.with_note("see DESIGN.md section 3.11"));
        }
        ok
    }

    /// The traits a type parameter is bounded by.
    fn param_bounds(&self, param: &ast::GenericParam) -> Vec<TraitId> {
        param
            .bounds
            .iter()
            .filter_map(|b| self.traits.by_name.get(b.as_str()).copied())
            .collect()
    }

    /// Checks that each type argument implements the bounds of its parameter
    /// (DESIGN §3.11), reporting every one that does not.
    pub(crate) fn check_type_bounds(
        &mut self,
        what: &str,
        params: &[ast::GenericParam],
        args: &[TypeId],
        at: Span,
    ) -> bool {
        let mut ok = true;
        let item = what.split('<').next().unwrap_or(what).to_string();
        for (param, &arg) in params.iter().zip(args) {
            if arg == TypeId::ERROR {
                continue;
            }
            for bound in &param.bounds {
                let Some(id) = self.traits.by_name.get(bound.as_str()).copied() else {
                    continue;
                };
                if self.satisfies(arg, id) {
                    continue;
                }
                let ty = self.types.name(arg);
                let trait_name = bound.as_str();
                let mut diag = Diagnostic::error(format!(
                    "the type `{ty}` does not implement the trait `{trait_name}`"
                ))
                .with_label(
                    at,
                    format!(
                        "required by the bound `{}: {trait_name}` of `{item}`",
                        param.name.as_str()
                    ),
                )
                .with_secondary(bound.span, "bound declared here");
                let hash = id == Builtin::Hash.id();
                diag = match self.types.get(arg) {
                    Type::Class(class) if self.is_opaque(class) => {
                        diag.with_help(format!("add the bound: `{ty}: {trait_name}`"))
                    }
                    Type::Optional(_) | Type::Tuple(_) if hash => diag.with_help(
                        "`Hash` is what a dict key needs, and a `T | None` or a tuple holding \
                         a class cannot be a key yet",
                    ),
                    Type::Class(_) => {
                        let list = if Builtin::of_id(id).is_some_and(Builtin::requires_eq) {
                            format!("Eq, {trait_name}")
                        } else {
                            trait_name.to_string()
                        };
                        diag.with_help(format!(
                            "a class implements a trait only when it declares it: \
                             `class {ty}({list}):`"
                        ))
                    }
                    _ => diag,
                };
                self.error(diag.with_note("see DESIGN.md section 3.11"));
                ok = false;
            }
        }
        ok
    }

    /// Whether every type argument implements its parameter's bounds, without
    /// reporting anything.
    pub(crate) fn bounds_hold(&mut self, params: &[ast::GenericParam], args: &[TypeId]) -> bool {
        for (param, &arg) in params.iter().zip(args) {
            for id in self.param_bounds(param) {
                if !self.satisfies(arg, id) {
                    return false;
                }
            }
        }
        true
    }

    /// Whether `ty` implements the trait `id` (DESIGN §3.11): a class when it
    /// declares it, a type parameter when it is bounded by it, and the builtin
    /// types per the table of §3.11.
    pub(crate) fn satisfies(&mut self, ty: TypeId, id: TraitId) -> bool {
        match self.types.get(ty) {
            Type::Error => true,
            Type::Class(class) => {
                if let Some(bounds) = self.traits.opaque.get(&class) {
                    return bounds.contains(&id);
                }
                if !self.declared_of(class).iter().any(|(t, _)| *t == id) {
                    return false;
                }
                // An instance of a generic class that derives the trait
                // implements it when its fields do.
                match Builtin::of_id(id) {
                    Some(b) if self.derived_lazily(class, b) => self.fields_satisfy(class, b),
                    _ => true,
                }
            }
            _ => match Builtin::of_id(id) {
                Some(builtin) => self.builtin_satisfies(ty, builtin),
                None => false,
            },
        }
    }

    /// Whether `class` is an instance of a generic class that declares the
    /// derivable `builtin` without defining its method, so that the method is
    /// derived on first use.
    fn derived_lazily(&self, class: ClassId, builtin: Builtin) -> bool {
        let Some((template, _)) = self.generics.origins.get(&class) else {
            return false;
        };
        builtin.derivable()
            && self.template_derives(template, builtin.method())
            && !self.generics.classes.get(template).is_some_and(|t| {
                t.methods
                    .iter()
                    .any(|m| m.name.as_str() == builtin.method())
            })
    }

    /// Whether every field of `class` implements `builtin`, silently. A cycle
    /// through the class itself (`next: Node<T> | None`) counts as implemented.
    fn fields_satisfy(&mut self, class: ClassId, builtin: Builtin) -> bool {
        if self.traits.derive_failed.contains(&(class, builtin)) {
            return false;
        }
        if !self.traits.satisfying.insert((class, builtin)) {
            return true;
        }
        let fields: Vec<TypeId> = self
            .types
            .class(class)
            .fields
            .iter()
            .map(|f| f.ty)
            .collect();
        let ok = fields
            .into_iter()
            .all(|ty| self.satisfies(ty, builtin.id()));
        self.traits.satisfying.remove(&(class, builtin));
        ok
    }

    /// Which builtin traits the builtin types implement.
    fn builtin_satisfies(&mut self, ty: TypeId, builtin: Builtin) -> bool {
        use Builtin::*;
        match self.types.get(ty) {
            Type::Int => matches!(
                builtin,
                Eq | Ord | Hash | Repr | Add | Sub | Mul | FloorDiv | Mod | Neg
            ),
            Type::Bool => matches!(builtin, Eq | Ord | Hash | Repr),
            Type::Str => matches!(builtin, Eq | Ord | Hash | Repr | Add),
            // No `Hash`: NaN breaks hash consistency.
            Type::Float => matches!(
                builtin,
                Eq | Ord | Repr | Add | Sub | Mul | Div | FloorDiv | Mod | Neg
            ),
            // `Hash` only for the flat tuples a dict takes as keys in M3.
            Type::Tuple(_) if builtin == Hash => self.types.is_hashable(ty),
            Type::Tuple(id) => {
                matches!(builtin, Eq | Ord | Repr) && {
                    let members = self.types.tuple_members(id).to_vec();
                    members.into_iter().all(|m| self.satisfies(m, builtin.id()))
                }
            }
            // Mutable, so no `Hash`.
            Type::List(elem) => match builtin {
                Eq | Repr => self.satisfies(elem, builtin.id()),
                Add => true,
                _ => false,
            },
            // No `Hash` yet: a `T | None` key is deferred (DESIGN §3.10).
            Type::Optional(inner) => {
                matches!(builtin, Eq | Repr) && self.satisfies(inner, builtin.id())
            }
            // No `==` on dicts and sets yet, and they are mutable.
            Type::Set(elem) => builtin == Repr && self.satisfies(elem, builtin.id()),
            Type::Dict(key, value) => {
                builtin == Repr
                    && self.satisfies(key, builtin.id())
                    && self.satisfies(value, builtin.id())
            }
            Type::Error => true,
            Type::Unit | Type::Class(_) => false,
        }
    }

    // -- definition-time checking -------------------------------------------

    /// Checks every trait default and every generic item with a bound once,
    /// at its definition, with its type parameters opaque (DESIGN §3.11).
    /// A failed item is poisoned, so that its uses add nothing.
    pub(crate) fn check_definitions(&mut self) {
        // Trait defaults, with `Self` an opaque implementor of the trait.
        for index in Builtin::ALL.len()..self.traits.list.len() {
            if self.traits.list[index].poisoned {
                continue;
            }
            let defaults: Vec<(usize, Rc<ast::FnDecl>)> = self.traits.list[index]
                .methods
                .iter()
                .enumerate()
                .filter(|(_, m)| m.has_body)
                .map(|(i, m)| (i, Rc::clone(&m.decl)))
                .collect();
            if defaults.is_empty() {
                continue;
            }
            let trait_name = self.traits.list[index].name.clone();
            let mark = self.mark();
            let self_ty = self.opaque_param("Self", &[TraitId(index)]);
            let class = self.types.class_of(self_ty);
            let mut failed = Vec::new();
            for (method, decl) in defaults {
                let before = self.generics.errors;
                let name = format!("{trait_name}.{}", decl.name.as_str());
                self.check_at_definition(&decl, class, vec![("Self".to_string(), self_ty)], name);
                let _ = self.drain_queues();
                if self.generics.errors != before {
                    failed.push(method);
                }
            }
            self.rollback(mark);
            for method in failed {
                self.traits.list[index].methods[method].poisoned = true;
            }
        }

        // Generic functions with at least one bound.
        for index in 0..self.generics.templates.len() {
            let template = &self.generics.templates[index];
            if template.class.is_some()
                || template.poisoned
                || template.decl.generics.iter().all(|g| g.bounds.is_empty())
            {
                continue;
            }
            let decl = Rc::clone(&template.decl);
            let mark = self.mark();
            let before = self.generics.errors;
            let args = self.opaque_params(&decl.generics);
            let env = bind(&decl.generics, &args);
            self.check_at_definition(&decl, None, env, decl.name.name.clone());
            let _ = self.drain_queues();
            self.rollback(mark);
            if self.generics.errors != before {
                self.generics.templates[index].poisoned = true;
            }
        }

        // Generic classes with at least one bound: an opaque instance, its
        // conformance and every method.
        for name in self.generics.class_order.clone() {
            let Some(template) = self.generics.classes.get(&name) else {
                continue;
            };
            if template.poisoned || template.decl.generics.iter().all(|g| g.bounds.is_empty()) {
                continue;
            }
            let decl = Rc::clone(&template.decl);
            let methods: Vec<ast::Ident> =
                template.methods.iter().map(|m| m.name.clone()).collect();
            let mark = self.mark();
            let before = self.generics.errors;
            let saved_flag = std::mem::replace(&mut self.generics.validating, true);
            let args = self.opaque_params(&decl.generics);
            let ty = self.instantiate_class(&name, args, decl.name.span);
            if let Type::Class(class) = self.types.get(ty) {
                for method in &methods {
                    self.instantiate_method(class, method);
                }
            }
            let _ = self.drain_queues();
            self.generics.validating = saved_flag;
            self.rollback(mark);
            if self.generics.errors != before
                && let Some(template) = self.generics.classes.get_mut(&name)
            {
                template.poisoned = true;
            }
        }
    }

    /// Checks the body of `decl` once under `env`, as a throwaway function.
    fn check_at_definition(
        &mut self,
        decl: &ast::FnDecl,
        class: Option<ClassId>,
        env: Vec<(String, TypeId)>,
        name: String,
    ) {
        let saved_env = std::mem::replace(&mut self.generics.env, env);
        let saved_stack = std::mem::take(&mut self.generics.stack);
        let saved_flag = std::mem::replace(&mut self.generics.validating, true);
        let sig = self.instance_sig(decl, class, name, "ty_definition_check".to_string());
        let id = hir::FuncId(self.sigs.len() as u32);
        self.sigs.push(sig);
        let _ = self.check_function(decl, id);
        self.generics.validating = saved_flag;
        self.generics.stack = saved_stack;
        self.generics.env = saved_env;
    }

    /// One opaque type per type parameter.
    fn opaque_params(&mut self, params: &[ast::GenericParam]) -> Vec<TypeId> {
        params
            .iter()
            .map(|param| {
                let bounds = self.param_bounds(param);
                self.opaque_param(param.name.as_str(), &bounds)
            })
            .collect()
    }

    /// A pseudo-class standing for a type parameter bounded by `bounds`: no
    /// fields, and exactly the methods of its bounds (`Ord` and `Hash` bring
    /// `Eq` along, since every implementor declares it too).
    fn opaque_param(&mut self, name: &str, bounds: &[TraitId]) -> TypeId {
        let mut all = bounds.to_vec();
        let eq = Builtin::Eq.id();
        if all
            .iter()
            .any(|t| Builtin::of_id(*t).is_some_and(Builtin::requires_eq))
            && !all.contains(&eq)
        {
            all.push(eq);
        }
        let (class, ty) = self.types.declare_class(name);
        self.types.set_fields(class, Vec::new());
        self.traits.opaque.insert(class, all.clone());
        for id in all {
            if self.traits.list[id.0].poisoned {
                continue;
            }
            let decls: Vec<Rc<ast::FnDecl>> = self.traits.list[id.0]
                .methods
                .iter()
                .map(|m| Rc::clone(&m.decl))
                .collect();
            for decl in decls {
                let method = decl.name.name.clone();
                if self.methods.contains_key(&(class, method.clone())) {
                    continue;
                }
                let (params, ret) = self.trait_method_sig(&decl, ty);
                let mut sig_params = vec![ParamSig {
                    name: "self".to_string(),
                    ty,
                    span: decl.name.span,
                    default: None,
                }];
                for (param, ty) in decl.params.iter().skip(1).zip(params) {
                    sig_params.push(ParamSig {
                        name: param.name.name.clone(),
                        ty,
                        span: param.name.span,
                        default: None,
                    });
                }
                let id = hir::FuncId(self.sigs.len() as u32);
                self.sigs.push(FnSig {
                    name: format!("{name}.{method}"),
                    symbol: "ty_definition_check".to_string(),
                    params: sig_params,
                    ret,
                    name_span: decl.name.span,
                    self_ty: Some(ty),
                });
                self.methods.insert((class, method), id);
            }
        }
        ty
    }

    /// Records the state a definition-time check rolls back to.
    fn mark(&mut self) -> Mark {
        Mark {
            types: self.types.clone(),
            sigs: self.sigs.len(),
            classes: self.types.classes().len(),
            generics: self.generics.snapshot(),
            synth_pending: std::mem::take(&mut self.traits.pending),
            conformance: std::mem::take(&mut self.traits.conformance),
        }
    }

    /// Forgets every type, signature, instance and synthesized function
    /// created since `mark`; only the diagnostics remain.
    fn rollback(&mut self, mark: Mark) {
        let (sigs, classes) = (mark.sigs, mark.classes);
        self.types = mark.types;
        self.sigs.truncate(sigs);
        self.methods
            .retain(|(class, _), func| class.index() < classes && func.index() < sigs);
        self.classes.retain(|_, entry| entry.id.index() < classes);
        self.generics.restore(mark.generics, sigs, classes);
        let traits = &mut self.traits;
        traits
            .eq_fns
            .retain(|class, func| class.index() < classes && func.index() < sigs);
        traits
            .repr_fns
            .retain(|class, func| class.index() < classes && func.index() < sigs);
        traits
            .hash_fns
            .retain(|class, func| class.index() < classes && func.index() < sigs);
        traits
            .key_probes
            .retain(|class, func| class.index() < classes && func.index() < sigs);
        traits.synth.retain(|_, func| func.index() < sigs);
        traits.pending = mark.synth_pending;
        traits
            .derive_failed
            .retain(|(class, _)| class.index() < classes);
        traits.opaque.retain(|class, _| class.index() < classes);
        traits.conformance = mark.conformance;
    }
}
