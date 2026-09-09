//! `typhoon-sema` — name resolution, type checking and lowering to a typed IR.
//!
//! [`check_module`] takes the AST the parser produced and returns a typed
//! [`hir::Program`] — or `None` when the program does not type-check, in which
//! case every problem has been pushed into the [`Diagnostics`] sink
//! (DESIGN §6.1, §6.3).
//!
//! ```
//! use typhoon_diag::{Diagnostics, SourceMap};
//!
//! let src = "fn main():\n    print(1 + 2)\n";
//! let mut sources = SourceMap::new();
//! let file = sources.add("main.ty", src);
//! let mut diags = Diagnostics::new();
//! let module = typhoon_parser::parse_module(file, src, &mut diags);
//!
//! let program = typhoon_sema::check_module(&module, file, &mut diags).unwrap();
//! assert_eq!(program.function(program.main).symbol, "ty_user_main");
//! ```
//!
//! # What the checker enforces (M0/M1 subset)
//!
//! * every name resolves, and every local is definitely assigned on every path
//!   that reads it (DESIGN §3.4);
//! * a local's type is fixed by its first assignment and never changes;
//! * there are no implicit conversions and no truthiness (DESIGN §3.6, §4.3);
//! * a function with a return type returns on every path;
//! * features that parse but belong to a later milestone (classes, lists,
//!   dicts, generics, `is`, `in`, indexing, attributes, comparison chains)
//!   are rejected with one diagnostic each.

#![warn(missing_docs)]

mod check;
mod consts;
mod dump;
mod exprs;
pub mod hir;
pub mod types;

use typhoon_ast as ast;
use typhoon_diag::{Diagnostic, Diagnostics, FileId, Span};

use crate::check::{Checker, ConstEntry, FnSig, ParamSig};
use crate::types::TypeId;

pub use dump::dump_program;

/// Type-checks a parsed module and lowers it to the typed IR.
///
/// Returns `None` when the module contains errors; in that case the
/// diagnostics describing them have been pushed into `diags`. `file` must be
/// the [`FileId`] the module was parsed from, so that synthesized diagnostics
/// point at the right file.
pub fn check_module(
    module: &ast::Module,
    file: FileId,
    diags: &mut Diagnostics,
) -> Option<hir::Program> {
    let errors_before = diags.iter().filter(|d| d.is_error()).count();
    let mut checker = Checker::new(diags);
    // Diagnostics about the module as a whole point at the start of the file.
    let whole_file = Span::new(file, 0, 0);

    let fn_decls = checker.collect_items(module);
    let mut functions = Vec::with_capacity(fn_decls.len());
    for (id, decl) in fn_decls.iter().enumerate() {
        functions.push(checker.check_function(decl, hir::FuncId(id as u32)));
    }
    let main = checker.check_entry_point(&fn_decls, whole_file);

    let types = checker.types.clone();
    let errors_after = diags.iter().filter(|d| d.is_error()).count();
    if errors_after > errors_before {
        return None;
    }
    Some(hir::Program {
        types,
        functions,
        main: main?,
    })
}

impl<'a> Checker<'a> {
    /// First pass: collect constants and function signatures, so that
    /// functions may be called before they are declared.
    fn collect_items<'m>(&mut self, module: &'m ast::Module) -> Vec<&'m ast::FnDecl> {
        let mut decls = Vec::new();
        for item in &module.items {
            match item {
                ast::Item::Class(decl) => {
                    self.unsupported(decl.span, "the `class` declaration", "M2");
                }
                ast::Item::Const(decl) => self.collect_const(decl),
                ast::Item::Fn(decl) => {
                    if self.collect_fn(decl, hir::FuncId(decls.len() as u32)) {
                        decls.push(decl);
                    }
                }
            }
        }
        decls
    }

    fn collect_const(&mut self, decl: &ast::ConstDecl) {
        let declared = self.resolve_type(&decl.ty);
        let Some(value) = self.eval_const(&decl.value) else {
            self.poisoned.insert(decl.name.name.clone());
            return;
        };
        if declared != TypeId::ERROR && value.ty() != declared {
            self.mismatch(decl.value.span, declared, value.ty());
            self.poisoned.insert(decl.name.name.clone());
            return;
        }
        if declared == TypeId::ERROR {
            self.poisoned.insert(decl.name.name.clone());
            return;
        }
        if self.duplicate_name(decl.name.as_str(), decl.name.span) {
            return;
        }
        self.consts.insert(
            decl.name.name.clone(),
            ConstEntry {
                value,
                span: decl.name.span,
            },
        );
    }

    /// Records the signature of `decl`, returning whether its body should be
    /// checked.
    fn collect_fn(&mut self, decl: &ast::FnDecl, id: hir::FuncId) -> bool {
        if !decl.generics.is_empty() {
            let span = decl.generics[0].span;
            self.unsupported(span, "a generic function", "M3");
            return false;
        }
        if decl.is_method() {
            self.error(
                Diagnostic::error("`self` is only allowed on a method of a class")
                    .with_label(decl.params[0].span, "not inside a class")
                    .with_note("classes arrive in M2, see DESIGN.md section 3.8"),
            );
            return false;
        }
        if self.duplicate_name(decl.name.as_str(), decl.name.span) {
            return false;
        }
        self.poisoned.remove(decl.name.as_str());

        let mut params = Vec::with_capacity(decl.params.len());
        let mut seen: Vec<&str> = Vec::new();
        for param in &decl.params {
            let ty = match &param.ty {
                Some(ty) => self.resolve_type(ty),
                // The parser already reported the missing annotation.
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
            let default = match &param.default {
                Some(expr) => {
                    let value = self.eval_const(expr);
                    match value {
                        Some(value) if ty != TypeId::ERROR && value.ty() != ty => {
                            self.mismatch(expr.span, ty, value.ty());
                            None
                        }
                        other => other,
                    }
                }
                None => None,
            };
            params.push(ParamSig {
                name: param.name.name.clone(),
                ty,
                span: param.name.span,
                default,
            });
        }
        // A parameter without a default may not follow one with a default.
        if let Some(first_default) = params.iter().position(|p| p.default.is_some())
            && let Some(bad) = params
                .iter()
                .skip(first_default)
                .find(|p| p.default.is_none())
        {
            let span = bad.span;
            let name = bad.name.clone();
            self.error(
                Diagnostic::error(format!(
                    "parameter `{name}` has no default but follows one that has"
                ))
                .with_label(span, "add a default, or move this parameter earlier"),
            );
        }

        let ret = match &decl.ret {
            Some(ty) => self.resolve_type(ty),
            None => TypeId::UNIT,
        };
        self.sigs.push(FnSig {
            name: decl.name.name.clone(),
            params,
            ret,
            name_span: decl.name.span,
        });
        self.fn_index.insert(decl.name.name.clone(), id);
        true
    }

    /// Reports a name that is already taken by a function or a constant.
    fn duplicate_name(&mut self, name: &str, span: Span) -> bool {
        let previous = self
            .fn_index
            .get(name)
            .map(|id| self.sigs[id.index()].name_span)
            .or_else(|| self.consts.get(name).map(|c| c.span));
        match previous {
            Some(previous) => {
                self.error(
                    Diagnostic::error(format!("the name `{name}` is defined multiple times"))
                        .with_label(span, format!("`{name}` redefined here"))
                        .with_secondary(previous, "previous definition here"),
                );
                true
            }
            None => false,
        }
    }

    /// Second pass: check one function body.
    fn check_function(&mut self, decl: &ast::FnDecl, id: hir::FuncId) -> hir::Function {
        self.locals.clear();
        self.scope.clear();
        self.assigned.clear();
        self.loop_depth = 0;
        self.ret_ty = self.sigs[id.index()].ret;
        self.fn_name = self.sigs[id.index()].name.clone();

        let params: Vec<(String, TypeId, Span)> = self.sigs[id.index()]
            .params
            .iter()
            .map(|p| (p.name.clone(), p.ty, p.span))
            .collect();
        let param_ids = params
            .iter()
            .map(|(name, ty, span)| self.declare_local(name, *ty, *span, true))
            .collect();

        let (body, flow) = self.check_block(&decl.body);
        if self.ret_ty != TypeId::UNIT
            && self.ret_ty != TypeId::ERROR
            && flow != check::Flow::Diverges
        {
            let ret_name = self.types.name(self.ret_ty);
            let fn_name = self.fn_name.clone();
            self.error(
                Diagnostic::error(format!(
                    "missing return in function `{fn_name}` returning `{ret_name}`"
                ))
                .with_label(
                    decl.name.span,
                    "this function can reach its end without returning a value",
                )
                .with_help(format!("add a `return <{ret_name}>` on every path")),
            );
        }

        hir::Function {
            name: self.fn_name.clone(),
            symbol: mangle(&self.fn_name),
            params: param_ids,
            ret: self.ret_ty,
            locals: std::mem::take(&mut self.locals),
            body,
            span: decl.span,
        }
    }

    /// Checks the `fn main():` entry point (DESIGN §3.3).
    fn check_entry_point(
        &mut self,
        decls: &[&ast::FnDecl],
        whole_file: Span,
    ) -> Option<hir::FuncId> {
        let Some(id) = self.fn_index.get("main").copied() else {
            self.error(
                Diagnostic::error("this program has no `main` function")
                    .with_label(whole_file, "this file declares no entry point")
                    .with_help("add `fn main():` as the entry point")
                    .with_note("every program needs an entry point, see DESIGN.md section 3.3"),
            );
            return None;
        };
        let decl = decls[id.index()];
        if let Some(param) = decl.params.first() {
            self.error(
                Diagnostic::error("`main` must not take parameters")
                    .with_label(param.span, "unexpected parameter"),
            );
        }
        if let Some(ret) = &decl.ret
            && self.resolve_type(ret) != TypeId::UNIT
        {
            self.error(
                Diagnostic::error("`main` must not have a return type")
                    .with_label(ret.span, "unexpected return type")
                    .with_help("the process exit code is not the value of `main`"),
            );
        }
        Some(id)
    }
}

/// Mangles a user function name into its symbol (`ty_user_<name>`), so that no
/// user function can collide with a C symbol such as `main` or `write`.
pub fn mangle(name: &str) -> String {
    format!("ty_user_{name}")
}
