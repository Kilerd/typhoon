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
//! inside a browser tab. Linking and running the produced IR still needs the
//! native driver (`typhoon run file.ty`); in-browser execution is not part of
//! this stage.
//!
//! ```
//! let out = typhoon_playground::compile_playground("fn main():\n    print(1)\n");
//! assert!(out.ok);
//! assert!(out.llvm_ir.unwrap().contains("define i32 @main()"));
//! ```

#![warn(missing_docs)]

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
}
