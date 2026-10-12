//! User-defined generics: generic functions and generic classes, implemented
//! by monomorphization (DESIGN §3.9, §4.4, §4.5).
//!
//! A generic declaration is kept as AST — a *template*. Every distinct tuple
//! of type arguments it is used with produces one *instance*: an ordinary
//! class in the [`TypeTable`](crate::types::TypeTable), or an ordinary
//! [`FnSig`] plus a queued body. The body is checked like any other function,
//! with the type parameters bound to the concrete type arguments, so the HIR
//! both backends consume is entirely monomorphic: neither codegen knows that
//! generics exist.
//!
//! * Instances are memoized per `(template, type arguments)`.
//! * An item without trait bounds is a template: an operator applied to a
//!   `T` is checked once `T` is known, inside the instance, and every
//!   diagnostic raised there carries the instantiation stack
//!   (`in instantiation of `max<Point>``). An item with a bound is also
//!   checked once at its definition (DESIGN §3.11, see `traits.rs`), and its
//!   instances must satisfy the bounds.
//! * The methods of a generic class are instantiated lazily, on first call, so
//!   a method nobody calls costs nothing.
//! * Polymorphic recursion (`f<T>` calling `f<list<T>>`) cannot be
//!   monomorphized; it runs into a depth limit with a dedicated error.
//! * A template's signature (and a generic class's fields) are validated once
//!   at declaration, with the type parameters standing for "unknown"; its body
//!   is only checked through its instances, the way C++ checks templates.
//!
//! Instance names are spelled like source types (`max<int>`,
//! `Stack<list<int>>.push`); their symbols escape the punctuation with `.`,
//! which no identifier may contain, so they can never collide with a
//! non-generic symbol (see [`symbol_safe`]).

use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

use typhoon_ast as ast;
use typhoon_diag::{Diagnostic, Span};

use crate::check::{Checker, ClassEntry, ConstValue, FnSig, ParamSig};
use crate::hir;
use crate::types::{ClassId, FieldInfo, Type, TypeId};

/// How deep a chain of instantiations may get before it is treated as
/// polymorphic recursion.
const MAX_DEPTH: usize = 64;

/// How many instances one program may create in total; a guard against a
/// recursion that branches instead of growing one chain.
const MAX_INSTANCES: usize = 10_000;

/// How many frames of the instantiation stack a diagnostic lists.
const MAX_NOTES: usize = 8;

/// How many frames of the instantiation stack get a secondary label.
const MAX_LABELS: usize = 3;

/// A generic function, or a method of one instance of a generic class.
pub(crate) struct FnTemplate {
    pub(crate) decl: Rc<ast::FnDecl>,
    /// The class instance a method belongs to; `None` for a free function.
    pub(crate) class: Option<ClassId>,
    /// The bindings of the enclosing class's type parameters.
    outer: Vec<(String, TypeId)>,
    /// The display name before type arguments: `max`, `Stack<int>.push`.
    name: String,
    /// The symbol before type arguments: `ty_user_max`.
    symbol: String,
    /// The signature was rejected; every use is silently an error.
    pub(crate) poisoned: bool,
    instances: HashMap<Vec<TypeId>, hir::FuncId>,
}

/// A generic class.
pub(crate) struct ClassTemplate {
    pub(crate) decl: Rc<ast::ClassDecl>,
    pub(crate) methods: Vec<Rc<ast::FnDecl>>,
    /// The declaration was rejected; every use is silently an error.
    pub(crate) poisoned: bool,
    instances: HashMap<Vec<TypeId>, ClassId>,
}

/// One frame of the instantiation stack.
#[derive(Debug, Clone)]
pub(crate) struct Frame {
    /// The instance, e.g. `max<Point>`.
    pub(crate) what: String,
    /// The use that required it.
    pub(crate) at: Span,
}

/// An instance whose body still has to be checked.
struct Pending {
    id: hir::FuncId,
    decl: Rc<ast::FnDecl>,
    env: Vec<(String, TypeId)>,
    stack: Vec<Frame>,
}

/// Everything the checker knows about generics.
#[derive(Default)]
pub(crate) struct Generics {
    /// Generic free functions by name, as indices into `templates`.
    fns: HashMap<String, usize>,
    pub(crate) templates: Vec<FnTemplate>,
    /// Method templates of generic class instances, created on first call.
    methods: HashMap<(ClassId, String), usize>,
    pub(crate) classes: HashMap<String, ClassTemplate>,
    /// Generic classes in declaration order, so diagnostics are stable.
    pub(crate) class_order: Vec<String>,
    /// The template and type arguments every class instance came from.
    pub(crate) origins: HashMap<ClassId, (String, Vec<TypeId>)>,
    /// The type parameters in scope and what they stand for.
    pub(crate) env: Vec<(String, TypeId)>,
    /// The instantiation stack of the code being checked.
    pub(crate) stack: Vec<Frame>,
    pending: VecDeque<Pending>,
    /// Diagnostics already reported from generic code, so that a mistake in a
    /// template is reported once, not once per instance.
    seen: HashSet<(String, Option<Span>)>,
    /// Set while a template is validated at its declaration, and while a
    /// bounded item is checked at its definition (DESIGN §3.11).
    pub(crate) validating: bool,
    /// Every error reported so far, including suppressed duplicates.
    pub(crate) errors: usize,
    /// Instances created so far.
    count: usize,
}

/// What [`Generics::snapshot`] records, so that the definition-time check of
/// a bounded item leaves no instance behind (DESIGN §3.11).
pub(crate) struct GenericsMark {
    templates: usize,
    /// The queued bodies, set aside so that the check only drains its own.
    pending: VecDeque<Pending>,
    count: usize,
}

impl Generics {
    /// Records the current extent of the instance tables and sets the queued
    /// bodies aside.
    pub(crate) fn snapshot(&mut self) -> GenericsMark {
        GenericsMark {
            templates: self.templates.len(),
            pending: std::mem::take(&mut self.pending),
            count: self.count,
        }
    }

    /// Forgets every instance, method template and queued body created since
    /// `mark`; `sigs` and `classes` are the sizes of the signature and class
    /// tables at that point.
    pub(crate) fn restore(&mut self, mark: GenericsMark, sigs: usize, classes: usize) {
        self.templates.truncate(mark.templates);
        for template in &mut self.templates {
            template.instances.retain(|_, id| id.index() < sigs);
        }
        self.methods
            .retain(|(class, _), index| class.index() < classes && *index < mark.templates);
        for template in self.classes.values_mut() {
            template.instances.retain(|_, id| id.index() < classes);
        }
        self.origins.retain(|class, _| class.index() < classes);
        self.pending = mark.pending;
        self.count = mark.count;
    }
}

impl Generics {
    /// Whether `name` is a generic function.
    pub(crate) fn has_fn(&self, name: &str) -> bool {
        self.fns.contains_key(name)
    }

    /// Whether `name` is a generic class.
    pub(crate) fn has_class(&self, name: &str) -> bool {
        self.classes.contains_key(name)
    }

    /// Where the generic function or class `name` is declared.
    pub(crate) fn name_span(&self, name: &str) -> Option<Span> {
        self.fns
            .get(name)
            .map(|t| self.templates[*t].decl.name.span)
            .or_else(|| self.classes.get(name).map(|c| c.decl.name.span))
    }
}

/// One parameter (or constructor field) a call's arguments are matched to.
struct Slot<'d> {
    name: &'d str,
    ty: Option<&'d ast::TypeExpr>,
}

/// A call site, for diagnostics.
struct Site<'s> {
    /// How the callee is named in diagnostics.
    display: &'s str,
    callee_span: Span,
    span: Span,
    /// A generated constructor, which is keyword-only.
    ctor: bool,
}

/// One argument of a generic call, checked during inference.
struct CheckedArg {
    /// The slot it fills.
    slot: usize,
    value: hir::Expr,
    /// When its parameter type mentions a type parameter another argument
    /// fixed: that argument and what it fixed, to explain a mismatch.
    why: Option<(Span, String)>,
}

/// The arguments of a call after inference: the type arguments, and every
/// checked argument, in source order.
type Inferred = (Vec<TypeId>, Vec<CheckedArg>);

impl Checker<'_> {
    // -- declarations -------------------------------------------------------

    /// Registers a generic class (DESIGN §3.9); called with the other class
    /// names, before any type is resolved.
    pub(crate) fn collect_generic_class(&mut self, decl: &ast::ClassDecl) {
        let name = decl.name.as_str();
        if self.duplicate_name(name, decl.name.span) {
            self.poisoned.insert(name.to_string());
            return;
        }
        let ok = self.check_generic_params(&decl.generics);
        self.generics.class_order.push(name.to_string());
        self.generics.classes.insert(
            name.to_string(),
            ClassTemplate {
                decl: Rc::new(decl.clone()),
                methods: decl.methods.iter().map(|m| Rc::new(m.clone())).collect(),
                poisoned: !ok,
                instances: HashMap::new(),
            },
        );
    }

    /// Registers a generic free function and validates its signature.
    pub(crate) fn collect_generic_fn(&mut self, decl: &ast::FnDecl) {
        let name = decl.name.as_str();
        if decl.is_method() {
            self.error(
                Diagnostic::error("`self` is only allowed on a method of a class")
                    .with_label(decl.params[0].span, "not inside a class")
                    .with_help("declare this function inside a `class`"),
            );
            return;
        }
        if name == "main" {
            self.error(
                Diagnostic::error("`main` cannot be generic")
                    .with_label(decl.generics[0].span, "unexpected type parameter")
                    .with_note("the entry point is `fn main():`, see DESIGN.md section 3.3"),
            );
            return;
        }
        if self.duplicate_name(name, decl.name.span) {
            return;
        }
        let before = self.generics.errors;
        let ok = self.check_generic_params(&decl.generics);
        let decl = Rc::new(decl.clone());
        if ok {
            let env = unknown_env(&decl.generics);
            self.with_validation(env, |checker| checker.validate_signature(&decl, false));
        }
        let index = self.generics.templates.len();
        self.generics.templates.push(FnTemplate {
            decl,
            class: None,
            outer: Vec::new(),
            name: name.to_string(),
            symbol: crate::mangle(name),
            poisoned: self.generics.errors != before,
            instances: HashMap::new(),
        });
        self.generics.fns.insert(name.to_string(), index);
    }

    /// Validates the fields and method signatures of every generic class, with
    /// the type parameters standing for "unknown": whatever does not depend on
    /// them is reported once here instead of once per instance.
    pub(crate) fn validate_generic_classes(&mut self) {
        for name in self.generics.class_order.clone() {
            let template = &self.generics.classes[&name];
            if template.poisoned {
                continue;
            }
            let decl = Rc::clone(&template.decl);
            let methods = template.methods.clone();
            let before = self.generics.errors;
            let mut env = unknown_env(&decl.generics);
            // `Self` is the instance, unknown until there is one.
            env.push(("Self".to_string(), TypeId::ERROR));
            self.with_validation(env, |checker| {
                checker.resolve_fields(&decl);
                let mut seen: Vec<&str> = Vec::new();
                for method in &methods {
                    if !method.generics.is_empty() {
                        checker.reject_generic_method(method);
                        continue;
                    }
                    if seen.contains(&method.name.as_str()) {
                        checker.error(
                            Diagnostic::error(format!(
                                "the method `{}` is declared twice on `{name}`",
                                method.name.as_str()
                            ))
                            .with_label(method.name.span, "duplicate method"),
                        );
                        continue;
                    }
                    seen.push(method.name.as_str());
                    checker.validate_signature(method, true);
                }
            });
            if self.generics.errors != before
                && let Some(template) = self.generics.classes.get_mut(&name)
            {
                template.poisoned = true;
            }
        }
    }

    /// Reports a method with type parameters of its own, which M3 does not
    /// have: a method takes the type parameters of its class.
    pub(crate) fn reject_generic_method(&mut self, decl: &ast::FnDecl) {
        self.error(
            Diagnostic::error(format!(
                "the method `{}` cannot have type parameters of its own",
                decl.name.as_str()
            ))
            .with_label(decl.generics[0].span, "type parameter on a method")
            .with_help(
                "make the class generic instead, or write a generic free function \
                 that takes the instance as a parameter",
            )
            .with_note("see DESIGN.md section 3.9"),
        );
    }

    /// Checks a list of type parameters: no duplicates, and no name that would
    /// hide a builtin type.
    fn check_generic_params(&mut self, params: &[ast::GenericParam]) -> bool {
        let mut ok = true;
        for (index, param) in params.iter().enumerate() {
            let name = param.name.as_str();
            if is_builtin_type(name) {
                self.error(
                    Diagnostic::error(format!(
                        "the type parameter `{name}` would hide the builtin type `{name}`"
                    ))
                    .with_label(param.span, "pick another name, e.g. `T`"),
                );
                ok = false;
            } else if params[..index].iter().any(|p| p.name.name == name) {
                self.error(
                    Diagnostic::error(format!("the type parameter `{name}` is declared twice"))
                        .with_label(param.span, "duplicate type parameter"),
                );
                ok = false;
            }
            ok &= self.check_bound_names(param);
        }
        ok
    }

    /// Runs `f` as the declaration-time validation of a template, with `env`
    /// binding its type parameters.
    fn with_validation(&mut self, env: Vec<(String, TypeId)>, f: impl FnOnce(&mut Self)) {
        let saved_env = std::mem::replace(&mut self.generics.env, env);
        let saved_stack = std::mem::take(&mut self.generics.stack);
        let saved_flag = std::mem::replace(&mut self.generics.validating, true);
        f(self);
        self.generics.validating = saved_flag;
        self.generics.stack = saved_stack;
        self.generics.env = saved_env;
    }

    /// The declaration-time checks of a signature that do not depend on the
    /// type arguments (the counterpart of `collect_fn`).
    fn validate_signature(&mut self, decl: &ast::FnDecl, in_class: bool) {
        if in_class && !decl.is_method() {
            let span = decl.params.first().map_or(decl.name.span, |p| p.span);
            self.error(
                Diagnostic::error(format!(
                    "the method `{}` must take `self` as its first parameter",
                    decl.name.as_str()
                ))
                .with_label(span, "expected `self` here")
                .with_note("see DESIGN.md section 3.8"),
            );
            return;
        }
        let mut seen: Vec<&str> = Vec::new();
        let mut first_default: Option<usize> = None;
        for (index, param) in decl.params.iter().enumerate() {
            // `self` stands for an instance that does not exist yet.
            let ty = match &param.ty {
                Some(ty) => self.resolve_type(ty),
                None => TypeId::ERROR,
            };
            if seen.contains(&param.name.as_str()) {
                self.error(
                    Diagnostic::error(format!(
                        "the parameter `{}` is declared twice",
                        param.name.as_str()
                    ))
                    .with_label(param.name.span, "duplicate parameter"),
                );
            }
            seen.push(param.name.as_str());
            match &param.default {
                Some(expr) => {
                    first_default.get_or_insert(index);
                    if let Some(value) = self.eval_const(expr)
                        && !self.assignable(ty, value.ty())
                    {
                        self.mismatch(expr.span, ty, value.ty());
                    }
                }
                None if first_default.is_some() => {
                    self.error(
                        Diagnostic::error(format!(
                            "parameter `{}` has no default but follows one that has",
                            param.name.as_str()
                        ))
                        .with_label(
                            param.name.span,
                            "add a default, or move this parameter earlier",
                        ),
                    );
                }
                None => {}
            }
        }
        if let Some(ret) = &decl.ret {
            self.resolve_type(ret);
        }
    }

    /// Resolves the fields of a class declaration and their constant defaults
    /// under the current type-parameter bindings (the counterpart of
    /// `collect_class_fields`).
    fn resolve_fields(
        &mut self,
        decl: &ast::ClassDecl,
    ) -> (Vec<FieldInfo>, Vec<Option<ConstValue>>) {
        let mut fields: Vec<FieldInfo> = Vec::with_capacity(decl.fields.len());
        let mut defaults: Vec<Option<ConstValue>> = Vec::with_capacity(decl.fields.len());
        for field in &decl.fields {
            let ty = self.resolve_type(&field.ty);
            if ty == TypeId::UNIT {
                self.error(
                    Diagnostic::error(format!(
                        "the field `{}` cannot have type `None`",
                        field.name.as_str()
                    ))
                    .with_label(field.ty.span, "`None` is not a value type"),
                );
            }
            if let Some(previous) = fields.iter().position(|f| f.name == field.name.name) {
                let previous = decl.fields[previous].name.span;
                self.error(
                    Diagnostic::error(format!(
                        "the field `{}` is declared twice",
                        field.name.as_str()
                    ))
                    .with_label(field.name.span, "duplicate field")
                    .with_secondary(previous, "first declared here"),
                );
                continue;
            }
            let default = match &field.default {
                Some(expr) => match self.eval_const(expr) {
                    Some(value) if !self.assignable(ty, value.ty()) => {
                        self.mismatch(expr.span, ty, value.ty());
                        None
                    }
                    other => other,
                },
                None => None,
            };
            fields.push(FieldInfo {
                name: field.name.name.clone(),
                ty,
            });
            defaults.push(default);
        }
        (fields, defaults)
    }

    // -- type resolution ------------------------------------------------------

    /// What the type parameter `name` currently stands for, if one is in scope.
    pub(crate) fn type_param(&self, name: &str) -> Option<TypeId> {
        self.generics
            .env
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, ty)| *ty)
    }

    /// Resolves a use of a type parameter.
    pub(crate) fn resolve_type_param(
        &mut self,
        name: &ast::Ident,
        args: &[TypeId],
        span: Span,
    ) -> TypeId {
        let ty = self.type_param(name.as_str()).unwrap_or(TypeId::ERROR);
        if !args.is_empty() {
            self.error(
                Diagnostic::error(format!(
                    "the type parameter `{}` takes no type arguments",
                    name.as_str()
                ))
                .with_label(span, "unexpected type arguments"),
            );
            return TypeId::ERROR;
        }
        ty
    }

    /// Resolves `Name<args>` for a generic class, instantiating it.
    pub(crate) fn resolve_generic_class(
        &mut self,
        name: &ast::Ident,
        args: &[TypeId],
        span: Span,
    ) -> TypeId {
        let Some(template) = self.generics.classes.get(name.as_str()) else {
            return TypeId::ERROR;
        };
        if template.poisoned {
            return TypeId::ERROR;
        }
        let wanted: Vec<String> = template
            .decl
            .generics
            .iter()
            .map(|g| g.name.name.clone())
            .collect();
        if args.len() != wanted.len() {
            self.error(
                Diagnostic::error(format!(
                    "`{}` takes {} but {} {} supplied",
                    name.as_str(),
                    count(wanted.len(), "type argument"),
                    args.len(),
                    if args.len() == 1 { "was" } else { "were" }
                ))
                .with_label(span, "wrong number of type arguments")
                .with_help(format!(
                    "write `{}<{}>`",
                    name.as_str(),
                    wanted.join(", ")
                )),
            );
            return TypeId::ERROR;
        }
        self.instantiate_class(name.as_str(), args.to_vec(), span)
    }

    // -- instantiation ----------------------------------------------------------

    /// The instance of the generic class `template` for `args`, created on
    /// first use (DESIGN §4.5).
    pub(crate) fn instantiate_class(
        &mut self,
        template: &str,
        args: Vec<TypeId>,
        at: Span,
    ) -> TypeId {
        if args.contains(&TypeId::ERROR) {
            return TypeId::ERROR;
        }
        let Some(entry) = self.generics.classes.get(template) else {
            return TypeId::ERROR;
        };
        if entry.poisoned {
            return TypeId::ERROR;
        }
        if let Some(id) = entry.instances.get(&args).copied() {
            return self.types.class_ty(id);
        }
        let decl = Rc::clone(&entry.decl);
        let what = self.instance_name(template, &args);
        if !self.check_type_args(&what, &args, at) {
            return TypeId::ERROR;
        }
        // DESIGN §3.11: every type argument declares the bounds of its
        // parameter.
        if !self.check_type_bounds(&what, &decl.generics, &args, at) {
            return TypeId::ERROR;
        }
        if !self.within_limits(&what, at) {
            if let Some(entry) = self.generics.classes.get_mut(template) {
                entry.poisoned = true;
            }
            return TypeId::ERROR;
        }

        // Register the instance before resolving its fields, so that a
        // self-referential field (`next: Node<T> | None`) finds it.
        let (id, ty) = self.types.declare_class(&what);
        self.generics.count += 1;
        if let Some(entry) = self.generics.classes.get_mut(template) {
            entry.instances.insert(args.clone(), id);
        }
        self.generics
            .origins
            .insert(id, (template.to_string(), args.clone()));

        let env = bind(&decl.generics, &args);
        let saved_env = std::mem::replace(&mut self.generics.env, env);
        self.generics.stack.push(Frame {
            what: what.clone(),
            at,
        });
        let (fields, defaults) = self.resolve_fields(&decl);
        // The instance's trait conformance is checked once checking bodies
        // is safe, since it may instantiate methods (DESIGN §3.11).
        self.queue_conformance(id, template);
        self.generics.stack.pop();
        self.generics.env = saved_env;

        self.types.set_fields(id, fields);
        // Keyed by the instance name, which is not an identifier, so no
        // source name can ever look it up; the constructor finds its field
        // defaults here.
        self.classes.insert(
            what,
            ClassEntry {
                id,
                ty,
                name_span: decl.name.span,
                defaults,
            },
        );
        ty
    }

    /// The instance of a function template for `args`, created on first use:
    /// its signature is resolved now, its body is queued (DESIGN §4.5).
    fn instantiate_fn(&mut self, index: usize, args: Vec<TypeId>, at: Span) -> Option<hir::FuncId> {
        if args.contains(&TypeId::ERROR) {
            return None;
        }
        let template = &self.generics.templates[index];
        if template.poisoned {
            return None;
        }
        if let Some(id) = template.instances.get(&args) {
            return Some(*id);
        }
        let decl = Rc::clone(&template.decl);
        let class = template.class;
        let mut env = template.outer.clone();
        let (name, symbol) = (template.name.clone(), template.symbol.clone());
        let (what, symbol) = if decl.generics.is_empty() {
            (name, symbol)
        } else {
            let list = self.type_list(&args);
            (format!("{name}<{list}>"), format!("{symbol}<{list}>"))
        };
        if !self.check_type_args(&what, &args, at) {
            return None;
        }
        if !self.check_type_bounds(&what, &decl.generics, &args, at) {
            return None;
        }
        if !self.within_limits(&what, at) {
            self.generics.templates[index].poisoned = true;
            return None;
        }

        env.extend(bind(&decl.generics, &args));
        // Inside a method, `Self` is the class instance (DESIGN §3.11).
        if let Some(class) = class {
            env.push(("Self".to_string(), self.types.class_ty(class)));
        }
        let saved_env = std::mem::replace(&mut self.generics.env, env.clone());
        self.generics.stack.push(Frame {
            what: what.clone(),
            at,
        });
        let sig = self.instance_sig(&decl, class, what, symbol_safe(&symbol));
        let frame = self.generics.stack.pop().expect("pushed above");
        self.generics.env = saved_env;

        let id = hir::FuncId(self.sigs.len() as u32);
        self.sigs.push(sig);
        self.generics.count += 1;
        self.generics.templates[index].instances.insert(args, id);
        let mut stack = self.generics.stack.clone();
        stack.push(frame);
        self.generics.pending.push_back(Pending {
            id,
            decl,
            env,
            stack,
        });
        Some(id)
    }

    /// The signature of one instance, under the current bindings.
    pub(crate) fn instance_sig(
        &mut self,
        decl: &ast::FnDecl,
        class: Option<ClassId>,
        name: String,
        symbol: String,
    ) -> FnSig {
        let mut params = Vec::with_capacity(decl.params.len());
        for param in &decl.params {
            let ty = match (&param.ty, param.is_self, class) {
                (None, true, Some(class)) => self.types.class_ty(class),
                (Some(ty), _, _) => self.resolve_type(ty),
                _ => TypeId::ERROR,
            };
            let default = match &param.default {
                Some(expr) => match self.eval_const(expr) {
                    Some(value) if !self.assignable(ty, value.ty()) => {
                        self.mismatch(expr.span, ty, value.ty());
                        None
                    }
                    other => other,
                },
                None => None,
            };
            params.push(ParamSig {
                name: param.name.name.clone(),
                ty,
                span: param.name.span,
                default,
            });
        }
        let ret = match &decl.ret {
            Some(ty) => self.resolve_type(ty),
            None => TypeId::UNIT,
        };
        FnSig {
            name,
            symbol,
            params,
            ret,
            name_span: decl.name.span,
            self_ty: class.map(|class| self.types.class_ty(class)),
        }
    }

    /// Rejects `None` as a type argument: it has no values to store.
    fn check_type_args(&mut self, what: &str, args: &[TypeId], at: Span) -> bool {
        if !args.contains(&TypeId::UNIT) {
            return true;
        }
        self.error(
            Diagnostic::error(format!("`None` cannot be a type argument, in `{what}`"))
                .with_label(at, "`None` is not a value type")
                .with_note("a type argument must be a type that has values"),
        );
        false
    }

    /// Enforces [`MAX_DEPTH`] and [`MAX_INSTANCES`].
    fn within_limits(&mut self, what: &str, at: Span) -> bool {
        if self.generics.stack.len() < MAX_DEPTH && self.generics.count < MAX_INSTANCES {
            return true;
        }
        let limit = if self.generics.stack.len() >= MAX_DEPTH {
            format!("the depth limit of {MAX_DEPTH} nested instantiations")
        } else {
            format!("the limit of {MAX_INSTANCES} instantiations")
        };
        self.error(
            Diagnostic::error(format!("instantiating `{}` exceeds {limit}", shorten(what)))
                .with_label(at, "instantiated here")
                .with_help(
                    "a generic function that calls itself with ever larger type arguments \
                 (polymorphic recursion) cannot be monomorphized; make the recursive call \
                 use the same type arguments",
                )
                .with_note("generics are compiled by monomorphization, see DESIGN.md section 4.5"),
        );
        false
    }

    /// Checks the bodies of every queued instance and builds every queued
    /// synthesized function, including the ones those request in turn, and
    /// appends them to `functions`. Every signature registered after the
    /// collection pass has exactly one queued body, so after sorting by id
    /// `functions[id]` is the function `id`.
    pub(crate) fn check_instances(&mut self, functions: &mut Vec<hir::Function>) {
        let mut done = self.drain_queues();
        // Instance bodies and synthesized functions are numbered when they
        // are requested, so putting them in id order makes `functions[id]`
        // the function `id` again.
        done.sort_by_key(|(id, _)| *id);
        for (id, function) in done {
            debug_assert_eq!(functions.len(), id.index());
            functions.push(function);
        }
    }

    /// Checks every queued instance body and builds every queued synthesized
    /// function (DESIGN §3.11), including the ones those request in turn, and
    /// runs the pending conformance checks of class instances.
    pub(crate) fn drain_queues(&mut self) -> Vec<(hir::FuncId, hir::Function)> {
        let mut done = Vec::new();
        loop {
            self.run_conformance_queue();
            if let Some(pending) = self.generics.pending.pop_front() {
                let saved_env = std::mem::replace(&mut self.generics.env, pending.env);
                let saved_stack = std::mem::replace(&mut self.generics.stack, pending.stack);
                done.push((pending.id, self.check_function(&pending.decl, pending.id)));
                self.generics.stack = saved_stack;
                self.generics.env = saved_env;
                continue;
            }
            if let Some(job) = self.traits.pending.pop_front() {
                let id = job.id;
                done.push((id, self.build_synth(job)));
                continue;
            }
            if !self.has_queued_conformance() {
                return done;
            }
        }
    }

    /// Adds the instantiation stack to a diagnostic raised inside generic code,
    /// or drops it when the same diagnostic was already reported from another
    /// instance of the same template.
    pub(crate) fn in_instantiation(&mut self, diag: Diagnostic) -> Option<Diagnostic> {
        if diag.is_error() {
            self.generics.errors += 1;
        }
        if !self.generics.validating && self.generics.stack.is_empty() {
            return Some(diag);
        }
        if !self
            .generics
            .seen
            .insert((diag.message.clone(), diag.span()))
        {
            return None;
        }
        // Innermost first, like a backtrace. A deep stack keeps its innermost
        // frames and the outermost ones, where the user's code started it.
        let mut diag = diag;
        let depth = self.generics.stack.len();
        let (inner, outer) = if depth > MAX_NOTES {
            (MAX_NOTES - 2, 2)
        } else {
            (depth, 0)
        };
        for (index, frame) in self.generics.stack.iter().rev().enumerate() {
            if index == inner && outer > 0 {
                let skipped = depth - inner - outer;
                diag = diag.with_note(format!(
                    "... {skipped} more instantiation{} ...",
                    if skipped == 1 { "" } else { "s" }
                ));
            }
            if index >= inner && index < depth - outer {
                continue;
            }
            let what = shorten(&frame.what);
            let labelled = index < MAX_LABELS || index + 1 == depth;
            if labelled && !diag.labels.iter().any(|l| l.span == frame.at) {
                diag = diag.with_secondary(frame.at, format!("`{what}` instantiated here"));
            }
            diag = diag.with_note(format!("in instantiation of `{what}`"));
        }
        Some(diag)
    }

    // -- calls ------------------------------------------------------------------

    /// Whether a call has to go through [`Checker::check_generic_call`]: it
    /// has explicit type arguments, or it names a generic function or class.
    pub(crate) fn is_generic_call(&self, callee: &ast::Expr, explicit: bool) -> bool {
        if explicit {
            return true;
        }
        match &callee.kind {
            ast::ExprKind::Name(name) => {
                !self.scope.contains_key(name.as_str())
                    && (self.generics.has_fn(name.as_str())
                        || self.generics.has_class(name.as_str()))
            }
            _ => false,
        }
    }

    /// A call of a generic function or constructor, or any call with explicit
    /// type arguments (DESIGN §3.9). `expected` is the type the context wants,
    /// which takes part in inference (DESIGN §4.4).
    pub(crate) fn check_generic_call(
        &mut self,
        callee: &ast::Expr,
        type_args: Option<&[ast::TypeExpr]>,
        args: &[ast::Arg],
        expected: Option<TypeId>,
        span: Span,
    ) -> hir::Expr {
        let name = match &callee.kind {
            ast::ExprKind::Name(name) => name,
            ast::ExprKind::Attribute { base, attr } => {
                self.check_expr(base);
                self.check_args_plain(args);
                self.error(
                    Diagnostic::error(format!(
                        "the method `{}` takes no type arguments",
                        attr.as_str()
                    ))
                    .with_label(callee.span, "unexpected type arguments")
                    .with_help("a method uses the type parameters of its class; remove the `<...>`")
                    .with_note("see DESIGN.md section 3.9"),
                );
                return self.error_expr(span);
            }
            _ => {
                self.check_args_plain(args);
                return self.error_expr(span);
            }
        };
        let text = name.as_str();
        if !self.scope.contains_key(text) {
            if self.generics.has_class(text) {
                return self.check_generic_construct(name, type_args, args, expected, span);
            }
            if let Some(index) = self.generics.fns.get(text).copied() {
                return self.check_generic_fn_call(index, name, type_args, args, expected, span);
            }
        }
        // Explicit type arguments on something that is not generic.
        self.check_args_plain(args);
        let known = self.scope.contains_key(text)
            || self.classes.contains_key(text)
            || self.fn_index.contains_key(text)
            || crate::exprs::is_builtin(text);
        if self.poisoned.contains(text) {
            return self.error_expr(span);
        }
        if !known {
            self.error(
                Diagnostic::error(format!("cannot find function `{text}` in this scope"))
                    .with_label(name.span, "not found in this scope"),
            );
            return self.error_expr(span);
        }
        self.error(
            Diagnostic::error(format!("`{text}` is not generic"))
                .with_label(callee.span, "unexpected type arguments")
                .with_help("remove the `<...>`"),
        );
        self.error_expr(span)
    }

    /// `f(args)` or `f<T>(args)` for a generic function `f`.
    fn check_generic_fn_call(
        &mut self,
        index: usize,
        name: &ast::Ident,
        type_args: Option<&[ast::TypeExpr]>,
        args: &[ast::Arg],
        expected: Option<TypeId>,
        span: Span,
    ) -> hir::Expr {
        let template = &self.generics.templates[index];
        let decl = Rc::clone(&template.decl);
        if template.poisoned {
            self.check_args_plain(args);
            return self.error_expr(span);
        }
        let site = Site {
            display: name.as_str(),
            callee_span: name.span,
            span,
            ctor: false,
        };
        let explicit = match type_args {
            Some(written) => match self.explicit_type_args(&site, &decl.generics, written) {
                Some(resolved) => Some(resolved),
                None => {
                    self.check_args_plain(args);
                    return self.error_expr(span);
                }
            },
            None => None,
        };
        let slots: Vec<Slot<'_>> = decl
            .params
            .iter()
            .map(|p| Slot {
                name: p.name.as_str(),
                ty: p.ty.as_ref(),
            })
            .collect();
        let Some((bindings, checked)) = self.infer_call(
            &site,
            &decl.generics,
            &slots,
            decl.ret.as_ref(),
            explicit,
            args,
            expected,
        ) else {
            return self.error_expr(span);
        };
        let Some(func) = self.instantiate_fn(index, bindings, span) else {
            return self.error_expr(span);
        };
        let sig = &self.sigs[func.index()];
        let ret = sig.ret;
        let params: Vec<(String, TypeId, Span, Option<ConstValue>)> = sig
            .params
            .iter()
            .map(|p| (p.name.clone(), p.ty, p.span, p.default.clone()))
            .collect();
        let Some((args, eval_order)) = self.finish_args(&site, &params, checked) else {
            return self.error_expr(span);
        };
        hir::Expr {
            kind: hir::ExprKind::Call {
                func,
                args,
                eval_order,
            },
            ty: ret,
            span,
        }
    }

    /// `C(field=…)` or `C<T>(field=…)` for a generic class `C`.
    fn check_generic_construct(
        &mut self,
        name: &ast::Ident,
        type_args: Option<&[ast::TypeExpr]>,
        args: &[ast::Arg],
        expected: Option<TypeId>,
        span: Span,
    ) -> hir::Expr {
        let template = &self.generics.classes[name.as_str()];
        let decl = Rc::clone(&template.decl);
        if template.poisoned {
            self.check_args_plain(args);
            return self.error_expr(span);
        }
        let site = Site {
            display: name.as_str(),
            callee_span: name.span,
            span,
            ctor: true,
        };
        // Explicit type arguments, or an expected type that is an instance of
        // this class, pick the instance up front; the generated constructor
        // then checks the arguments against its fields as usual.
        let direct = match type_args {
            Some(written) => match self.explicit_type_args(&site, &decl.generics, written) {
                Some(resolved) => Some(resolved),
                None => {
                    self.check_args_plain(args);
                    return self.error_expr(span);
                }
            },
            None => expected.and_then(|ty| self.instance_args_of(name.as_str(), ty)),
        };
        if let Some(targs) = direct {
            let ty = self.instantiate_class(name.as_str(), targs, span);
            return match self.types.get(ty) {
                Type::Class(class) => self.check_construct(class, name.span, args, span),
                _ => {
                    self.check_args_plain(args);
                    self.error_expr(span)
                }
            };
        }

        let slots: Vec<Slot<'_>> = decl
            .fields
            .iter()
            .map(|f| Slot {
                name: f.name.as_str(),
                ty: Some(&f.ty),
            })
            .collect();
        let Some((bindings, checked)) = self.infer_call(
            &site,
            &decl.generics,
            &slots,
            // An expected instance of this class was already used above.
            None,
            None,
            args,
            expected,
        ) else {
            return self.error_expr(span);
        };
        let ty = self.instantiate_class(name.as_str(), bindings, span);
        let Type::Class(class) = self.types.get(ty) else {
            return self.error_expr(span);
        };
        let info = self.types.class(class);
        let instance = info.name.clone();
        let defaults = self
            .classes
            .get(&instance)
            .map(|c| c.defaults.clone())
            .unwrap_or_default();
        let params: Vec<(String, TypeId, Span, Option<ConstValue>)> = info
            .fields
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let span = decl.fields.get(i).map_or(name.span, |d| d.name.span);
                let default = defaults.get(i).cloned().flatten();
                (f.name.clone(), f.ty, span, default)
            })
            .collect();
        let site = Site {
            display: &instance,
            ..site
        };
        let Some((fields, eval_order)) = self.finish_args(&site, &params, checked) else {
            return self.error_expr(span);
        };
        hir::Expr {
            kind: hir::ExprKind::New {
                class,
                fields,
                eval_order,
            },
            ty,
            span,
        }
    }

    /// The method `name` of an instance of a generic class, instantiated on
    /// its first call. `None` when the class has no such method.
    pub(crate) fn instantiate_method(
        &mut self,
        class: ClassId,
        name: &ast::Ident,
    ) -> Option<hir::FuncId> {
        let key = (class, name.name.clone());
        let index = match self.generics.methods.get(&key) {
            Some(index) => *index,
            None => {
                let (template, args) = self.generics.origins.get(&class)?.clone();
                let entry = self.generics.classes.get(&template)?;
                let Some(method) = entry
                    .methods
                    .iter()
                    .find(|m| m.name.name == name.name && m.generics.is_empty())
                else {
                    // `class Box<T>(Eq)` without an `__eq__` derives one per
                    // instance (DESIGN §3.11).
                    return self.derive_for_instance(class, &template, name);
                };
                let method = Rc::clone(method);
                let outer = bind(&entry.decl.generics, &args);
                let class_name = self.types.class(class).name.clone();
                let index = self.generics.templates.len();
                self.generics.templates.push(FnTemplate {
                    decl: method,
                    class: Some(class),
                    outer,
                    name: format!("{class_name}.{}", name.as_str()),
                    symbol: format!("ty_user_{class_name}__{}", name.as_str()),
                    poisoned: false,
                    instances: HashMap::new(),
                });
                self.generics.methods.insert(key.clone(), index);
                index
            }
        };
        let id = self.instantiate_fn(index, Vec::new(), name.span)?;
        self.methods.insert(key, id);
        Some(id)
    }

    /// Whether `name` is a method of the generic class `class` is an instance
    /// of, whether or not it has been instantiated yet.
    pub(crate) fn is_generic_method(&self, class: ClassId, name: &str) -> bool {
        let Some((template, _)) = self.generics.origins.get(&class) else {
            return false;
        };
        self.generics
            .classes
            .get(template)
            .is_some_and(|t| t.methods.iter().any(|m| m.name.name == name))
            || self.template_derives(template, name)
    }

    /// Resolves explicitly written type arguments, checking their number.
    fn explicit_type_args(
        &mut self,
        site: &Site<'_>,
        params: &[ast::GenericParam],
        written: &[ast::TypeExpr],
    ) -> Option<Vec<TypeId>> {
        if written.len() != params.len() {
            let span = written
                .first()
                .zip(written.last())
                .map_or(site.callee_span, |(a, b)| a.span.merge(b.span));
            let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
            self.error(
                Diagnostic::error(format!(
                    "`{}` takes {} but {} {} supplied",
                    site.display,
                    count(params.len(), "type argument"),
                    written.len(),
                    if written.len() == 1 { "was" } else { "were" }
                ))
                .with_label(span, "wrong number of type arguments")
                .with_help(format!(
                    "write `{}<{}>(...)`",
                    site.display,
                    names.join(", ")
                )),
            );
            return None;
        }
        let resolved: Vec<TypeId> = written.iter().map(|t| self.resolve_type(t)).collect();
        if resolved.contains(&TypeId::ERROR) {
            return None;
        }
        Some(resolved)
    }

    /// The type arguments of `ty` when it is (possibly `| None` of) an
    /// instance of the generic class `template`.
    fn instance_args_of(&self, template: &str, ty: TypeId) -> Option<Vec<TypeId>> {
        let class = self.types.class_of(ty)?;
        let (origin, args) = self.generics.origins.get(&class)?;
        (origin == template).then(|| args.clone())
    }

    /// Matches the arguments of a generic call to its slots and infers the
    /// type arguments (DESIGN §4.4): first from the arguments, by first-order
    /// matching of each parameter type against the argument's type, then from
    /// the expected type. Arguments that need a type from their context to
    /// check at all — `[]`, `None`, another generic call — are checked last,
    /// with whatever has been inferred by then. Every argument is checked
    /// exactly once.
    #[allow(clippy::too_many_arguments)]
    fn infer_call(
        &mut self,
        site: &Site<'_>,
        params: &[ast::GenericParam],
        slots: &[Slot<'_>],
        ret: Option<&ast::TypeExpr>,
        explicit: Option<Vec<TypeId>>,
        args: &[ast::Arg],
        expected: Option<TypeId>,
    ) -> Option<Inferred> {
        let generics: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
        let assigned = self.assign_slots(site, slots, args);
        let ok = assigned.iter().all(Option::is_some);

        let mut bindings: Vec<Option<TypeId>> = match explicit {
            Some(resolved) => resolved.into_iter().map(Some).collect(),
            None => vec![None; generics.len()],
        };
        // Which argument fixed each type parameter.
        let mut origin: Vec<Option<Span>> = vec![None; generics.len()];
        let mut checked: Vec<Option<hir::Expr>> = vec![None; args.len()];
        let mut deferred = Vec::new();
        for (index, arg) in args.iter().enumerate() {
            let Some(slot) = assigned[index] else {
                self.check_expr(&arg.value);
                continue;
            };
            let declared = slots[slot].ty;
            let hint = declared.and_then(|te| self.subst(te, &generics, &bindings));
            if hint.is_none() && self.needs_hint(&arg.value) {
                deferred.push(index);
                continue;
            }
            let value = self.check_expr_hint(&arg.value, hint);
            if hint.is_none()
                && let Some(te) = declared
            {
                self.unify_from(te, &value, &generics, &mut bindings, &mut origin);
            }
            checked[index] = Some(value);
        }

        // The expected type fills in what the arguments left open.
        if let (Some(expected), Some(ret)) = (expected, ret)
            && bindings.iter().any(Option::is_none)
        {
            self.unify(ret, expected, &generics, &mut bindings);
        }

        for index in deferred {
            let slot = assigned[index].expect("only assigned arguments are deferred");
            let declared = slots[slot].ty;
            let hint = declared.and_then(|te| self.subst(te, &generics, &bindings));
            let value = self.check_expr_hint(&args[index].value, hint);
            if hint.is_none()
                && let Some(te) = declared
            {
                self.unify_from(te, &value, &generics, &mut bindings, &mut origin);
            }
            checked[index] = Some(value);
        }
        let had_error = checked.iter().flatten().any(|v| v.ty == TypeId::ERROR);

        if !ok {
            return None;
        }
        if let Some(missing) = bindings.iter().position(Option::is_none) {
            if !had_error {
                let param = generics[missing];
                let example: Vec<&str> = generics.iter().map(|_| "int").collect();
                self.error(
                    Diagnostic::error(format!(
                        "cannot infer the type argument `{param}` of `{}`",
                        site.display
                    ))
                    .with_label(site.span, format!("cannot infer `{param}` for this call"))
                    .with_help(format!(
                        "write the type arguments explicitly, e.g. `{}<{}>(...)`",
                        site.display,
                        example.join(", ")
                    ))
                    .with_note(
                        "type arguments are inferred from the arguments and from the \
                         expected type, see DESIGN.md section 4.4",
                    ),
                );
            }
            return None;
        }
        let bindings: Vec<TypeId> = bindings
            .into_iter()
            .map(|b| b.expect("all bound"))
            .collect();
        let checked = assigned
            .into_iter()
            .zip(checked)
            .filter_map(|(slot, value)| {
                let (slot, value) = (slot?, value?);
                let why = slots[slot].ty.and_then(|te| {
                    generics.iter().enumerate().find_map(|(i, g)| {
                        let at = origin[i]?;
                        (at != value.span && mentions(te, g)).then(|| {
                            let ty = self.types.name(bindings[i]);
                            (
                                at,
                                format!("`{g}` is inferred as `{ty}` from this argument"),
                            )
                        })
                    })
                });
                Some(CheckedArg { slot, value, why })
            })
            .collect();
        Some((bindings, checked))
    }

    /// [`Checker::unify`] for one argument, recording it as the origin of
    /// every type parameter it binds.
    fn unify_from(
        &mut self,
        te: &ast::TypeExpr,
        value: &hir::Expr,
        generics: &[&str],
        bindings: &mut [Option<TypeId>],
        origin: &mut [Option<Span>],
    ) {
        let before: Vec<bool> = bindings.iter().map(Option::is_some).collect();
        self.unify(te, value.ty, generics, bindings);
        for (index, was_bound) in before.into_iter().enumerate() {
            if !was_bound && bindings[index].is_some() {
                origin[index] = Some(value.span);
            }
        }
    }

    /// Matches each argument to the slot it fills, reporting surplus, unknown
    /// and repeated arguments. Unmatched arguments are `None`.
    fn assign_slots(
        &mut self,
        site: &Site<'_>,
        slots: &[Slot<'_>],
        args: &[ast::Arg],
    ) -> Vec<Option<usize>> {
        let display = site.display;
        let mut assigned = vec![None; args.len()];
        let mut filled = vec![false; slots.len()];
        let mut next = 0usize;
        for (index, arg) in args.iter().enumerate() {
            match &arg.name {
                None if site.ctor => {
                    self.error(
                        Diagnostic::error(format!(
                            "the constructor of `{display}` only takes keyword arguments"
                        ))
                        .with_label(arg.span, "name the field, e.g. `x=1`")
                        .with_note("the generated constructor is keyword-only, see DESIGN.md 3.8"),
                    );
                }
                None => {
                    if next >= slots.len() {
                        self.error(
                            Diagnostic::error(format!(
                                "`{display}` takes {} but {} {} supplied",
                                count(slots.len(), "argument"),
                                args.len(),
                                if args.len() == 1 { "was" } else { "were" }
                            ))
                            .with_label(arg.span, "unexpected argument")
                            .with_secondary(site.callee_span, format!("`{display}` declared here")),
                        );
                        continue;
                    }
                    assigned[index] = Some(next);
                    filled[next] = true;
                    next += 1;
                }
                Some(name) => match slots.iter().position(|s| s.name == name.as_str()) {
                    None => {
                        let diag = if site.ctor {
                            Diagnostic::error(format!(
                                "`{display}` has no field `{}`",
                                name.as_str()
                            ))
                            .with_label(name.span, "unknown field")
                            .with_secondary(
                                site.callee_span,
                                format!("`{display}` constructed here"),
                            )
                        } else {
                            Diagnostic::error(format!(
                                "`{display}` has no parameter named `{}`",
                                name.as_str()
                            ))
                            .with_label(name.span, "unknown keyword argument")
                            .with_secondary(site.callee_span, format!("`{display}` declared here"))
                        };
                        self.error(diag);
                    }
                    Some(slot) if filled[slot] => {
                        let what = if site.ctor { "field" } else { "argument" };
                        self.error(
                            Diagnostic::error(format!(
                                "{what} `{}` specified twice",
                                name.as_str()
                            ))
                            .with_label(name.span, "already supplied"),
                        );
                    }
                    Some(slot) => {
                        assigned[index] = Some(slot);
                        filled[slot] = true;
                    }
                },
            }
        }
        assigned
    }

    /// Checks already-checked arguments against the parameter types of the
    /// chosen instance, fills in defaults and reports missing arguments.
    /// Returns the arguments in parameter order and their evaluation order.
    fn finish_args(
        &mut self,
        site: &Site<'_>,
        params: &[(String, TypeId, Span, Option<ConstValue>)],
        checked: Vec<CheckedArg>,
    ) -> Option<(Vec<hir::Expr>, Vec<u32>)> {
        let mut slots: Vec<Option<hir::Expr>> = vec![None; params.len()];
        let mut eval_order = Vec::with_capacity(checked.len());
        let mut ok = true;
        for CheckedArg { slot, value, why } in checked {
            let want = params[slot].1;
            if !self.assignable(want, value.ty) {
                match why {
                    Some((at, note)) if want != TypeId::ERROR && value.ty != TypeId::ERROR => {
                        let (e, f) = (self.types.name(want), self.types.name(value.ty));
                        self.error(
                            Diagnostic::error(format!(
                                "mismatched types: expected `{e}`, found `{f}`"
                            ))
                            .with_label(value.span, format!("expected `{e}`, found `{f}`"))
                            .with_secondary(at, note),
                        );
                    }
                    _ => self.mismatch(value.span, want, value.ty),
                }
                ok = false;
                continue;
            }
            slots[slot] = Some(self.coerce(value, want));
            eval_order.push(slot as u32);
        }
        let mut missing = Vec::new();
        for (index, slot) in slots.iter_mut().enumerate() {
            if slot.is_some() {
                continue;
            }
            match params[index].3.clone() {
                Some(default) => {
                    let span = if site.ctor {
                        site.callee_span
                    } else {
                        params[index].2
                    };
                    let expr = default.to_expr(span);
                    *slot = Some(self.coerce(expr, params[index].1));
                }
                None => missing.push(format!("`{}`", params[index].0)),
            }
        }
        if !missing.is_empty() && ok {
            let display = site.display;
            let plural = if missing.len() == 1 { "" } else { "s" };
            let list = missing.join(", ");
            let diag = if site.ctor {
                Diagnostic::error(format!(
                    "missing field{plural} {list} in the constructor of `{display}`"
                ))
                .with_label(site.span, "every field without a default must be supplied")
                .with_help(format!("write `{display}(field=value, …)`"))
            } else {
                Diagnostic::error(format!(
                    "missing argument{plural} {list} in the call to `{display}`"
                ))
                .with_label(site.span, "call is missing arguments")
                .with_secondary(site.callee_span, format!("`{display}` declared here"))
            };
            self.error(diag);
            ok = false;
        }
        if !ok {
            return None;
        }
        Some((
            slots
                .into_iter()
                .map(|s| s.expect("filled above"))
                .collect(),
            eval_order,
        ))
    }

    /// Checks call arguments only for their own diagnostics, when the call
    /// itself has already been rejected.
    fn check_args_plain(&mut self, args: &[ast::Arg]) {
        for arg in args {
            self.check_expr(&arg.value);
        }
    }

    /// Whether an argument can only be checked once its expected type is
    /// known, so inference should look at the other arguments first.
    fn needs_hint(&self, expr: &ast::Expr) -> bool {
        match &expr.kind {
            ast::ExprKind::List(items) => {
                items.is_empty() || items.iter().all(|item| self.needs_hint(item))
            }
            ast::ExprKind::Tuple(items) => items.iter().any(|item| self.needs_hint(item)),
            ast::ExprKind::None => true,
            ast::ExprKind::Call {
                callee,
                type_args: None,
                ..
            } => self.is_generic_call(callee, false),
            _ => false,
        }
    }

    /// The type a parameter type of a template denotes once the type
    /// parameters in `generics` are bound, or `None` when one of them is still
    /// unknown (or the type is one this best-effort hint does not cover). It
    /// reports nothing: it only provides expected types for arguments, and the
    /// instance's real signature is resolved later.
    fn subst(
        &mut self,
        te: &ast::TypeExpr,
        generics: &[&str],
        bindings: &[Option<TypeId>],
    ) -> Option<TypeId> {
        match &te.kind {
            ast::TypeExprKind::None => Some(TypeId::UNIT),
            ast::TypeExprKind::Error => None,
            ast::TypeExprKind::Union(members) => {
                let [a, b] = members.as_slice() else {
                    return None;
                };
                let inner = match (&a.kind, &b.kind) {
                    (ast::TypeExprKind::None, _) => b,
                    (_, ast::TypeExprKind::None) => a,
                    _ => return None,
                };
                let inner = self.subst(inner, generics, bindings)?;
                // Every `T` may be optional (DESIGN §3.10).
                (inner != TypeId::UNIT).then(|| self.types.optional_of(inner))
            }
            ast::TypeExprKind::Named { name, args } => {
                let text = name.as_str();
                if let Some(index) = generics.iter().position(|g| *g == text) {
                    return if args.is_empty() {
                        bindings[index]
                    } else {
                        None
                    };
                }
                match text {
                    "int" | "float" | "bool" | "str" if !args.is_empty() => None,
                    "int" => Some(TypeId::INT),
                    "float" => Some(TypeId::FLOAT),
                    "bool" => Some(TypeId::BOOL),
                    "str" => Some(TypeId::STR),
                    "list" if args.len() == 1 => {
                        let elem = self.subst(&args[0], generics, bindings)?;
                        (elem != TypeId::UNIT).then(|| self.types.list_of(elem))
                    }
                    "tuple" if !args.is_empty() => {
                        let mut members = Vec::with_capacity(args.len());
                        for arg in args {
                            let member = self.subst(arg, generics, bindings)?;
                            if member == TypeId::UNIT {
                                return None;
                            }
                            members.push(member);
                        }
                        Some(self.types.tuple_of(&members))
                    }
                    "dict" if args.len() == 2 => {
                        let key = self.subst(&args[0], generics, bindings)?;
                        let value = self.subst(&args[1], generics, bindings)?;
                        (self.is_key_type(key) && value != TypeId::UNIT)
                            .then(|| self.types.dict_of(key, value))
                    }
                    "set" if args.len() == 1 => {
                        let elem = self.subst(&args[0], generics, bindings)?;
                        self.is_key_type(elem).then(|| self.types.set_of(elem))
                    }
                    _ if self.generics.has_class(text) => {
                        let wanted = self.generics.classes[text].decl.generics.len();
                        if args.len() != wanted {
                            return None;
                        }
                        let mut targs = Vec::with_capacity(args.len());
                        for arg in args {
                            targs.push(self.subst(arg, generics, bindings)?);
                        }
                        // Only a hint: an argument that breaks a bound is
                        // reported where the instance is really requested.
                        let params = self.generics.classes[text].decl.generics.clone();
                        if !self.bounds_hold(&params, &targs) {
                            return None;
                        }
                        let ty = self.instantiate_class(text, targs, te.span);
                        (ty != TypeId::ERROR).then_some(ty)
                    }
                    _ if args.is_empty() => self.classes.get(text).map(|c| c.ty),
                    _ => None,
                }
            }
        }
    }

    /// First-order matching of a parameter type against an argument type:
    /// binds every still-unbound type parameter the structure reaches.
    /// Mismatches are not reported here; the argument is checked against the
    /// instance's parameter type afterwards, which reports them precisely.
    fn unify(
        &mut self,
        te: &ast::TypeExpr,
        ty: TypeId,
        generics: &[&str],
        bindings: &mut [Option<TypeId>],
    ) {
        if ty == TypeId::ERROR {
            return;
        }
        match &te.kind {
            ast::TypeExprKind::Named { name, args } => {
                let text = name.as_str();
                if let Some(index) = generics.iter().position(|g| *g == text) {
                    if args.is_empty() && bindings[index].is_none() {
                        bindings[index] = Some(ty);
                    }
                    return;
                }
                // A `list<int> | None` (an expected type, say) still says what
                // `list<T>` binds, the way the `| None` of a class instance does.
                let shape = self.types.as_optional(ty).unwrap_or(ty);
                match (text, self.types.get(shape)) {
                    ("list", Type::List(elem)) | ("set", Type::Set(elem)) if args.len() == 1 => {
                        self.unify(&args[0], elem, generics, bindings);
                    }
                    ("dict", Type::Dict(key, value)) if args.len() == 2 => {
                        self.unify(&args[0], key, generics, bindings);
                        self.unify(&args[1], value, generics, bindings);
                    }
                    ("tuple", Type::Tuple(id)) => {
                        let members = self.types.tuple_members(id).to_vec();
                        if members.len() == args.len() {
                            for (arg, member) in args.iter().zip(members) {
                                self.unify(arg, member, generics, bindings);
                            }
                        }
                    }
                    // An instance of a generic class, or the `| None` of one.
                    _ => {
                        let Some(class) = self.types.class_of(ty) else {
                            return;
                        };
                        let Some((origin, targs)) = self.generics.origins.get(&class) else {
                            return;
                        };
                        if origin == text && targs.len() == args.len() {
                            let targs = targs.clone();
                            for (arg, targ) in args.iter().zip(targs) {
                                self.unify(arg, targ, generics, bindings);
                            }
                        }
                    }
                }
            }
            // `X | None`: both a `T` and a `T | None` bind `X` to `T`; a bare
            // `None` says nothing about `X`.
            ast::TypeExprKind::Union(members) => {
                if ty == TypeId::UNIT {
                    return;
                }
                let inner = self.types.as_optional(ty).unwrap_or(ty);
                for member in members {
                    if !matches!(member.kind, ast::TypeExprKind::None) {
                        self.unify(member, inner, generics, bindings);
                    }
                }
            }
            ast::TypeExprKind::None | ast::TypeExprKind::Error => {}
        }
    }

    // -- names ------------------------------------------------------------------

    /// `Stack<int, str>` for the template `Stack`.
    fn instance_name(&self, template: &str, args: &[TypeId]) -> String {
        format!("{template}<{}>", self.type_list(args))
    }

    /// `int, list<str>`.
    fn type_list(&self, args: &[TypeId]) -> String {
        args.iter()
            .map(|t| self.types.name(*t))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Whether the type expression `te` mentions the type parameter `name`.
fn mentions(te: &ast::TypeExpr, name: &str) -> bool {
    match &te.kind {
        ast::TypeExprKind::Named { name: n, args } => {
            n.as_str() == name || args.iter().any(|a| mentions(a, name))
        }
        ast::TypeExprKind::Union(members) => members.iter().any(|m| mentions(m, name)),
        ast::TypeExprKind::None | ast::TypeExprKind::Error => false,
    }
}

/// Binds each type parameter to "unknown", for declaration-time validation.
pub(crate) fn unknown_env(params: &[ast::GenericParam]) -> Vec<(String, TypeId)> {
    params
        .iter()
        .map(|p| (p.name.name.clone(), TypeId::ERROR))
        .collect()
}

/// Binds each type parameter to its type argument.
pub(crate) fn bind(params: &[ast::GenericParam], args: &[TypeId]) -> Vec<(String, TypeId)> {
    params
        .iter()
        .map(|p| p.name.name.clone())
        .zip(args.iter().copied())
        .collect()
}

/// `1 type argument`, `2 arguments`.
fn count(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// Whether `name` is a builtin type a type parameter must not hide.
fn is_builtin_type(name: &str) -> bool {
    matches!(
        name,
        "int"
            | "float"
            | "bool"
            | "str"
            | "list"
            | "tuple"
            | "dict"
            | "set"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "f32"
            | "f64"
    )
}

/// Shortens an instance name for a diagnostic, since the names polymorphic
/// recursion produces grow without bound: `f<list<list<…>>>`, with the
/// brackets still balanced.
fn shorten(name: &str) -> String {
    const LIMIT: usize = 48;
    if name.len() <= LIMIT {
        return name.to_string();
    }
    let mut end = LIMIT;
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    // Cut right after a `<` when there is one, rather than inside a name.
    if let Some(open) = name[..end].rfind('<') {
        end = open + 1;
    }
    let head = &name[..end];
    let open = head
        .matches('<')
        .count()
        .saturating_sub(head.matches('>').count());
    format!("{head}…{}", ">".repeat(open))
}

/// Turns an instance name such as `ty_user_max<list<int>>` into a symbol
/// that is a plain LLVM identifier and a valid wasm name.
///
/// Every piece of type punctuation becomes a `.` escape. No identifier may
/// contain a `.`, so the mapping is injective and an instance symbol can never
/// collide with a non-generic one:
///
/// | source | symbol |
/// |---|---|
/// | `<` | `.L` |
/// | `>` | `.G` |
/// | `, ` | `.C` |
/// | ` \| ` | `.U` |
pub(crate) fn symbol_safe(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 8);
    for c in name.chars() {
        match c {
            '<' => out.push_str(".L"),
            '>' => out.push_str(".G"),
            ',' => out.push_str(".C"),
            '|' => out.push_str(".U"),
            ' ' => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_escape_type_punctuation() {
        assert_eq!(symbol_safe("ty_user_max<int>"), "ty_user_max.Lint.G");
        assert_eq!(
            symbol_safe("ty_user_f<list<int>, tuple<str, float>>"),
            "ty_user_f.Llist.Lint.G.Ctuple.Lstr.Cfloat.G.G"
        );
        assert_eq!(
            symbol_safe("ty_user_Stack<Node | None>__push"),
            "ty_user_Stack.LNode.UNone.G__push"
        );
        // Every character is an LLVM identifier character.
        let symbol = symbol_safe("ty_user_g<tuple<int, list<str>>, Node | None>");
        assert!(
            symbol
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.'),
            "{symbol}"
        );
    }

    #[test]
    fn distinct_instances_get_distinct_symbols() {
        let names = [
            "f<list<int>>",
            "f<list<int>, int>",
            "f<tuple<int, int>>",
            "f<tuple<int>, int>",
            "f<Pair<int, int>>",
            "f<Pair<int>, int>",
        ];
        let symbols: HashSet<String> = names.iter().map(|n| symbol_safe(n)).collect();
        assert_eq!(symbols.len(), names.len());
    }

    #[test]
    fn long_names_are_shortened_on_a_char_boundary() {
        let short = "max<int>";
        assert_eq!(shorten(short), short);
        let long = format!("f<{}int{}>", "list<".repeat(30), ">".repeat(30));
        let shortened = shorten(&long);
        assert!(shortened.starts_with("f<list<list<"), "{shortened}");
        assert!(shortened.contains('…'), "{shortened}");
        assert!(shortened.len() < long.len());
        // The brackets stay balanced.
        assert_eq!(
            shortened.matches('<').count(),
            shortened.matches('>').count(),
            "{shortened}"
        );
    }

    #[test]
    fn counts_are_pluralized() {
        assert_eq!(count(1, "type argument"), "1 type argument");
        assert_eq!(count(2, "argument"), "2 arguments");
        assert_eq!(count(0, "argument"), "0 arguments");
    }
}
