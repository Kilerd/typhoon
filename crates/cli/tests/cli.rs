//! Integration tests for the `typhoon` binary.
//!
//! They pin the CLI contract that scripts and CI depend on: the subcommand
//! surface, where diagnostics go, and the exit codes (0 ok, 1 compile/link
//! error, 2 usage error).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The workspace root (`crates/cli/../..`).
fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/cli always has two ancestors")
}

/// Runs the `typhoon` binary built by Cargo for this test.
fn typhoon(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_typhoon"))
        .args(args)
        .current_dir(repo_root())
        .output()
        .expect("failed to run the typhoon binary")
}

fn code(out: &Output) -> i32 {
    out.status
        .code()
        .expect("typhoon should not die from a signal")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A unique path in the temp directory that no test writes to twice.
fn temp_output(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("typhoon-cli-{label}-{}", std::process::id()))
}

// -- usage errors (exit 2) ------------------------------------------------

#[test]
fn no_arguments_is_a_usage_error() {
    let out = typhoon(&[]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("Usage"), "{}", stderr(&out));
}

#[test]
fn unknown_subcommand_is_a_usage_error() {
    let out = typhoon(&["frobnicate"]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("frobnicate"), "{}", stderr(&out));
}

#[test]
fn unknown_flag_is_a_usage_error() {
    let out = typhoon(&["build", "examples/hello.ty", "--not-a-flag"]);
    assert_eq!(code(&out), 2);
}

#[test]
fn optimisation_level_above_three_is_a_usage_error() {
    let out = typhoon(&["build", "examples/hello.ty", "-O", "9"]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("0..=3"), "{}", stderr(&out));
}

#[test]
fn missing_input_file_is_a_usage_error() {
    let out = typhoon(&["build", "definitely-not-here.ty"]);
    assert_eq!(code(&out), 2);
    let err = stderr(&out);
    assert!(err.contains("definitely-not-here.ty"), "{err}");
    assert!(err.contains("cannot read"), "{err}");
}

#[test]
fn missing_input_file_is_a_usage_error_for_every_subcommand() {
    for sub in ["build", "run", "emit-ir", "check"] {
        let out = typhoon(&[sub, "definitely-not-here.ty"]);
        assert_eq!(code(&out), 2, "subcommand `{sub}`: {}", stderr(&out));
    }
}

// -- compile errors (exit 1) ----------------------------------------------

#[test]
fn build_reports_the_stub_frontend_error() {
    let output = temp_output("build");
    let out = typhoon(&["build", "examples/hello.ty", "-o", output.to_str().unwrap()]);
    assert_eq!(code(&out), 1, "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("frontend not implemented yet"),
        "{}",
        stderr(&out)
    );
    assert!(stdout(&out).is_empty(), "diagnostics must not go to stdout");
    assert!(
        !output.exists(),
        "a failed build must not leave an executable"
    );
}

#[test]
fn check_reports_the_stub_frontend_error() {
    let out = typhoon(&["check", "examples/hello.ty"]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("frontend not implemented yet"));
    assert!(stdout(&out).is_empty());
}

#[test]
fn emit_ir_reports_the_stub_frontend_error() {
    let out = typhoon(&["emit-ir", "examples/hello.ty"]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("frontend not implemented yet"));
    assert!(
        stdout(&out).is_empty(),
        "no IR may be printed when compilation fails"
    );
}

#[test]
fn run_reports_the_stub_frontend_error() {
    let out = typhoon(&["run", "examples/hello.ty"]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("frontend not implemented yet"));
}

#[test]
fn every_example_currently_fails_with_the_stub_frontend() {
    // A blunt guard: once the frontend lands this test tells the Phase B
    // executor to update it, rather than letting a silent regression hide.
    let examples = std::fs::read_dir(repo_root().join("examples")).unwrap();
    let mut checked = 0;
    for entry in examples.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "ty") {
            continue;
        }
        let rel = format!("examples/{}", path.file_name().unwrap().to_string_lossy());
        let out = typhoon(&["check", &rel]);
        assert_eq!(code(&out), 1, "{rel}");
        checked += 1;
    }
    assert!(
        checked >= 11,
        "expected at least 11 examples, found {checked}"
    );
}

// -- success paths (exit 0) -----------------------------------------------

#[test]
fn help_exits_zero_and_lists_every_subcommand() {
    let out = typhoon(&["--help"]);
    assert_eq!(code(&out), 0);
    let text = stdout(&out);
    for sub in ["build", "run", "emit-ir", "check"] {
        assert!(text.contains(sub), "`{sub}` missing from --help:\n{text}");
    }
}

#[test]
fn build_help_documents_every_flag() {
    let out = typhoon(&["build", "--help"]);
    assert_eq!(code(&out), 0);
    let text = stdout(&out);
    for flag in ["--output", "--emit-ir", "--keep-temps", "-O"] {
        assert!(
            text.contains(flag),
            "`{flag}` missing from `build --help`:\n{text}"
        );
    }
}

#[test]
fn version_exits_zero() {
    let out = typhoon(&["--version"]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).contains(env!("CARGO_PKG_VERSION")));
}
