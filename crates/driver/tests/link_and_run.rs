//! End-to-end tests for `link_ll` / `run_binary` against hand-written LLVM IR.
//!
//! These are the tests that keep the *backend contract* honest: the symbol
//! names the runtime exports, the `print` output format from DESIGN section
//! 4.6, Boehm GC linkage, and the panic exit code. They do not need the
//! frontend, so they stay green while `typhoon-codegen` is a stub.

mod common;

use std::path::PathBuf;

use typhoon_driver::{BuildOptions, DriverError, compile_source, link_ll, run_binary};

use common::{Scratch, ensure_runtime_lib, fixture};

/// Links a fixture into `scratch` and returns the executable's path.
fn link_fixture(scratch: &Scratch, name: &str, opts: &BuildOptions) -> PathBuf {
    ensure_runtime_lib();
    let stem = name.trim_end_matches(".ll");
    let output = scratch.join(stem);
    link_ll(&fixture(name), &output, opts)
        .unwrap_or_else(|err| panic!("linking {name} failed: {err}"));
    output
}

#[test]
fn runtime_archive_is_discoverable() {
    let lib = ensure_runtime_lib();
    assert!(lib.is_file(), "{} is not a file", lib.display());
    assert_eq!(lib.file_name().unwrap(), "libtyphoon_runtime.a");
}

#[test]
fn hello_prints_exactly_one_line() {
    let scratch = Scratch::new("hello");
    let exe = link_fixture(&scratch, "hello.ll", &BuildOptions::default());
    let out = run_binary(&exe, &[]).unwrap();
    assert_eq!(out.stdout, "Hello, Typhoon!\n");
    assert_eq!(out.stderr, "");
    assert_eq!(out.status, 0);
}

#[test]
fn print_builtins_match_the_design_format() {
    let scratch = Scratch::new("prints");
    let exe = link_fixture(&scratch, "prints.ll", &BuildOptions::default());
    let out = run_binary(&exe, &[]).unwrap();
    assert_eq!(out.stdout, "-42\n5.0\nTrue\nFalse\nNone\n7 ok\n");
    assert_eq!(out.status, 0);
}

#[test]
fn gc_allocation_links_and_runs() {
    let scratch = Scratch::new("alloc");
    let exe = link_fixture(&scratch, "alloc.ll", &BuildOptions::default());
    let out = run_binary(&exe, &[]).unwrap();
    assert_eq!(out.status, 0, "stderr: {}", out.stderr);
    assert_eq!(out.stdout, "200\n");
}

#[test]
fn panic_flushes_stdout_and_exits_101() {
    let scratch = Scratch::new("panic");
    let exe = link_fixture(&scratch, "panic.ll", &BuildOptions::default());
    let out = run_binary(&exe, &[]).unwrap();
    assert_eq!(out.status, 101);
    // Buffered stdout written before the panic must not be lost.
    assert_eq!(out.stdout, "before\n");
    assert!(
        out.stderr.contains("panic: index out of range"),
        "stderr was: {:?}",
        out.stderr
    );
}

#[test]
fn invalid_ir_surfaces_clang_diagnostics() {
    ensure_runtime_lib();
    let scratch = Scratch::new("invalid");
    let err = link_ll(
        &fixture("invalid.ll"),
        &scratch.join("invalid"),
        &BuildOptions::default(),
    )
    .expect_err("broken IR must not link");
    let DriverError::Link(message) = &err else {
        panic!("expected DriverError::Link, got {err:?}");
    };
    assert!(!message.trim().is_empty(), "clang's message was empty");
    assert!(
        message.contains("missing") || message.contains("error"),
        "clang's message did not mention the problem: {message}"
    );
    assert!(err.to_string().contains("linking failed"));
    assert!(
        !scratch.join("invalid").exists(),
        "a broken link must not leave a binary"
    );
}

#[test]
fn every_optimisation_level_links_and_behaves_the_same() {
    let scratch = Scratch::new("optlevels");
    for level in 0..=3u8 {
        let opts = BuildOptions {
            opt_level: level,
            ..BuildOptions::default()
        };
        ensure_runtime_lib();
        let exe = scratch.join(&format!("hello-O{level}"));
        link_ll(&fixture("hello.ll"), &exe, &opts)
            .unwrap_or_else(|err| panic!("-O{level} failed: {err}"));
        let out = run_binary(&exe, &[]).unwrap();
        assert_eq!(out.stdout, "Hello, Typhoon!\n", "-O{level}");
        assert_eq!(out.status, 0, "-O{level}");
    }
}

#[test]
fn opt_level_above_three_is_rejected() {
    let opts = BuildOptions {
        opt_level: 4,
        ..BuildOptions::default()
    };
    let scratch = Scratch::new("badopt");
    let err = link_ll(&fixture("hello.ll"), &scratch.join("x"), &opts).unwrap_err();
    assert!(matches!(err, DriverError::InvalidOptLevel(4)), "{err:?}");
}

#[test]
fn keep_temps_leaves_the_ll_file_next_to_the_binary() {
    let scratch = Scratch::new("keeptemps");
    let opts = BuildOptions {
        keep_temps: true,
        ..BuildOptions::default()
    };
    let exe = link_fixture(&scratch, "hello.ll", &opts);
    let ll = exe.with_extension("ll");
    assert!(ll.is_file(), "{} was not kept", ll.display());
    assert!(
        std::fs::read_to_string(&ll)
            .unwrap()
            .contains("ty_print_str")
    );
}

#[test]
fn without_keep_temps_no_ll_file_is_left_behind() {
    let scratch = Scratch::new("notemps");
    let exe = link_fixture(&scratch, "hello.ll", &BuildOptions::default());
    assert!(!exe.with_extension("ll").exists());
}

#[test]
fn output_directories_are_created_on_demand() {
    let scratch = Scratch::new("nested");
    ensure_runtime_lib();
    let exe = scratch.join("a/b/c/hello");
    link_ll(&fixture("hello.ll"), &exe, &BuildOptions::default()).unwrap();
    assert_eq!(run_binary(&exe, &[]).unwrap().stdout, "Hello, Typhoon!\n");
}

#[test]
fn run_binary_forwards_arguments() {
    // `/bin/echo` stands in for a Typhoon program until argv is wired up.
    let out = run_binary(std::path::Path::new("/bin/echo"), &["a".into(), "b".into()]).unwrap();
    assert_eq!(out.stdout, "a b\n");
    assert_eq!(out.status, 0);
}

#[test]
fn run_binary_reports_a_missing_executable() {
    let err = run_binary(std::path::Path::new("/definitely/not/a/binary"), &[]).unwrap_err();
    assert!(matches!(err, DriverError::Io(_)), "{err:?}");
}

#[test]
fn compile_source_builds_and_runs_a_real_program() {
    ensure_runtime_lib();
    let src = std::fs::read_to_string(common::repo_root().join("examples/hello.ty")).unwrap();
    let artifact = compile_source("hello.ty", &src, &BuildOptions::default()).unwrap();
    let out = run_binary(artifact.binary(), &[]).unwrap();
    assert_eq!(out.stdout, "Hello, Typhoon!\n");
    assert_eq!(out.status, 0);
}

#[test]
fn compile_source_reports_frontend_diagnostics() {
    let err = compile_source(
        "bad.ty",
        "fn main():\n    print(nope)\n",
        &BuildOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(err, DriverError::Compile(_)), "{err:?}");
    assert_eq!(err.diagnostics().len(), 1);
    assert!(
        err.diagnostics()[0].contains("cannot find value `nope`"),
        "{:?}",
        err.diagnostics()
    );
}

#[test]
fn unified_diff_points_at_the_differing_line() {
    let diff = common::unified_diff("a\nb\nc\n", "a\nX\nc\n");
    assert!(diff.contains("-b"), "{diff}");
    assert!(diff.contains("+X"), "{diff}");
}
