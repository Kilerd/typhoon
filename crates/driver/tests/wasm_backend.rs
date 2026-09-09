//! End-to-end tests for the WebAssembly backend (DESIGN §6.2.1).
//!
//! Every case compiles a Typhoon program with `typhoon-codegen-wasm`, then runs
//! it under `wasmtime` with the same two host imports the browser loader
//! provides. The golden suite does this for every file in `tests/run` and
//! `examples`; this file covers the plumbing itself — the entry point, the
//! shared memory, panics, exit codes — so that a failure here points at the
//! backend rather than at one program.

mod common;

use common::wasm::{TRAP_EXIT_CODE, compile_and_run, is_available};

/// Compiles and runs `source`, asserting it exits cleanly, and returns stdout.
fn output(source: &str) -> String {
    let result = compile_and_run("test.ty", source).expect("the program runs");
    assert_eq!(
        result.status, 0,
        "unexpected exit\nstdout: {}\nstderr: {}",
        result.stdout, result.stderr
    );
    result.stdout
}

#[test]
fn hello_world() {
    if !is_available() {
        return;
    }
    assert_eq!(
        output("fn main():\n    print(\"Hello, Typhoon!\")\n"),
        "Hello, Typhoon!\n"
    );
}

#[test]
fn arithmetic_and_control_flow() {
    if !is_available() {
        return;
    }
    let source = "\
fn fib(n: int) -> int:
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

fn main():
    total = 0
    for i in range(10):
        total = total + fib(i)
    print(total)
    print(7 // 2, -7 // 2, 7 % 3, -7 % 3)
    print(2 ** 10, 2.0 ** 0.5)
    print(1 / 4, 3.5 % 2.0)
";
    assert_eq!(
        output(source),
        "88\n3 -4 1 2\n1024 1.4142135623730951\n0.25 1.5\n"
    );
}

#[test]
fn strings_lists_and_tuples() {
    if !is_available() {
        return;
    }
    let source = "\
fn main():
    xs = [1, 2, 3]
    xs.append(4)
    print(xs, len(xs), xs[-1])
    words = \"a,b,c\".split(\",\")
    print(words, \",\".join(words))
    pair = (1, \"two\")
    print(pair, pair[1])
    print(f\"{len(words)} words, pi is {3.14159:.2f}\")
";
    assert_eq!(
        output(source),
        "[1, 2, 3, 4] 4 4\n[\"a\", \"b\", \"c\"] a,b,c\n(1, \"two\") two\n3 words, pi is 3.14\n"
    );
}

#[test]
fn a_panic_exits_with_101_and_writes_to_stderr() {
    if !is_available() {
        return;
    }
    let result = compile_and_run(
        "boom.ty",
        "fn main():\n    print(\"before\")\n    xs = [1]\n    print(xs[5])\n",
    )
    .expect("the program runs");
    assert_eq!(result.status, 101);
    // Everything printed before the panic is still flushed.
    assert_eq!(result.stdout, "before\n");
    assert!(
        result.stderr.contains("panic: list index out of range"),
        "{}",
        result.stderr
    );
}

#[test]
fn division_by_zero_panics() {
    if !is_available() {
        return;
    }
    let result = compile_and_run("div.ty", "fn main():\n    d = 0\n    print(1 // d)\n")
        .expect("the program runs");
    assert_eq!(result.status, 101);
    assert!(
        result.stderr.contains("panic: integer division by zero"),
        "{}",
        result.stderr
    );
    assert_ne!(result.status, TRAP_EXIT_CODE, "a panic must not be a trap");
}

#[test]
fn output_is_line_buffered_across_a_long_run() {
    if !is_available() {
        return;
    }
    // More than the runtime's 8 KiB buffer, so the host sees several writes and
    // has to stitch them together.
    let source = "\
fn main():
    for i in range(2000):
        print(i, \"台风\")
";
    let result = compile_and_run("many.ty", source).expect("the program runs");
    assert_eq!(result.status, 0);
    let lines: Vec<&str> = result.stdout.lines().collect();
    assert_eq!(lines.len(), 2000);
    assert_eq!(lines[0], "0 台风");
    assert_eq!(lines[1999], "1999 台风");
}

#[test]
fn without_a_collector_an_allocation_loop_panics_instead_of_trapping() {
    if !is_available() {
        return;
    }
    // v1 of the wasm backend never frees (DESIGN §6.2.1). Given a 32 MiB
    // budget, a loop that allocates a few hundred megabytes must therefore run
    // out — and the interesting part is *how*: `ty_alloc` sees a null block and
    // reports `panic: out of memory` with exit code 101, the same as any other
    // Typhoon panic, rather than trapping somewhere inside the allocator.
    let source = "\
fn main():
    last = \"\"
    for i in range(2000000):
        last = f\"{i}-{i}\"
    print(last)
";
    let module = typhoon_codegen_wasm::compile_to_wasm("hog.ty", source).expect("it compiles");
    let result = common::wasm::run_wasm_with_limit(&module, Some(32 * 1024 * 1024))
        .expect("the program runs");
    assert_eq!(
        result.status, 101,
        "stdout: {}\nstderr: {}",
        result.stdout, result.stderr
    );
    assert!(
        result.stderr.contains("panic: out of memory"),
        "{}",
        result.stderr
    );
}

#[test]
fn deep_recursion_traps_instead_of_running_away() {
    if !is_available() {
        return;
    }
    // The wasm stack is far smaller than a native one (wasmtime and every
    // browser cap it), so a recursion the native backend handles happily can
    // exhaust it. What matters is that it ends as a reported trap with the
    // output written so far, not as a silent failure — the JS loader turns the
    // engine's error into the same thing.
    let source = "\
fn down(n: int) -> int:
    if n <= 0:
        return 0
    return 1 + down(n - 1)

fn main():
    print(down(1000))
    print(down(10000000))
";
    let result = compile_and_run("deep.ty", source).expect("the program runs");
    assert_eq!(result.status, TRAP_EXIT_CODE, "stderr: {}", result.stderr);
    assert_eq!(result.stdout, "1000\n");
    assert!(result.stderr.starts_with("trap: "), "{}", result.stderr);
}
