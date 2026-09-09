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

/// Makes sure `libtyphoon_runtime.a` exists before a test links a binary.
///
/// The runtime is a `staticlib`, so no crate can depend on it and Cargo never
/// builds it for us; the driver's harness does the same thing.
fn ensure_runtime_lib() {
    use std::sync::OnceLock;
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        if std::env::var_os("TYPHOON_RUNTIME_LIB").is_some() {
            return;
        }
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let out = Command::new(cargo)
            .args(["build", "-p", "typhoon-runtime", "--manifest-path"])
            .arg(repo_root().join("Cargo.toml"))
            .output();
        if let Ok(out) = out {
            assert!(
                out.status.success(),
                "`cargo build -p typhoon-runtime` failed:\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    });
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

/// A program that parses but does not type-check.
const BAD_SOURCE: &str = "tests/fail/undefined_name.ty";

#[test]
fn build_reports_diagnostics_on_stderr() {
    let output = temp_output("build-bad");
    let out = typhoon(&["build", BAD_SOURCE, "-o", output.to_str().unwrap()]);
    assert_eq!(code(&out), 1, "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("cannot find value `total`"),
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
fn check_reports_diagnostics_on_stderr() {
    let out = typhoon(&["check", BAD_SOURCE]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("cannot find value `total`"));
    assert!(stdout(&out).is_empty());
}

#[test]
fn emit_ir_prints_no_ir_when_compilation_fails() {
    let out = typhoon(&["emit-ir", BAD_SOURCE]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("cannot find value `total`"));
    assert!(
        stdout(&out).is_empty(),
        "no IR may be printed when compilation fails"
    );
}

#[test]
fn run_reports_diagnostics_on_stderr() {
    let out = typhoon(&["run", BAD_SOURCE]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("cannot find value `total`"));
}

#[test]
fn a_runtime_panic_propagates_its_exit_code() {
    ensure_runtime_lib();
    let out = typhoon(&["run", "tests/run/panic_div_zero.ty"]);
    assert_eq!(code(&out), 101, "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("panic: integer division by zero"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn every_example_is_accepted_or_names_its_milestone() {
    // M0/M1 examples must check; the ones carrying a later milestone must be
    // rejected with a diagnostic that names that milestone, never with a
    // crash or a silent success.
    let examples = std::fs::read_dir(repo_root().join("examples")).unwrap();
    let mut checked = 0;
    for entry in examples.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "ty") {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap();
        let milestone = source
            .lines()
            .find_map(|line| line.trim().strip_prefix("# milestone: M"))
            .and_then(|m| m.trim().parse::<u32>().ok())
            .unwrap_or_else(|| panic!("{} has no `# milestone:` line", path.display()));
        let rel = format!("examples/{}", path.file_name().unwrap().to_string_lossy());
        let out = typhoon(&["check", &rel]);
        if milestone <= 1 {
            assert_eq!(code(&out), 0, "{rel} must check:\n{}", stderr(&out));
        } else {
            assert_eq!(code(&out), 1, "{rel} must be rejected");
            assert!(
                stderr(&out).contains("not supported yet"),
                "{rel} must say what is missing:\n{}",
                stderr(&out)
            );
        }
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
fn build_run_and_emit_ir_work_end_to_end() {
    ensure_runtime_lib();
    let output = temp_output("build-hello");
    let _ = std::fs::remove_file(&output);
    let out = typhoon(&[
        "build",
        "examples/hello.ty",
        "-o",
        output.to_str().unwrap(),
        "--emit-ir",
    ]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(output.is_file(), "the executable was not produced");
    let ir = output.with_extension("ll");
    assert!(ir.is_file(), "--emit-ir must write the module next to it");
    assert!(
        std::fs::read_to_string(&ir)
            .unwrap()
            .contains("@ty_user_main")
    );

    let ran = Command::new(&output).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&ran.stdout), "Hello, Typhoon!\n");

    let out = typhoon(&["run", "examples/hello.ty"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "Hello, Typhoon!\n");

    let out = typhoon(&["emit-ir", "examples/hello.ty"]);
    assert_eq!(code(&out), 0);
    assert!(
        stdout(&out).contains("define i32 @main()"),
        "{}",
        stdout(&out)
    );

    let out = typhoon(&["check", "examples/hello.ty"]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).is_empty(), "check prints nothing on success");

    let _ = std::fs::remove_file(&output);
    let _ = std::fs::remove_file(&ir);
}

#[test]
fn version_exits_zero() {
    let out = typhoon(&["--version"]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).contains(env!("CARGO_PKG_VERSION")));
}
