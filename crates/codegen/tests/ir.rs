//! Codegen tests: snapshots of the emitted LLVM IR, and a check that every IR
//! module the compiler can produce for the repository's programs is accepted
//! by LLVM itself.

use std::path::{Path, PathBuf};
use std::process::Command;

use typhoon_codegen::compile_to_llvm_ir;

/// Compiles `src`, panicking with the rendered diagnostics on failure.
#[track_caller]
fn ir(src: &str) -> String {
    compile_to_llvm_ir("main.ty", src).unwrap_or_else(|errors| panic!("{}", errors.join("\n")))
}

/// The workspace root.
fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/codegen always has two ancestors")
}

// ---------------------------------------------------------------------------
// Snapshots
// ---------------------------------------------------------------------------

macro_rules! snapshot_ir {
    ($name:ident, $src:expr) => {
        #[test]
        fn $name() {
            insta::assert_snapshot!(ir($src));
        }
    };
}

snapshot_ir!(
    hello_and_the_entry_point,
    "fn main():\n    print(\"Hello, Typhoon!\")\n"
);

snapshot_ir!(
    recursive_fib,
    "fn fib(n: int) -> int:\n    if n < 2:\n        return n\n    return fib(n - 1) + fib(n - 2)\n\nfn main():\n    print(fib(10))\n"
);

snapshot_ir!(
    integer_floor_division_and_modulo,
    "fn divide(a: int, b: int) -> int:\n    return a // b\n\nfn main():\n    print(divide(-7, 2))\n    print(-7 % 2)\n    print(7 / 2)\n"
);

snapshot_ir!(
    float_arithmetic_is_contracted,
    "fn main():\n    a = 1.5\n    b = 2.5\n    print(a * b + a)\n    print(a - b)\n    print(a / b)\n    print(float(a) * b + a)\n"
);

snapshot_ir!(
    float_floor_division_and_modulo,
    "fn main():\n    a = 7.0\n    b = 2.0\n    print(a // b)\n    print(a % b)\n"
);

snapshot_ir!(
    powers_use_sqrt_multiply_and_the_runtime,
    "fn main():\n    x = 2.0\n    n = 3\n    print(x ** 0.5)\n    print(x ** 2.0)\n    print(x ** 3.0)\n    print(n ** 2)\n    print(n ** n)\n"
);

snapshot_ir!(
    short_circuit_operators,
    "fn side(n: int) -> bool:\n    print(n)\n    return n > 0\n\nfn main():\n    print(side(1) and side(2))\n    print(side(0) or side(3))\n"
);

snapshot_ir!(
    for_range_is_a_counting_loop,
    "fn main():\n    total = 0\n    for i in range(10):\n        total = total + i\n    for j in range(10, 0, -2):\n        total = total + j\n    print(total)\n"
);

snapshot_ir!(
    while_with_break_and_continue,
    "fn main():\n    i = 0\n    while i < 10:\n        i = i + 1\n        if i == 3:\n            continue\n        if i > 5:\n            break\n        print(i)\n"
);

snapshot_ir!(
    f_strings_build_a_string,
    "fn main():\n    n = 2\n    x = 1.5\n    print(f\"n={n} x={x:.2f} {{literal}}\")\n"
);

snapshot_ir!(
    string_concatenation_and_equality,
    "fn main():\n    a = \"Hello, \"\n    b = a + \"Typhoon!\"\n    print(b)\n    print(b == \"Hello, Typhoon!\")\n    print(b != a)\n"
);

snapshot_ir!(
    keyword_arguments_and_defaults,
    "fn scale(x: float, factor: float = 2.0) -> float:\n    return x * factor\n\nfn main():\n    print(scale(1.0))\n    print(scale(factor=3.0, x=1.0))\n"
);

snapshot_ir!(
    constants_are_folded,
    "LIMIT: int = 2 * 5\nNAME: str = \"ty\" + \"phoon\"\n\nfn main():\n    for i in range(LIMIT):\n        print(i)\n    print(NAME)\n"
);

snapshot_ir!(
    printing_every_type,
    "fn main():\n    print(1, 1.5, True, \"s\")\n    print()\n"
);

snapshot_ir!(
    bitwise_operations_and_shifts,
    "fn main():\n    n = 5\n    k = 2\n    print(n & 3, n | 8, n ^ 1, ~n)\n    print(n << 2, n >> 1)\n    print(n << k)\n"
);

snapshot_ir!(
    builtins_lower_to_intrinsics,
    "fn main():\n    n = -3\n    x = -1.5\n    print(abs(n), abs(x))\n    print(min(1, 2), max(1.0, 2.0))\n    print(float(n), int(x))\n"
);

snapshot_ir!(
    a_unit_function_returns_void,
    "fn log(n: int):\n    if n < 0:\n        return\n    print(n)\n\nfn main():\n    log(1)\n    log(-1)\n"
);

// ---------------------------------------------------------------------------
// Every generated module must be valid LLVM IR
// ---------------------------------------------------------------------------

/// How to check that a `.ll` file parses: `llvm-as` when it is available,
/// otherwise `clang -c -x ir`, which every machine that can link a Typhoon
/// binary has anyway.
enum Verifier {
    LlvmAs(PathBuf),
    Clang(PathBuf),
}

impl Verifier {
    fn find() -> Verifier {
        let candidates = [
            std::env::var_os("TYPHOON_LLVM_AS").map(PathBuf::from),
            Some(PathBuf::from("/opt/homebrew/opt/llvm/bin/llvm-as")),
            Some(PathBuf::from("/usr/local/opt/llvm/bin/llvm-as")),
        ];
        for candidate in candidates.into_iter().flatten() {
            if candidate.is_file() {
                return Verifier::LlvmAs(candidate);
            }
        }
        if let Ok(out) = Command::new("llvm-as").arg("--version").output()
            && out.status.success()
        {
            return Verifier::LlvmAs(PathBuf::from("llvm-as"));
        }
        let clang = std::env::var_os("TYPHOON_CLANG")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("clang"));
        Verifier::Clang(clang)
    }

    /// Returns the tool's complaints, or `None` when the module is valid.
    fn check(&self, path: &Path) -> Option<String> {
        let out = match self {
            Verifier::LlvmAs(tool) => Command::new(tool)
                .arg(path)
                .arg("-o")
                .arg(devnull())
                .output(),
            Verifier::Clang(tool) => Command::new(tool)
                .args(["-Wno-override-module", "-c", "-x", "ir"])
                .arg(path)
                .arg("-o")
                .arg(devnull())
                .output(),
        };
        let out = out.unwrap_or_else(|err| panic!("cannot run the IR verifier: {err}"));
        if out.status.success() {
            return None;
        }
        Some(format!(
            "{}{}",
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        ))
    }
}

fn devnull() -> PathBuf {
    PathBuf::from(if cfg!(windows) { "NUL" } else { "/dev/null" })
}

/// The milestone a golden file declares, or 0.
fn milestone_of(source: &str) -> u32 {
    for line in source.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("# milestone:") {
            return rest
                .trim()
                .trim_start_matches(['M', 'm'])
                .parse()
                .unwrap_or(0);
        }
    }
    0
}

/// Every `.ty` file of the repository that M1 is expected to compile.
fn m1_sources() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for dir in ["examples", "tests/run"] {
        let path = repo_root().join(dir);
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&path)
            .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()))
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "ty"))
            .collect();
        entries.sort();
        for entry in entries {
            let source = std::fs::read_to_string(&entry).unwrap();
            if milestone_of(&source) > 1 {
                continue;
            }
            out.push((
                format!("{dir}/{}", entry.file_name().unwrap().to_string_lossy()),
                source,
            ));
        }
    }
    out
}

#[test]
fn every_repository_program_compiles_to_valid_llvm_ir() {
    let verifier = Verifier::find();
    let dir = std::env::temp_dir().join(format!("typhoon-ir-check-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let sources = m1_sources();
    assert!(
        sources.len() >= 20,
        "expected the repository to carry at least 20 M0/M1 programs, found {}",
        sources.len()
    );

    let mut problems = Vec::new();
    for (name, source) in &sources {
        let text = match compile_to_llvm_ir(name, source) {
            Ok(text) => text,
            Err(errors) => {
                problems.push(format!("{name} did not compile:\n{}", errors.join("\n")));
                continue;
            }
        };
        let path = dir.join(name.replace('/', "_")).with_extension("ll");
        std::fs::write(&path, &text).unwrap();
        if let Some(complaint) = verifier.check(&path) {
            problems.push(format!("{name} is not valid LLVM IR:\n{complaint}"));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(problems.is_empty(), "{}", problems.join("\n\n"));
}

#[test]
fn the_ir_is_deterministic() {
    let source = "LIMIT: int = 3\n\nfn main():\n    for i in range(LIMIT):\n        print(f\"{i}\", \"x\", 1.0)\n";
    assert_eq!(ir(source), ir(source));
}

#[test]
fn every_declared_runtime_symbol_is_used_and_declared_once() {
    let text = ir(
        "fn main():\n    print(1, \"a\" + \"b\", 2.0, True)\n    print(f\"{1}{2.0}{True}{1:d}{2.0:.2f}\")\n",
    );
    let declarations: Vec<&str> = text.lines().filter(|l| l.starts_with("declare ")).collect();
    let mut symbols: Vec<&str> = declarations
        .iter()
        .map(|line| {
            line.split('@')
                .nth(1)
                .and_then(|rest| rest.split('(').next())
                .expect("a declaration names a symbol")
        })
        .collect();
    assert!(
        symbols.len() > 5,
        "expected several runtime declarations:\n{text}"
    );
    for symbol in &symbols {
        assert!(
            text.contains(&format!("call void @{symbol}("))
                || text.contains(&format!("@{symbol}(i"))
                || text.contains(&format!("@{symbol}(double"))
                || text.contains(&format!("@{symbol}(ptr")),
            "{symbol} is declared but never called:\n{text}"
        );
    }
    let before = symbols.len();
    symbols.sort_unstable();
    symbols.dedup();
    assert_eq!(before, symbols.len(), "a symbol is declared twice:\n{text}");
}

#[test]
fn nothing_is_declared_that_is_not_needed() {
    // `print` of an int must not drag in the string or float runtime.
    let text = ir("fn main():\n    print(1)\n");
    assert!(text.contains("declare void @ty_print_int(i64)"), "{text}");
    assert!(!text.contains("ty_str_concat"), "{text}");
    assert!(!text.contains("ty_print_float"), "{text}");
    assert!(!text.contains("llvm."), "{text}");
}
