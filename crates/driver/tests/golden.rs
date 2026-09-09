//! The Typhoon golden test harness (DESIGN section 7.1).
//!
//! Every `.ty` file under `tests/run/`, `tests/fail/` and `examples/` becomes
//! its own named test case, so `cargo test` reports `run/while_sum`,
//! `examples/fib`, `fail/undefined_name`, … individually.
//!
//! # File directives
//!
//! Directives are whole-line comments anywhere in the file:
//!
//! | directive | meaning |
//! |---|---|
//! | `# expect: <line>` | one expected line of stdout, in order |
//! | `# error: <substring>` | a substring the rendered diagnostics must contain |
//! | `# milestone: M0` | the milestone this file starts being enforced at |
//!
//! `tests/run/` and `examples/` files must compile, run with exit code 0 and
//! produce exactly the `# expect:` lines. `tests/fail/` files must fail to
//! compile with diagnostics containing every `# error:` substring.
//!
//! # Milestones
//!
//! Files above [`CURRENT_MILESTONE`] are reported as *ignored* rather than
//! failed, so the suite can carry the test cases for future milestones from
//! day one. Set `TYPHOON_MILESTONE=M1` to raise the bar without editing this
//! file.

mod common;

use std::path::{Path, PathBuf};

use libtest_mimic::{Arguments, Failed, Trial};
use typhoon_driver::{BuildOptions, DriverError, compile_source, compile_to_ir, run_binary};

use common::{Scratch, repo_root, unified_diff};

/// The milestone whose test cases are currently enforced (DESIGN section 8).
///
/// Raise this as milestones land; anything above it is reported as ignored.
const CURRENT_MILESTONE: &str = "M0";

/// What a `.ty` file asserts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Must compile, run and print the `# expect:` lines.
    Run,
    /// Must fail to compile with diagnostics matching the `# error:` lines.
    Fail,
}

/// One collected `.ty` file.
#[derive(Debug)]
struct Case {
    name: String,
    path: PathBuf,
    kind: Kind,
    milestone: u32,
    expect: Vec<String>,
    errors: Vec<String>,
}

/// Parses `M<n>` into its ordinal. Anything else is treated as M0.
fn milestone_rank(text: &str) -> u32 {
    text.trim()
        .trim_start_matches(['M', 'm'])
        .parse()
        .unwrap_or(0)
}

/// The milestone the suite enforces, honouring the `TYPHOON_MILESTONE` override.
fn current_milestone() -> u32 {
    std::env::var("TYPHOON_MILESTONE")
        .ok()
        .filter(|v| !v.is_empty())
        .map_or_else(|| milestone_rank(CURRENT_MILESTONE), |v| milestone_rank(&v))
}

/// Extracts the value of a whole-line `# <key>: <value>` directive.
fn directive<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let line = line.trim();
    let rest = line.strip_prefix('#')?.trim_start();
    let rest = rest.strip_prefix(key)?;
    let rest = rest.strip_prefix(':')?;
    // A single leading space is the separator, everything after it is payload.
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

/// Reads all directives out of a source file.
fn parse_directives(source: &str) -> (u32, Option<u32>, Vec<String>, Vec<String>) {
    let mut declared = None;
    let mut expect = Vec::new();
    let mut errors = Vec::new();
    for line in source.lines() {
        if let Some(v) = directive(line, "milestone") {
            declared = Some(milestone_rank(v));
        } else if let Some(v) = directive(line, "expect") {
            expect.push(v.to_string());
        } else if let Some(v) = directive(line, "error") {
            errors.push(v.to_string());
        }
    }
    (declared.unwrap_or(0), declared, expect, errors)
}

/// Collects every `.ty` file in `dir`, naming the cases `<prefix>/<stem>`.
fn collect_dir(dir: &Path, prefix: &str, kind: Kind, into: &mut Vec<Case>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "ty"))
        .collect();
    paths.sort();

    for path in paths {
        let source = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(err) => panic!("cannot read {}: {err}", path.display()),
        };
        let (milestone, _declared, expect, errors) = parse_directives(&source);
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        into.push(Case {
            name: format!("{prefix}/{stem}"),
            path,
            kind,
            milestone,
            expect,
            errors,
        });
    }
}

/// Discovers every golden test case in the repository.
fn collect_cases() -> Vec<Case> {
    let root = repo_root();
    let mut cases = Vec::new();
    collect_dir(&root.join("tests/run"), "run", Kind::Run, &mut cases);
    collect_dir(&root.join("examples"), "examples", Kind::Run, &mut cases);
    collect_dir(&root.join("tests/fail"), "fail", Kind::Fail, &mut cases);
    cases
}

/// Runs one success case: compile, execute, compare stdout byte for byte.
fn run_case(case: &Case) -> Result<(), Failed> {
    let source = std::fs::read_to_string(&case.path)?;
    let file_name = case
        .path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    let scratch = Scratch::new("golden");
    let opts = BuildOptions {
        output: Some(scratch.join(case.path.file_stem().unwrap().to_string_lossy().as_ref())),
        ..BuildOptions::default()
    };
    let artifact = compile_source(&file_name, &source, &opts)
        .map_err(|err| Failed::from(format!("{} did not compile:\n{err}", case.name)))?;

    let output = run_binary(artifact.binary(), &[])?;

    let expected: String = case.expect.iter().map(|line| format!("{line}\n")).collect();
    if output.stdout != expected {
        return Err(Failed::from(format!(
            "{}: stdout does not match the `# expect:` lines\n{}",
            case.name,
            unified_diff(&expected, &output.stdout)
        )));
    }
    if output.status != 0 {
        return Err(Failed::from(format!(
            "{} exited with {} (expected 0)\nstderr:\n{}",
            case.name, output.status, output.stderr
        )));
    }
    Ok(())
}

/// Runs one failure case: compilation must fail with matching diagnostics.
fn fail_case(case: &Case) -> Result<(), Failed> {
    let source = std::fs::read_to_string(&case.path)?;
    let file_name = case
        .path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    match compile_to_ir(&file_name, &source) {
        Ok(_) => Err(Failed::from(format!(
            "{} compiled successfully, but it must be rejected",
            case.name
        ))),
        Err(DriverError::Compile(diagnostics)) => {
            let rendered = diagnostics.join("\n");
            let missing: Vec<&String> = case
                .errors
                .iter()
                .filter(|e| !rendered.contains(e.as_str()))
                .collect();
            if missing.is_empty() {
                Ok(())
            } else {
                Err(Failed::from(format!(
                    "{}: diagnostics are missing {:?}\nrendered diagnostics:\n{rendered}",
                    case.name, missing
                )))
            }
        }
        Err(other) => Err(Failed::from(format!(
            "{}: expected a compile error, got {other}",
            case.name
        ))),
    }
}

/// A meta-test asserting that every golden file is well formed.
///
/// Without this, deleting a `# expect:` line would silently weaken the suite.
fn manifest_check() -> Result<(), Failed> {
    let root = repo_root();
    let mut problems = Vec::new();

    for (dir, prefix, kind) in [
        (root.join("tests/run"), "run", Kind::Run),
        (root.join("examples"), "examples", Kind::Run),
        (root.join("tests/fail"), "fail", Kind::Fail),
    ] {
        if !dir.is_dir() {
            problems.push(format!("{} does not exist", dir.display()));
            continue;
        }
        let mut cases = Vec::new();
        collect_dir(&dir, prefix, kind, &mut cases);
        if cases.is_empty() {
            problems.push(format!("{} contains no .ty files", dir.display()));
        }
        for case in cases {
            let source = std::fs::read_to_string(&case.path)?;
            let (_, declared, _, _) = parse_directives(&source);
            if declared.is_none() {
                problems.push(format!("{} has no `# milestone:` line", case.name));
            }
            match case.kind {
                Kind::Run if case.expect.is_empty() => {
                    problems.push(format!("{} has no `# expect:` lines", case.name));
                }
                Kind::Fail if case.errors.is_empty() => {
                    problems.push(format!("{} has no `# error:` lines", case.name));
                }
                _ => {}
            }
        }
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(Failed::from(problems.join("\n")))
    }
}

/// A meta-test for the directive parser itself.
fn directive_parser_check() -> Result<(), Failed> {
    let source = "\
# A comment.
# milestone: M2
# expect: 1
# expect:
# error: mismatched types

fn main():
    print(1)        # expect: not a directive, this is a trailing comment
";
    let (milestone, declared, expect, errors) = parse_directives(source);
    let mut problems = Vec::new();
    if milestone != 2 || declared != Some(2) {
        problems.push(format!(
            "milestone parsed as {milestone:?}/{declared:?}, want 2"
        ));
    }
    if expect != ["1", ""] {
        problems.push(format!("expect parsed as {expect:?}, want [\"1\", \"\"]"));
    }
    if errors != ["mismatched types"] {
        problems.push(format!("errors parsed as {errors:?}"));
    }
    if milestone_rank("M10") != 10 || milestone_rank("M0") != 0 {
        problems.push("milestone_rank is wrong".into());
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(Failed::from(problems.join("\n")))
    }
}

fn main() {
    let args = Arguments::from_args();
    let current = current_milestone();

    // Build the runtime archive once, before any trial thread needs it.
    common::ensure_runtime_lib();

    let mut trials = vec![
        Trial::test("harness/manifest", manifest_check).with_kind("golden"),
        Trial::test("harness/directives", directive_parser_check).with_kind("golden"),
    ];

    for case in collect_cases() {
        let ignored = case.milestone > current;
        let kind = case.kind;
        let name = case.name.clone();
        trials.push(
            Trial::test(name, move || match kind {
                Kind::Run => run_case(&case),
                Kind::Fail => fail_case(&case),
            })
            .with_kind("golden")
            .with_ignored_flag(ignored),
        );
    }

    libtest_mimic::run(&args, trials).exit();
}
