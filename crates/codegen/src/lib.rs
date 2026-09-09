//! `typhoon-codegen` — Typhoon source to textual LLVM IR.
//!
//! See `docs/DESIGN.md` §6.2: the M0–M4 backend emits **text** LLVM IR
//! (LLVM 18+ syntax, opaque `ptr`) and hands it to the system `clang` for
//! optimisation and linking. There is no `llvm-sys` / `inkwell` dependency.
//!
//! The crate owns the last two steps of the pipeline and nothing else: it runs
//! [`typhoon_parser::parse_module`] and [`typhoon_sema::check_module`], then
//! lowers the resulting typed IR. It never looks at the AST itself
//! (DESIGN §6.1).
//!
//! ```
//! let ir = typhoon_codegen::compile_to_llvm_ir(
//!     "main.ty",
//!     "fn main():\n    print(1 + 2)\n",
//! )
//! .unwrap();
//! assert!(ir.contains("define i32 @main()"));
//! assert!(ir.contains("@ty_user_main"));
//! ```
//!
//! # Shape of the generated module
//!
//! * every user function becomes one `define internal`, its symbol mangled to
//!   `ty_user_<name>` so that it can never collide with a C symbol;
//! * every local becomes one `alloca` in the entry block plus loads and
//!   stores — `clang -O2` runs `mem2reg` and puts them back in registers;
//! * `@main` starts the runtime, calls the user's `main` and exits through
//!   `ty_rt_exit`, which flushes stdout.

#![warn(missing_docs)]

mod emit;

use typhoon_diag::{Diagnostics, SourceMap, render};

/// Compiles one Typhoon source file to textual LLVM IR.
///
/// `file_name` is used only for diagnostics; `source` is the file's contents.
///
/// # Errors
///
/// Returns the rendered compiler diagnostics, one string per diagnostic, when
/// the program does not compile.
pub fn compile_to_llvm_ir(file_name: &str, source: &str) -> Result<String, Vec<String>> {
    let mut sources = SourceMap::new();
    let file = sources.add(file_name, source);
    let mut diags = Diagnostics::new();

    let module = typhoon_parser::parse_module(file, source, &mut diags);
    // A syntax error leaves error nodes all over the tree; type-checking it
    // would only add noise, so the parser's diagnostics are reported alone.
    let program = if diags.has_errors() {
        None
    } else {
        typhoon_sema::check_module(&module, file, &mut diags)
    };

    match program {
        Some(program) if !diags.has_errors() => Ok(emit::emit_module(&program)),
        _ => Err(render_all(&diags, &sources)),
    }
}

/// Renders every diagnostic, one string each.
fn render_all(diags: &Diagnostics, sources: &SourceMap) -> Vec<String> {
    let rendered: Vec<String> = diags
        .iter()
        .map(|diag| render(diag, sources, false))
        .collect();
    if rendered.is_empty() {
        // Should not happen: `check_module` only fails after a diagnostic.
        return vec!["internal error: compilation failed without a diagnostic".to_string()];
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_world_compiles() {
        let ir = compile_to_llvm_ir("hello.ty", "fn main():\n    print(\"hi\")\n").unwrap();
        assert!(ir.contains("define internal void @ty_user_main()"), "{ir}");
        assert!(ir.contains("@ty_print_str"), "{ir}");
        assert!(ir.contains("call void @ty_rt_init()"), "{ir}");
    }

    #[test]
    fn a_type_error_is_rendered() {
        let errors = compile_to_llvm_ir("bad.ty", "fn main():\n    x = 1 + 1.0\n").unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("mismatched types"), "{}", errors[0]);
        assert!(errors[0].contains("bad.ty:2"), "{}", errors[0]);
    }

    #[test]
    fn a_syntax_error_stops_before_sema() {
        let errors = compile_to_llvm_ir("bad.ty", "def main():\n    pass\n").unwrap_err();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("def"), "{}", errors[0]);
    }
}
