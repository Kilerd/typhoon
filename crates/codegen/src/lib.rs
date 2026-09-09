//! `typhoon-codegen` — Typhoon source to textual LLVM IR.
//!
//! See `docs/DESIGN.md` §6.2: the M0–M4 backend emits **text** LLVM IR
//! (LLVM 18+ syntax, opaque `ptr`) and hands it to the system `clang` for
//! optimisation and linking. There is no `llvm-sys` / `inkwell` dependency.
//!
//! # Status
//!
//! This crate is a stub. [`compile_to_llvm_ir`] currently always fails; the
//! frontend (lexer → parser → sema → IR lowering → codegen) is being built in
//! parallel and will replace this file wholesale. Everything downstream
//! (`typhoon-driver`, the `typhoon` CLI, the golden test harness) is already
//! wired up against this signature, so the only change needed is the body.

/// Compiles one Typhoon source file to textual LLVM IR.
///
/// `file_name` is used only for diagnostics; `source` is the file's contents.
///
/// # Errors
///
/// Returns the rendered compiler diagnostics, one string per diagnostic, when
/// the program does not compile.
pub fn compile_to_llvm_ir(file_name: &str, source: &str) -> Result<String, Vec<String>> {
    let _ = (file_name, source);
    Err(vec!["frontend not implemented yet".into()])
}
