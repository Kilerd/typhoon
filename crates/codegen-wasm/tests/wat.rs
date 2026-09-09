//! Backend tests: WAT snapshots of the emitted modules, and a check that every
//! program in the repository lowers to a module a wasm engine accepts.
//!
//! The snapshots are the readable form of the binary this crate produces; they
//! are what makes a change in lowering visible in review. What they cannot
//! check is *behaviour* — that is the golden suite's job, which runs every one
//! of these programs on both backends and compares the output byte for byte
//! (`crates/driver/tests/golden.rs`).

use std::path::{Path, PathBuf};

use typhoon_codegen_wasm::compile_to_wasm;

/// Compiles `src` to WAT, panicking with the rendered diagnostics on failure.
#[track_caller]
fn wat(src: &str) -> String {
    let module =
        compile_to_wasm("main.ty", src).unwrap_or_else(|errors| panic!("{}", errors.join("\n")));
    wasmprinter::print_bytes(&module).expect("the module prints")
}

/// The workspace root.
fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/codegen-wasm always has two ancestors")
}

// ---------------------------------------------------------------------------
// Snapshots
// ---------------------------------------------------------------------------

macro_rules! snapshot_wat {
    ($name:ident, $src:expr) => {
        #[test]
        fn $name() {
            insta::assert_snapshot!(wat($src));
        }
    };
}

snapshot_wat!(
    hello_and_the_entry_point,
    "fn main():\n    print(\"Hello, Typhoon!\")\n"
);

snapshot_wat!(
    recursive_fib,
    "fn fib(n: int) -> int:\n    if n < 2:\n        return n\n    return fib(n - 1) + fib(n - 2)\n\nfn main():\n    print(fib(10))\n"
);

snapshot_wat!(
    integer_floor_division_and_modulo,
    "fn divide(a: int, b: int) -> int:\n    return a // b\n\nfn main():\n    print(divide(-7, 2))\n    print(-7 % 2)\n    print(7 / 2)\n"
);

snapshot_wat!(
    float_arithmetic_has_no_contraction,
    "fn main():\n    a = 1.5\n    b = 2.5\n    print(a * b + a)\n    print(a % b)\n    print(float(a) * b)\n"
);

snapshot_wat!(
    powers_use_sqrt_multiply_and_the_runtime,
    "fn main():\n    x = 2.0\n    n = 3\n    print(x ** 0.5)\n    print(x ** 2.0)\n    print(x ** 3.0)\n    print(n ** 2)\n    print(n ** n)\n"
);

snapshot_wat!(
    for_range_is_a_counting_loop,
    "fn main():\n    total = 0\n    for i in range(10):\n        total = total + i\n    print(total)\n"
);

snapshot_wat!(
    while_with_break_and_continue,
    "fn main():\n    i = 0\n    while i < 10:\n        i = i + 1\n        if i == 3:\n            continue\n        if i > 5:\n            break\n        print(i)\n"
);

snapshot_wat!(
    short_circuit_operators,
    "fn side(n: int) -> bool:\n    print(n)\n    return n > 0\n\nfn main():\n    print(side(1) and side(2))\n"
);

snapshot_wat!(
    f_strings_build_a_string,
    "fn main():\n    n = 2\n    x = 1.5\n    print(f\"n={n} x={x:.2f}\")\n"
);

snapshot_wat!(
    a_list_literal_and_an_index,
    "fn main():\n    xs = [1, 2, 3]\n    xs.append(4)\n    print(xs[-1], len(xs))\n"
);

snapshot_wat!(
    a_class_instance_and_its_fields,
    "class Point:\n    x: int\n    y: int = 0\n\nfn main():\n    p = Point(x=1)\n    p.y = 2\n    print(p.x + p.y)\n"
);

snapshot_wat!(
    tuples_are_scalarised_into_locals,
    "fn pair(n: int) -> tuple<int, bool>:\n    return (n, n > 0)\n\nfn main():\n    a, b = pair(2)\n    print(a, b)\n    print(pair(1) == pair(1))\n"
);

snapshot_wat!(
    builtins_without_a_wasm_instruction,
    "fn main():\n    n = -3\n    x = -1.5\n    print(abs(n), abs(x))\n    print(min(1, 2), max(1.0, 2.0))\n    print(float(n), int(x))\n"
);

// ---------------------------------------------------------------------------
// Every generated module must be a valid wasm module
// ---------------------------------------------------------------------------

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

/// Every `.ty` file of the repository the current milestone is expected to
/// compile.
fn milestone_sources() -> Vec<(String, String)> {
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
            if milestone_of(&source) > 2 {
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
fn every_repository_program_emits_a_valid_module() {
    let sources = milestone_sources();
    assert!(
        sources.len() >= 50,
        "expected the repository to carry at least 50 M0-M2 programs, found {}",
        sources.len()
    );

    let mut problems = Vec::new();
    for (name, source) in &sources {
        match compile_to_wasm(name, source) {
            // `compile_to_wasm` validates before returning, so reaching here
            // means the module is well formed; print it too, because
            // `wasmprinter` walks structures the validator does not.
            Ok(module) => {
                if let Err(err) = wasmprinter::print_bytes(&module) {
                    problems.push(format!("{name} does not print: {err}"));
                }
            }
            Err(errors) => {
                problems.push(format!("{name} did not compile:\n{}", errors.join("\n")));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n\n"));
}

#[test]
fn the_emitted_module_carries_names_for_its_functions() {
    let text = wat("fn helper() -> int:\n    return 1\n\nfn main():\n    print(helper())\n");
    assert!(text.contains("$ty_user_main"), "{text}");
    assert!(text.contains("$ty_user_helper"), "{text}");
    assert!(text.contains("$_start"), "{text}");
    assert!(
        text.contains("(export \"_start\" (func $_start))"),
        "{text}"
    );
}
