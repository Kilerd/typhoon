//! `typhoon-playground` — the typhoon compiler frontend, compiled to
//! WebAssembly for the web playground in `web/`.
//!
//! The crate is a thin shell around the real compiler crates: it runs the
//! pipeline of DESIGN §6.3 (`lexer` → `parser` → `sema` → `codegen`) over a
//! string and collects everything the playground wants to display — rendered
//! diagnostics, the token dump, the AST dump, the typed HIR dump, the textual
//! LLVM IR and per-phase timings — into one [`PlaygroundOutput`].
//!
//! Nothing here shells out or touches the filesystem, so the whole thing runs
//! inside a browser tab. Turning the LLVM IR into a binary still needs the
//! native driver (`typhoon run file.ty`), but the page does not have to wait
//! for it to run a program: [`compile_wasm_playground`] goes down the second
//! backend of DESIGN §6.2.1 instead and hands the page a wasm module it can
//! instantiate itself.
//!
//! ```
//! let out = typhoon_playground::compile_playground("fn main():\n    print(1)\n");
//! assert!(out.ok);
//! assert!(out.llvm_ir.unwrap().contains("define i32 @main()"));
//! ```

#![warn(missing_docs)]

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};
use typhoon_diag::{Diagnostics, SourceMap, render};
use wasm_bindgen::prelude::*;
use web_time::Instant;

/// The name the playground's buffer is registered under in the source map; it
/// is what the `--> here:line:col` line of every diagnostic shows.
const FILE_NAME: &str = "playground.ty";

/// How long each phase of the pipeline took, in milliseconds.
///
/// The numbers are wall-clock and measured separately per phase, so they
/// overlap a little: `parse` includes lexing (the parser drives the lexer
/// itself) and `codegen` currently includes a re-parse and a re-check, because
/// [`typhoon_codegen`] only exposes a source-to-IR entry point.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Timings {
    /// Tokenizing the source for the token dump.
    pub lex: f64,
    /// Parsing (lexes again internally).
    pub parse: f64,
    /// Name resolution, type checking and lowering to the typed HIR.
    pub sema: f64,
    /// Emitting textual LLVM IR.
    pub codegen: f64,
}

/// Everything the playground displays for one compilation.
///
/// Serialised to JSON by [`compile`]; the field names are the keys the page's
/// `main.js` reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaygroundOutput {
    /// Whether the program compiled: no errors, and LLVM IR was produced.
    pub ok: bool,
    /// Every diagnostic, rendered as plain text (never ANSI-coloured).
    pub diagnostics: Vec<String>,
    /// The token dump ([`typhoon_lexer::dump_tokens`]); always produced.
    pub tokens: String,
    /// The AST dump ([`typhoon_ast::dump`]); always produced, because the
    /// parser recovers from syntax errors and still returns a module.
    pub ast: String,
    /// The typed HIR dump, or `None` when the program did not type-check.
    pub hir: Option<String>,
    /// The textual LLVM IR, or `None` when the program did not compile.
    pub llvm_ir: Option<String>,
    /// Per-phase wall-clock timings.
    pub timings_ms: Timings,
}

/// Runs the whole frontend over `source` and collects what the playground
/// shows.
///
/// Never fails and never panics: a program full of errors comes back with
/// `ok: false`, the diagnostics that explain it, and whatever dumps could
/// still be produced. Phases are skipped the same way the real compiler skips
/// them — type checking a tree with syntax errors would only add noise, so
/// `hir` is `None` as soon as the parser reported an error.
pub fn compile_playground(source: &str) -> PlaygroundOutput {
    let mut sources = SourceMap::new();
    let file = sources.add(FILE_NAME, source);

    // The parser lexes the file again itself, so this pass exists only to
    // produce the token dump; its diagnostics would be exact duplicates of the
    // ones `parse_module` reports and go into a sink that is thrown away.
    let mut lexer_diags = Diagnostics::new();
    let started = Instant::now();
    let tokens = typhoon_lexer::tokenize(file, source, &mut lexer_diags);
    let lex = millis(started);
    let tokens = typhoon_lexer::dump_tokens(&tokens);

    let mut diags = Diagnostics::new();
    let started = Instant::now();
    let module = typhoon_parser::parse_module(file, source, &mut diags);
    let parse = millis(started);
    let ast = typhoon_ast::dump(&module);

    let mut sema = 0.0;
    let hir = if diags.has_errors() {
        None
    } else {
        let started = Instant::now();
        let program = typhoon_sema::check_module(&module, file, &mut diags);
        sema = millis(started);
        program.as_ref().map(typhoon_sema::dump_program)
    };

    let mut codegen = 0.0;
    let mut llvm_ir = None;
    if hir.is_some() && !diags.has_errors() {
        let started = Instant::now();
        // `typhoon_codegen` only takes source, so this re-runs the parser and
        // the checker; see `Timings`.
        let emitted = typhoon_codegen::compile_to_llvm_ir(FILE_NAME, source);
        codegen = millis(started);
        match emitted {
            Ok(ir) => llvm_ir = Some(ir),
            // Unreachable unless codegen disagrees with the run above; the
            // diagnostics are already rendered, so keep them as they are.
            Err(rendered) => {
                return PlaygroundOutput {
                    ok: false,
                    diagnostics: rendered,
                    tokens,
                    ast,
                    hir,
                    llvm_ir: None,
                    timings_ms: Timings {
                        lex,
                        parse,
                        sema,
                        codegen,
                    },
                };
            }
        }
    }

    PlaygroundOutput {
        ok: llvm_ir.is_some(),
        diagnostics: diags
            .iter()
            .map(|diag| render(diag, &sources, false))
            .collect(),
        tokens,
        ast,
        hir,
        llvm_ir,
        timings_ms: Timings {
            lex,
            parse,
            sema,
            codegen,
        },
    }
}

/// Milliseconds elapsed since `started`, as a float.
fn millis(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

/// Compiles `source` and returns [`PlaygroundOutput`] as a JSON string.
///
/// A string rather than a structured value keeps the JS side free of
/// generated glue for every field, and the payload (LLVM IR, dumps) is text
/// anyway.
#[wasm_bindgen]
pub fn compile(source: &str) -> String {
    let output = compile_playground(source);
    serde_json::to_string(&output).unwrap_or_else(|err| {
        // `PlaygroundOutput` is plain strings and floats, so this cannot
        // happen; report it as a diagnostic rather than panicking.
        format!(
            "{{\"ok\":false,\"diagnostics\":[\"internal error: {}\"],\
             \"tokens\":\"\",\"ast\":\"\",\"hir\":null,\"llvm_ir\":null,\
             \"timings_ms\":{{\"lex\":0,\"parse\":0,\"sema\":0,\"codegen\":0}}}}",
            err.to_string().replace('"', "'")
        )
    })
}

/// The result of compiling one program for in-browser execution.
///
/// Serialised to JSON by [`compile_wasm`]; the field names are the keys the
/// page's `worker.js` and `main.js` read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WasmOutput {
    /// Whether a module was produced.
    pub ok: bool,
    /// Every diagnostic, rendered as plain text (never ANSI-coloured); empty
    /// when the program compiled.
    pub diagnostics: Vec<String>,
    /// The module as standard base64 (with padding), or `None` when the
    /// program did not compile.
    ///
    /// Base64 rather than an array of numbers because the payload crosses into
    /// JS as JSON, which has no byte string: a `Uint8Array` would become tens
    /// of thousands of decimal literals, several times the size of the module
    /// it describes.
    pub wasm: Option<String>,
    /// Wall-clock milliseconds spent in the backend.
    pub compile_ms: f64,
}

/// Compiles `source` to a WebAssembly module the page can run itself.
///
/// This is the whole pipeline again, not a continuation of
/// [`compile_playground`]: [`typhoon_codegen_wasm::compile_to_wasm`] only
/// exposes a source-to-module entry point, so a page that shows both the LLVM
/// IR and runs the program parses it twice. That is cheap next to the round
/// trip through a worker, and it keeps the two backends independent — the wasm
/// module the user runs is exactly what `typhoon build --target wasm` would
/// produce for the same file.
///
/// Never fails and never panics; a program that does not compile comes back
/// with `ok: false`, `wasm: None` and the diagnostics that explain it.
pub fn compile_wasm_playground(source: &str) -> WasmOutput {
    let started = Instant::now();
    let compiled = typhoon_codegen_wasm::compile_to_wasm(FILE_NAME, source);
    let compile_ms = millis(started);

    match compiled {
        Ok(module) => WasmOutput {
            ok: true,
            diagnostics: Vec::new(),
            wasm: Some(BASE64.encode(module)),
            compile_ms,
        },
        // `compile_to_wasm` renders its diagnostics itself, so there is no
        // source map to consult here.
        Err(rendered) => WasmOutput {
            ok: false,
            diagnostics: rendered,
            wasm: None,
            compile_ms,
        },
    }
}

/// Compiles `source` for execution and returns [`WasmOutput`] as a JSON
/// string.
///
/// The counterpart of [`compile`]: a string rather than a structured value, so
/// that the JS side needs no generated glue per field.
#[wasm_bindgen]
pub fn compile_wasm(source: &str) -> String {
    let output = compile_wasm_playground(source);
    serde_json::to_string(&output).unwrap_or_else(|err| {
        // `WasmOutput` is plain strings and a float, so this cannot happen;
        // report it as a diagnostic rather than panicking.
        format!(
            "{{\"ok\":false,\"diagnostics\":[\"internal error: {}\"],\
             \"wasm\":null,\"compile_ms\":0}}",
            err.to_string().replace('"', "'")
        )
    })
}

/// Routes Rust panics to `console.error` with a readable stack, so that a
/// compiler bug shows up in the browser console instead of as an opaque
/// `unreachable` trap.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::path::PathBuf;

    /// The repository's `examples/` directory.
    fn examples_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples")
    }

    /// All diagnostics of one run, joined, for substring assertions.
    fn all_diagnostics(output: &PlaygroundOutput) -> String {
        output.diagnostics.join("\n")
    }

    #[test]
    fn hello_world_compiles() {
        let out = compile_playground("fn main():\n    print(\"Hello, Typhoon!\")\n");
        assert!(out.ok, "diagnostics: {}", all_diagnostics(&out));
        assert!(out.diagnostics.is_empty());
        let ir = out.llvm_ir.expect("hello world produces IR");
        assert!(ir.contains("ty_print_str"), "IR was:\n{ir}");
        assert!(out.hir.expect("hello world type-checks").contains("main"));
        assert!(out.tokens.contains("Fn @0..2"));
        assert!(out.ast.starts_with("Module"));
    }

    #[test]
    fn syntax_error_still_dumps_tokens_and_ast() {
        let out = compile_playground("def main():\n    print(1)\n");
        assert!(!out.ok);
        assert!(
            all_diagnostics(&out).contains("`fn`"),
            "expected a hint about `fn`, got: {}",
            all_diagnostics(&out)
        );
        assert!(!out.tokens.is_empty(), "tokens are dumped even on error");
        assert!(
            !out.ast.is_empty(),
            "the parser recovers and returns a tree"
        );
        // Type checking is skipped once the parser reported an error.
        assert!(out.hir.is_none());
        assert!(out.llvm_ir.is_none());
    }

    #[test]
    fn type_error_is_reported() {
        let out = compile_playground("fn main():\n    x: str = 1\n");
        assert!(!out.ok);
        assert!(
            all_diagnostics(&out).contains("mismatched types"),
            "got: {}",
            all_diagnostics(&out)
        );
        // The tree parsed, so both dumps are there and only the IR is missing.
        assert!(!out.ast.is_empty());
        assert!(out.llvm_ir.is_none());
    }

    #[test]
    fn current_milestone_examples_compile() {
        let mut checked = 0;
        for entry in fs::read_dir(examples_dir()).expect("examples/ is readable") {
            let path = entry.expect("readable entry").path();
            if path.extension().is_none_or(|ext| ext != "ty") {
                continue;
            }
            let source = fs::read_to_string(&path).expect("readable example");
            // Each example declares the milestone it belongs to (DESIGN 7.1);
            // later ones are expected to be rejected with a diagnostic.
            if !source.contains("# milestone: M0") && !source.contains("# milestone: M1") {
                continue;
            }
            let out = compile_playground(&source);
            assert!(
                out.ok,
                "{} should compile, got:\n{}",
                path.display(),
                all_diagnostics(&out)
            );
            assert!(out.llvm_ir.is_some());
            checked += 1;
        }
        assert!(
            checked > 0,
            "found no M0/M1 examples in {:?}",
            examples_dir()
        );
    }

    #[test]
    fn hostile_input_never_panics() {
        // The playground compiles on every keystroke, so it sees half-written
        // and plainly broken programs constantly: each must come back as an
        // output, never as a panic (which on wasm is an unrecoverable trap).
        let sources = [
            "",
            "\n\n\n",
            "fn",
            "fn main(",
            "fn main():",
            "fn main():\n\tprint(1)\n", // tabs are rejected by the lexer
            "fn main():\n    print(\"unclosed\n", // unterminated string
            "fn main():\n    print(999999999999999999999999)\n",
            "fn main():\n    print(0x)\n",
            "fn main():\n        print(1)\n  print(2)\n", // inconsistent indent
            "class C:\n    x: int\n",
            "fn main():\n    print(f\"{1 +}\")\n",
            "🌀 = 1\n",
            "fn main():\n    return\n\nfn main():\n    return\n", // duplicate name
        ];
        for source in sources {
            let out = compile_playground(source);
            // Whatever the verdict, the two invariants the page relies on hold.
            assert_eq!(
                out.ok,
                out.llvm_ir.is_some(),
                "`ok` and `llvm_ir` disagree for {source:?}"
            );
            assert!(
                out.ok || !out.diagnostics.is_empty(),
                "{source:?} failed without a diagnostic"
            );
            assert!(!out.ast.is_empty(), "no AST dump for {source:?}");
        }
    }

    #[test]
    fn later_milestone_features_report_the_milestone() {
        // Constructs the parser accepts but the current subset does not
        // implement are rejected with a diagnostic naming the milestone they
        // arrive in, rather than a panic (README, "Status"). Skipped once the
        // feature lands, so that this test does not fight the compiler's own
        // progress.
        let out = compile_playground("fn main():\n    xs = {1: 2}\n    print(xs)\n");
        if !out.ok {
            assert!(
                all_diagnostics(&out).contains('M'),
                "expected a milestone in: {}",
                all_diagnostics(&out)
            );
        }
    }

    #[test]
    fn output_round_trips_through_json() {
        let out = compile_playground("fn main():\n    print(1 + 2)\n");
        let json = compile("fn main():\n    print(1 + 2)\n");
        let parsed: PlaygroundOutput = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(parsed.ok, out.ok);
        assert_eq!(parsed.llvm_ir, out.llvm_ir);
        assert_eq!(parsed.tokens, out.tokens);
        assert_eq!(parsed.ast, out.ast);
        assert_eq!(parsed.hir, out.hir);
        assert_eq!(parsed.diagnostics, out.diagnostics);

        let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        for key in [
            "ok",
            "diagnostics",
            "tokens",
            "ast",
            "hir",
            "llvm_ir",
            "timings_ms",
        ] {
            assert!(value.get(key).is_some(), "missing key `{key}`");
        }
        assert!(value["timings_ms"]["codegen"].is_number());
    }

    #[test]
    fn empty_source_is_an_error_not_a_panic() {
        let out = compile_playground("");
        assert!(!out.ok);
        assert!(all_diagnostics(&out).contains("main"));
    }

    /// The bytes of a [`WasmOutput`], decoded from its base64 field.
    fn wasm_bytes(output: &WasmOutput) -> Vec<u8> {
        let encoded = output.wasm.as_ref().expect("a module was produced");
        BASE64.decode(encoded).expect("valid standard base64")
    }

    #[test]
    fn hello_world_compiles_to_a_wasm_module() {
        let out = compile_wasm_playground("fn main():\n    print(\"Hello, Typhoon!\")\n");
        assert!(out.ok, "diagnostics: {}", out.diagnostics.join("\n"));
        assert!(out.diagnostics.is_empty());
        // The magic number is all this crate checks: `compile_to_wasm`
        // validates the module itself before returning it.
        assert_eq!(&wasm_bytes(&out)[..4], b"\0asm");
    }

    #[test]
    fn type_error_produces_no_wasm() {
        let out = compile_wasm_playground("fn main():\n    x: str = 1\n");
        assert!(!out.ok);
        assert!(out.wasm.is_none());
        assert!(
            out.diagnostics.join("\n").contains("mismatched types"),
            "got: {:?}",
            out.diagnostics
        );
    }

    #[test]
    fn wasm_output_round_trips_through_json() {
        let source = "fn main():\n    print(1 + 2)\n";
        let json = compile_wasm(source);
        let parsed: WasmOutput = serde_json::from_str(&json).expect("valid JSON");
        assert!(parsed.ok);
        assert_eq!(
            wasm_bytes(&parsed),
            wasm_bytes(&compile_wasm_playground(source))
        );

        // The page reads these four keys and nothing else.
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        for key in ["ok", "diagnostics", "wasm", "compile_ms"] {
            assert!(value.get(key).is_some(), "missing key `{key}`");
        }
        assert!(value["compile_ms"].is_number());
        assert!(value["wasm"].is_string());

        let broken = compile_wasm("fn main():\n    x: str = 1\n");
        let value: serde_json::Value = serde_json::from_str(&broken).expect("valid JSON");
        assert_eq!(value["ok"], serde_json::Value::Bool(false));
        assert!(value["wasm"].is_null());
        assert!(
            !value["diagnostics"]
                .as_array()
                .expect("an array")
                .is_empty()
        );
    }

    #[test]
    fn hostile_input_never_panics_in_the_wasm_backend() {
        // Same contract as `hostile_input_never_panics`: the Run button
        // compiles whatever is in the editor, so every one of these must come
        // back as an output rather than as a trap.
        let sources = [
            "",
            "fn main(",
            "fn main():\n\tprint(1)\n",
            "fn main():\n    print(\"unclosed\n",
            "class C:\n    x: int\n",
            "🌀 = 1\n",
        ];
        for source in sources {
            let out = compile_wasm_playground(source);
            assert_eq!(
                out.ok,
                out.wasm.is_some(),
                "`ok` and `wasm` disagree for {source:?}"
            );
            assert!(
                out.ok || !out.diagnostics.is_empty(),
                "{source:?} failed without a diagnostic"
            );
        }
    }
}
