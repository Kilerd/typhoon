//! `typhoon-codegen-wasm` — Typhoon source to a binary WebAssembly module.
//!
//! See `docs/DESIGN.md` §6.2.1. This is the second backend: it lowers the same
//! typed HIR `typhoon-codegen` lowers to LLVM IR, but emits a wasm module
//! directly with [`wasm_encoder`] — no LLVM, no text format, no interpreter.
//! It is what runs Typhoon in a browser (the playground's Run button) and what
//! `typhoon build --target wasm` produces.
//!
//! The emitted module is only half a program: it imports the linear memory and
//! the `ty_*` C ABI from a second instance, `crates/runtime` compiled for
//! `wasm32-unknown-unknown`, so both backends share one runtime implementation
//! byte for byte. A loader instantiates the runtime first, then the user
//! module with the runtime's exports, and calls the user module's `_start`.
//!
//! ```
//! let wasm = typhoon_codegen_wasm::compile_to_wasm(
//!     "main.ty",
//!     "fn main():\n    print(\"Hello\")\n",
//! )
//! .unwrap();
//! assert_eq!(&wasm[..4], b"\0asm");
//! ```
//!
//! # Shape of the generated module
//!
//! * one wasm function per user function, in HIR order, plus an exported
//!   `_start` that starts the runtime, materialises the string literals, calls
//!   the user's `main` and exits through `ty_rt_exit`;
//! * every local becomes a wasm local — `int` an `i64`, `float` an `f64`,
//!   `bool` an `i32`, every reference an `i32` address, and a `tuple` one local
//!   per member;
//! * string literals live in passive data segments and are copied into
//!   runtime-allocated blocks by `_start`, because the two modules share one
//!   heap and the user module has no static address space of its own.
//!
//! Every module is validated with [`wasmparser`] before it is returned, so a
//! lowering bug fails at compile time rather than as a trap in the browser.

#![warn(missing_docs)]

mod emit;
mod layout;
mod rt;

pub use emit::START_EXPORT;

use typhoon_diag::{Diagnostics, SourceMap, render};

/// Compiles one Typhoon source file to a binary wasm module.
///
/// `file_name` is used only for diagnostics; `source` is the file's contents.
///
/// # Errors
///
/// Returns the rendered compiler diagnostics, one string per diagnostic, when
/// the program does not compile — or a single `internal error` string if the
/// emitted module fails validation, which is always a bug in this crate.
pub fn compile_to_wasm(file_name: &str, source: &str) -> Result<Vec<u8>, Vec<String>> {
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
        Some(program) if !diags.has_errors() => {
            let wasm = emit::emit_module(&program);
            match validate(&wasm) {
                Ok(()) => Ok(wasm),
                Err(message) => Err(vec![format!(
                    "internal error: the generated wasm module is invalid: {message}\n\
                     note: this is a bug in typhoon-codegen-wasm, please report it"
                )]),
            }
        }
        _ => Err(render_all(&diags, &sources)),
    }
}

/// Validates a wasm module the way an engine would before running it.
///
/// # Errors
///
/// The validator's message, which points at the offending byte offset.
pub fn validate(wasm: &[u8]) -> Result<(), String> {
    wasmparser::Validator::new()
        .validate_all(wasm)
        .map(|_| ())
        .map_err(|err| err.to_string())
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
    fn hello_world_compiles_and_validates() {
        let wasm = compile_to_wasm("hello.ty", "fn main():\n    print(\"hi\")\n").unwrap();
        assert_eq!(&wasm[..4], b"\0asm");
        validate(&wasm).unwrap();
    }

    #[test]
    fn a_type_error_is_rendered() {
        let errors = compile_to_wasm("bad.ty", "fn main():\n    x = 1 + 1.0\n").unwrap_err();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("mismatched types"), "{}", errors[0]);
        assert!(errors[0].contains("bad.ty:2"), "{}", errors[0]);
    }

    #[test]
    fn a_syntax_error_stops_before_sema() {
        let errors = compile_to_wasm("bad.ty", "def main():\n    pass\n").unwrap_err();
        assert!(!errors.is_empty());
        assert!(
            errors.iter().all(|e| !e.contains("cannot find")),
            "{errors:?}"
        );
    }
}
