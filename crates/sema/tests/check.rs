//! Type-checker tests: what the checker accepts, what it rejects, and the
//! exact wording of the diagnostics it produces.

use typhoon_diag::{Diagnostics, SourceMap, render_all};
use typhoon_sema::{check_module, dump_program};

/// Type-checks `src`, returning the dumped IR on success and the rendered
/// diagnostics on failure.
fn check(src: &str) -> Result<String, String> {
    let mut sources = SourceMap::new();
    let file = sources.add("main.ty", src);
    let mut diags = Diagnostics::new();
    let module = typhoon_parser::parse_module(file, src, &mut diags);
    assert!(
        !diags.has_errors(),
        "the test program must parse:\n{}",
        render_all(&diags, &sources, false)
    );
    match check_module(&module, file, &mut diags) {
        Some(program) => Ok(dump_program(&program)),
        None => Err(render_all(&diags, &sources, false)),
    }
}

/// Type-checks `src`, which must succeed.
fn ok(src: &str) -> String {
    check(src).unwrap_or_else(|err| panic!("expected the program to check:\n{err}"))
}

/// Type-checks `src`, which must fail, and returns the rendered diagnostics.
fn err(src: &str) -> String {
    check(src).expect_err("expected the program to be rejected")
}

/// Asserts that the diagnostics of `src` contain every `wanted` substring.
#[track_caller]
fn rejects(src: &str, wanted: &[&str]) -> String {
    let rendered = err(src);
    for want in wanted {
        assert!(
            rendered.contains(want),
            "diagnostics do not mention {want:?}:\n{rendered}"
        );
    }
    rendered
}

/// Wraps `body` in a `main` function, indenting it.
fn in_main(body: &str) -> String {
    let mut src = String::from("fn main():\n");
    for line in body.lines() {
        src.push_str("    ");
        src.push_str(line);
        src.push('\n');
    }
    src
}

#[track_caller]
fn ok_main(body: &str) -> String {
    ok(&in_main(body))
}

#[track_caller]
fn rejects_main(body: &str, wanted: &[&str]) -> String {
    rejects(&in_main(body), wanted)
}

// ---------------------------------------------------------------------------
// Types of expressions
// ---------------------------------------------------------------------------

#[test]
fn literals_have_the_expected_types() {
    let dump = ok_main("a = 1\nb = 1.5\nc = True\nd = \"s\"");
    assert!(dump.contains("local a: int"), "{dump}");
    assert!(dump.contains("local b: float"), "{dump}");
    assert!(dump.contains("local c: bool"), "{dump}");
    assert!(dump.contains("local d: str"), "{dump}");
}

#[test]
fn int_arithmetic_stays_int_but_division_is_float() {
    let dump = ok_main("a = 7 + 1\nb = 7 // 2\nc = 7 % 2\nd = 7 / 2\ne = 2 ** 3");
    assert!(dump.contains("local a: int"), "{dump}");
    assert!(dump.contains("local b: int"), "{dump}");
    assert!(dump.contains("local c: int"), "{dump}");
    // DESIGN 3.6: `/` on two ints is a float.
    assert!(dump.contains("local d: float"), "{dump}");
    assert!(dump.contains("local e: int"), "{dump}");
}

#[test]
fn comparisons_and_boolean_operators_yield_bool() {
    let dump = ok_main("a = 1 < 2\nb = a and True\nc = not a\nd = \"x\" == \"y\"");
    for name in ["a", "b", "c", "d"] {
        assert!(dump.contains(&format!("local {name}: bool")), "{dump}");
    }
}

#[test]
fn strings_concatenate_and_f_strings_are_str() {
    let dump = ok_main("a = \"x\" + \"y\"\nb = f\"{a}!\"");
    assert!(dump.contains("local a: str"), "{dump}");
    assert!(dump.contains("local b: str"), "{dump}");
}

#[test]
fn conversions_have_the_documented_shape() {
    let dump =
        ok_main("a = float(1)\nb = int(1.5)\nc = float(1.5)\nd = abs(-1)\ne = min(1.0, 2.0)");
    assert!(dump.contains("a = float(1:int):float"), "{dump}");
    assert!(dump.contains("b = int(1.5:float):int"), "{dump}");
    // DESIGN 4.3: `float(x)` on a float is a contraction barrier, not a no-op.
    assert!(dump.contains("c = fence(1.5:float):float"), "{dump}");
    assert!(dump.contains("d = abs("), "{dump}");
    assert!(dump.contains("e = min("), "{dump}");
}

#[test]
fn powers_of_two_and_a_half_are_lowered_to_multiply_and_sqrt() {
    let dump = ok_main("x = 3.0\na = x ** 0.5\nb = x ** 2.0\nc = 3 ** 2\nd = x ** 3.0");
    assert!(dump.contains("a = sqrt(x:float):float"), "{dump}");
    assert!(dump.contains("b = square(x:float):float"), "{dump}");
    assert!(dump.contains("c = square(3:int):int"), "{dump}");
    assert!(dump.contains("d = (x:float ** 3.0:float):float"), "{dump}");
}

#[test]
fn constants_are_folded_into_literals_at_every_use() {
    let dump = ok(
        "LIMIT: int = 2 * 5\nSCALE: float = 1.5\n\nfn main():\n    n = LIMIT + 1\n    f = SCALE\n",
    );
    assert!(dump.contains("n = (10:int + 1:int):int"), "{dump}");
    assert!(dump.contains("f = 1.5:float"), "{dump}");
}

#[test]
fn a_constant_expression_may_use_earlier_constants_and_operators() {
    let dump = ok(
        "A: int = 1 << 4\nB: int = A - 6\nC: float = 1.0 / 4.0\nD: bool = A > B\nE: str = \"a\" + \"b\"\n\nfn main():\n    print(A, B, C, D, E)\n",
    );
    assert!(
        dump.contains("print(16:int, 10:int, 0.25:float, True:bool, \"ab\":str)"),
        "{dump}"
    );
}

#[test]
fn main_is_mangled_and_returns_unit() {
    let src = "fn main():\n    pass\n";
    let mut sources = SourceMap::new();
    let file = sources.add("main.ty", src);
    let mut diags = Diagnostics::new();
    let module = typhoon_parser::parse_module(file, src, &mut diags);
    let program = check_module(&module, file, &mut diags).unwrap();
    let main = program.function(program.main);
    assert_eq!(main.name, "main");
    assert_eq!(main.symbol, "ty_user_main");
    assert_eq!(main.ret, typhoon_sema::types::TypeId::UNIT);
    assert!(main.params.is_empty());
}

// ---------------------------------------------------------------------------
// Lowering
// ---------------------------------------------------------------------------

#[test]
fn elif_chains_become_nested_ifs() {
    let dump =
        ok_main("n = 1\nif n > 1:\n    print(1)\nelif n > 0:\n    print(2)\nelse:\n    print(3)");
    insta::assert_snapshot!(dump);
}

#[test]
fn for_range_carries_start_stop_and_step() {
    let dump = ok_main(
        "for i in range(10):\n    print(i)\nfor j in range(1, 10):\n    print(j)\nfor k in range(10, 0, -2):\n    print(k)",
    );
    insta::assert_snapshot!(dump);
}

#[test]
fn defaults_are_substituted_and_keywords_reordered() {
    let dump = ok(
        "fn scale(x: float, factor: float = 2.0) -> float:\n    return x * factor\n\nfn main():\n    print(scale(1.0))\n    print(scale(factor=3.0, x=1.0))\n",
    );
    insta::assert_snapshot!(dump);
}

#[test]
fn a_unit_function_call_is_a_statement() {
    let dump = ok("fn log(n: int):\n    print(n)\n\nfn main():\n    log(1)\n");
    assert!(dump.contains("log(1:int):None"), "{dump}");
}

// ---------------------------------------------------------------------------
// Definite assignment (DESIGN 3.4)
// ---------------------------------------------------------------------------

#[test]
fn assignment_in_both_branches_is_definite() {
    ok_main("n = 1\nif n > 0:\n    v = 1\nelse:\n    v = 2\nprint(v)");
}

#[test]
fn assignment_in_one_branch_is_not_definite() {
    rejects_main(
        "n = 1\nif n > 0:\n    v = 1\nprint(v)",
        [
            "use of possibly-uninitialized variable `v`",
            "may be uninitialized here",
        ]
        .as_slice(),
    );
}

#[test]
fn a_diverging_branch_does_not_have_to_assign() {
    // The `return` path never reaches the read, so `v` is definite after the
    // `if` even though the `then` branch never assigns it.
    ok(
        "fn f(n: int) -> int:\n    if n > 0:\n        return 0\n    else:\n        v = 1\n    return v\n\nfn main():\n    print(f(1))\n",
    );
}

#[test]
fn a_loop_body_may_not_run() {
    rejects_main(
        "for i in range(3):\n    last = i\nprint(last)",
        ["use of possibly-uninitialized variable `last`"].as_slice(),
    );
    rejects_main(
        "n = 0\nwhile n > 0:\n    seen = n\nprint(seen)",
        ["use of possibly-uninitialized variable `seen`"].as_slice(),
    );
}

#[test]
fn a_variable_assigned_before_the_loop_stays_definite() {
    ok_main("last = -1\nfor i in range(3):\n    last = i\nprint(last)");
    // The loop variable itself is an ordinary local (DESIGN 3.4).
    ok_main("i = -1\nfor i in range(3):\n    print(i)\nprint(i)");
}

#[test]
fn the_loop_variable_is_not_definite_after_an_empty_loop() {
    rejects_main(
        "for i in range(3):\n    print(i)\nprint(i)",
        ["use of possibly-uninitialized variable `i`"].as_slice(),
    );
}

#[test]
fn a_variable_declared_in_a_branch_is_visible_afterwards() {
    // Function scope, not block scope: the declaration escapes the `if`, only
    // the assignment has to be completed on every path.
    ok_main("n = 1\nif n > 0:\n    v = 1\nelse:\n    pass\nv = 2\nprint(v)");
}

#[test]
fn the_loop_variable_must_be_an_int() {
    rejects_main(
        "i = \"x\"\nfor i in range(3):\n    print(i)",
        ["mismatched types", "expected `str`, found `int`"].as_slice(),
    );
}

// ---------------------------------------------------------------------------
// Return analysis
// ---------------------------------------------------------------------------

#[test]
fn every_path_must_return() {
    rejects(
        "fn f(n: int) -> int:\n    if n > 0:\n        return 1\n\nfn main():\n    print(f(1))\n",
        ["missing return in function `f` returning `int`"].as_slice(),
    );
    // An `if`/`else` where both arms return is enough.
    ok(
        "fn f(n: int) -> int:\n    if n > 0:\n        return 1\n    else:\n        return 2\n\nfn main():\n    print(f(1))\n",
    );
    // A loop never counts as returning.
    rejects(
        "fn f(n: int) -> int:\n    while n > 0:\n        return 1\n\nfn main():\n    print(f(1))\n",
        ["missing return"].as_slice(),
    );
}

#[test]
fn a_unit_function_needs_no_return() {
    ok("fn log(n: int):\n    if n > 0:\n        return\n    print(n)\n\nfn main():\n    log(1)\n");
}

#[test]
fn statements_after_a_return_are_dead_but_checked() {
    // Unreachable code is dropped from the IR, yet still type-checked.
    let dump = ok("fn f() -> int:\n    return 1\n    return 2\n\nfn main():\n    print(f())\n");
    assert_eq!(dump.matches("return").count(), 1, "{dump}");
    rejects(
        "fn f() -> int:\n    return 1\n    return \"x\"\n\nfn main():\n    print(f())\n",
        ["mismatched types"].as_slice(),
    );
}

// ---------------------------------------------------------------------------
// Loops
// ---------------------------------------------------------------------------

#[test]
fn break_and_continue_need_a_loop() {
    rejects_main("break", ["`break` outside of a loop"].as_slice());
    rejects_main("continue", ["`continue` outside of a loop"].as_slice());
    ok_main("while True:\n    break");
    ok_main("for i in range(3):\n    continue");
}

#[test]
fn range_takes_one_to_three_arguments() {
    rejects_main(
        "for i in range():\n    print(i)",
        ["`range` takes 1, 2 or 3 arguments"].as_slice(),
    );
    rejects_main(
        "for i in range(1, 2, 3, 4):\n    print(i)",
        ["`range` takes 1, 2 or 3 arguments"].as_slice(),
    );
    rejects_main(
        "for i in range(1.0):\n    print(i)",
        ["mismatched types", "expected `int`, found `float`"].as_slice(),
    );
}

// ---------------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------------

#[test]
fn arguments_are_checked_against_the_signature() {
    let program = "fn add(a: int, b: int) -> int:\n    return a + b\n\nfn main():\n";
    rejects(
        &format!("{program}    print(add(1))\n"),
        ["missing argument `b`"].as_slice(),
    );
    rejects(
        &format!("{program}    print(add(1, 2, 3))\n"),
        ["`add` takes 2 arguments but 3 were supplied"].as_slice(),
    );
    rejects(
        &format!("{program}    print(add(1, c=2))\n"),
        ["`add` has no parameter named `c`"].as_slice(),
    );
    rejects(
        &format!("{program}    print(add(1, a=2))\n"),
        ["argument `a` specified twice"].as_slice(),
    );
    rejects(
        &format!("{program}    print(add(1, 2.0))\n"),
        ["mismatched types", "expected `int`, found `float`"].as_slice(),
    );
    ok(&format!("{program}    print(add(b=2, a=1))\n"));
}

#[test]
fn functions_are_not_values_and_values_are_not_functions() {
    rejects(
        "fn f() -> int:\n    return 1\n\nfn main():\n    g = f\n",
        ["`f` is a function, not a value"].as_slice(),
    );
    rejects_main(
        "x = 1\nprint(x(2))",
        ["cannot call a value of type `int`"].as_slice(),
    );
    rejects_main(
        "print(nope(1))",
        ["cannot find function `nope` in this scope"].as_slice(),
    );
}

#[test]
fn a_default_must_be_a_constant_of_the_parameter_type() {
    rejects(
        "fn f(a: int = 1.5) -> int:\n    return a\n\nfn main():\n    print(f())\n",
        ["mismatched types", "expected `int`, found `float`"].as_slice(),
    );
    rejects(
        "fn g() -> int:\n    return 1\n\nfn f(a: int = g()) -> int:\n    return a\n\nfn main():\n    print(f())\n",
        ["expected a constant expression"].as_slice(),
    );
    rejects(
        "fn f(a: int = 1, b: int) -> int:\n    return a + b\n\nfn main():\n    print(f(1, 2))\n",
        ["parameter `b` has no default but follows one that has"].as_slice(),
    );
}

#[test]
fn builtins_check_their_arguments() {
    rejects_main(
        "print(float(1, 2))",
        ["`float` takes 1 argument"].as_slice(),
    );
    rejects_main("print(min(1))", ["`min` takes 2 arguments"].as_slice());
    rejects_main("print(min(1, 2.0))", ["mismatched types"].as_slice());
    rejects_main(
        "print(abs(\"x\"))",
        ["`abs` cannot be applied to a value of type `str`"].as_slice(),
    );
    rejects_main(
        "print(float(x=1))",
        ["does not take keyword arguments"].as_slice(),
    );
    rejects_main(
        "r = range(3)",
        ["`range` can only be used as the iterable of a `for` loop"].as_slice(),
    );
}

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

#[test]
fn names_must_resolve() {
    rejects_main(
        "print(total)",
        ["cannot find value `total` in this scope"].as_slice(),
    );
    rejects(
        "fn main():\n    x: Widget = 1\n",
        ["cannot find type `Widget` in this scope"].as_slice(),
    );
}

#[test]
fn a_name_may_only_be_declared_once() {
    rejects(
        "fn f() -> int:\n    return 1\n\nfn f() -> int:\n    return 2\n\nfn main():\n    print(f())\n",
        ["the name `f` is defined multiple times"].as_slice(),
    );
    rejects(
        "N: int = 1\n\nfn N() -> int:\n    return 1\n\nfn main():\n    print(N())\n",
        ["the name `N` is defined multiple times"].as_slice(),
    );
    rejects_main(
        "x: int = 1\nx: int = 2",
        ["the variable `x` is already declared in this function"].as_slice(),
    );
    rejects(
        "fn f(a: int, a: int) -> int:\n    return a\n\nfn main():\n    print(f(1, 2))\n",
        ["the parameter `a` is declared twice"].as_slice(),
    );
}

#[test]
fn a_constant_is_not_a_variable() {
    rejects(
        "LIMIT: int = 1\n\nfn main():\n    LIMIT = 2\n",
        ["cannot assign to the constant `LIMIT`"].as_slice(),
    );
}

#[test]
fn the_entry_point_is_checked() {
    rejects(
        "fn helper() -> int:\n    return 1\n",
        ["this program has no `main` function"].as_slice(),
    );
    rejects(
        "fn main(n: int):\n    print(n)\n",
        ["`main` must not take parameters"].as_slice(),
    );
    rejects(
        "fn main() -> int:\n    return 0\n",
        ["`main` must not have a return type"].as_slice(),
    );
}

// ---------------------------------------------------------------------------
// No implicit conversions, no truthiness (DESIGN 3.6, 4.3)
// ---------------------------------------------------------------------------

#[test]
fn int_and_float_never_mix() {
    for source in [
        "x = 1 + 1.0",
        "x = 1.0 - 1",
        "x = 1 * 1.0",
        "x = 1.0 / 1",
        "x = 1 // 1.0",
        "x = 1.0 % 1",
        "x = 1.0 ** 2",
        "x = 1 < 1.0",
        "x = min(1, 1.0)",
    ] {
        rejects_main(source, ["mismatched types"].as_slice());
    }
    // The help always points at the conversion that is missing.
    let rendered = rejects_main("x = 1.0 + 1", ["expected `float`, found `int`"].as_slice());
    assert!(rendered.contains("use `float(x)`"), "{rendered}");
}

#[test]
fn a_local_keeps_the_type_of_its_first_assignment() {
    rejects_main(
        "x = 1\nx = 1.5",
        [
            "mismatched types",
            "expected `int`, found `float`",
            "was declared with type `int` here",
        ]
        .as_slice(),
    );
    ok_main("x = 1\nx = 2");
}

#[test]
fn conditions_must_be_bool() {
    rejects_main(
        "if 1:\n    pass",
        ["expected `bool`, found `int`", "compare explicitly"].as_slice(),
    );
    rejects_main(
        "while 1.0:\n    pass",
        ["expected `bool`, found `float`"].as_slice(),
    );
    rejects_main(
        "x = 1\nif x and True:\n    pass",
        ["expected `bool`, found `int`"].as_slice(),
    );
    rejects_main(
        "print(not 1)",
        ["`not` cannot be applied to a value of type `int`"].as_slice(),
    );
}

#[test]
fn operators_are_defined_per_type() {
    rejects_main(
        "print(\"a\" - \"b\")",
        ["`-` cannot be applied to `str` and `str`"].as_slice(),
    );
    rejects_main(
        "print(\"a\" < \"b\")",
        ["`<` cannot be applied to `str` and `str`"].as_slice(),
    );
    rejects_main(
        "print(True < False)",
        ["`<` cannot be applied to `bool` and `bool`"].as_slice(),
    );
    rejects_main(
        "print(1.0 & 2.0)",
        ["bitwise operators require `int` operands"].as_slice(),
    );
    rejects_main(
        "print(~1.5)",
        ["`~` cannot be applied to a value of type `float`"].as_slice(),
    );
    ok_main("print(\"a\" == \"b\", True != False)");
}

#[test]
fn unit_values_are_not_values() {
    rejects(
        "fn log(n: int):\n    print(n)\n\nfn main():\n    x = log(1)\n",
        ["cannot declare `x` with type `None`"].as_slice(),
    );
    rejects(
        "fn log(n: int):\n    print(n)\n\nfn main():\n    print(log(1))\n",
        ["cannot print a value of type `None`"].as_slice(),
    );
    rejects_main(
        "x = None",
        ["cannot declare `x` with type `None`"].as_slice(),
    );
}

// ---------------------------------------------------------------------------
// f-strings (DESIGN 3.7)
// ---------------------------------------------------------------------------

#[test]
fn format_specs_are_checked_against_the_value() {
    ok_main("n = 1\nf = 1.5\ns = \"x\"\nb = True\nprint(f\"{n} {f} {s} {b}\")");
    ok_main("n = 1\nprint(f\"{n:d}\")");
    ok_main("s = \"x\"\nprint(f\"{s:s}\")");
    ok_main("f = 1.5\nprint(f\"{f:.3f}\")");
    rejects_main(
        "n = 1\nprint(f\"{n:.2f}\")",
        [
            "the format spec `.2f` requires a `float`, found `int`",
            "use `float(x)`",
        ]
        .as_slice(),
    );
    rejects_main(
        "f = 1.5\nprint(f\"{f:d}\")",
        ["the format spec `d` requires an `int`, found `float`"].as_slice(),
    );
    rejects_main(
        "n = 1\nprint(f\"{n:x}\")",
        ["unsupported format spec `x`"].as_slice(),
    );
    rejects_main(
        "f = 1.5\nprint(f\"{f:.99f}\")",
        ["invalid format spec `.99f`"].as_slice(),
    );
}

// ---------------------------------------------------------------------------
// Constant expressions
// ---------------------------------------------------------------------------

#[test]
fn constant_initializers_are_evaluated_at_compile_time() {
    rejects(
        "N: int = 1 // 0\n\nfn main():\n    print(N)\n",
        ["division by zero in a constant expression"].as_slice(),
    );
    rejects(
        "N: int = 2 ** -1\n\nfn main():\n    print(N)\n",
        ["negative exponent in an integer power"].as_slice(),
    );
    rejects(
        "N: int = M + 1\nM: int = 1\n\nfn main():\n    print(N)\n",
        [
            "expected a constant expression",
            "only constants declared earlier",
        ]
        .as_slice(),
    );
    rejects(
        "N: float = 1\n\nfn main():\n    print(N)\n",
        ["mismatched types", "expected `float`, found `int`"].as_slice(),
    );
}

// ---------------------------------------------------------------------------
// Features of later milestones
// ---------------------------------------------------------------------------

#[test]
fn later_milestones_are_named_in_the_diagnostic() {
    let cases: &[(&str, &str)] = &[
        (
            "x = [1]",
            "a list literal is not supported yet (planned for M2)",
        ),
        (
            "x = {1: 2}",
            "a dict literal is not supported yet (planned for M3)",
        ),
        (
            "x = {1, 2}",
            "a set literal is not supported yet (planned for M3)",
        ),
        (
            "x = (1, 2)",
            "a tuple is not supported yet (planned for M2)",
        ),
        (
            "x = 1\ny = x[0]",
            "indexing is not supported yet (planned for M2)",
        ),
        (
            "x = 1\ny = x.field",
            "attribute access is not supported yet (planned for M2)",
        ),
        (
            "x = 1 < 2 < 3",
            "a comparison chain is not supported yet (planned for M3)",
        ),
        (
            "x = 1\ny = x is None",
            "the `is` operator is not supported yet (planned for M3)",
        ),
        (
            "x = 1\ny = x in \"a\"",
            "the `in` operator is not supported yet (planned for M2)",
        ),
        (
            "print(len(\"a\"))",
            "the builtin `len` is not supported yet (planned for M2)",
        ),
        (
            "print(str(1))",
            "the builtin `str` is not supported yet (planned for M2)",
        ),
        (
            "print(sorted(1))",
            "the builtin `sorted` is not supported yet (planned for M3)",
        ),
        (
            "xs: list<int> = [1]",
            "the type `list` is not supported yet (planned for M2)",
        ),
        (
            "x: i32 = 1",
            "the fixed-width type `i32` is not supported yet (planned for M2)",
        ),
        (
            "for c in \"abc\":\n    print(1)",
            "iterating over anything but `range(...)` is not supported yet (planned for M2)",
        ),
    ];
    for (body, wanted) in cases {
        rejects_main(body, [*wanted].as_slice());
    }

    rejects(
        "class Point:\n    x: int\n\nfn main():\n    pass\n",
        ["the `class` declaration is not supported yet (planned for M2)"].as_slice(),
    );
    rejects(
        "fn f<T>(a: T) -> T:\n    return a\n\nfn main():\n    pass\n",
        ["a generic function is not supported yet (planned for M3)"].as_slice(),
    );
    rejects(
        "fn f(a: int) -> int | None:\n    return a\n\nfn main():\n    pass\n",
        ["an optional type (`T | None`) is not supported yet (planned for M3)"].as_slice(),
    );
    rejects(
        "fn f(a: int) -> int:\n    return a\n\nfn main():\n    print(f<int>(1))\n",
        ["a generic call is not supported yet (planned for M3)"].as_slice(),
    );
    rejects(
        "fn f(self) -> int:\n    return 1\n\nfn main():\n    pass\n",
        ["`self` is only allowed on a method of a class"].as_slice(),
    );
}

#[test]
fn one_mistake_is_one_diagnostic() {
    // A rejected initializer poisons the variable, so reading it later adds
    // nothing (the count is of `error: ` headlines).
    let rendered = err("fn main():\n    x = 1 + 1.0\n    print(x)\n    print(x + 1)\n");
    assert_eq!(rendered.matches("error: ").count(), 1, "{rendered}");

    let rendered = err("LIMIT: int = nope()\n\nfn main():\n    print(LIMIT)\n");
    assert_eq!(rendered.matches("error: ").count(), 1, "{rendered}");

    let rendered = err("fn main():\n    print([1][0])\n");
    assert_eq!(rendered.matches("error: ").count(), 1, "{rendered}");
}

// ---------------------------------------------------------------------------
// Rendered diagnostics
// ---------------------------------------------------------------------------

macro_rules! snapshot_error {
    ($name:ident, $src:expr) => {
        #[test]
        fn $name() {
            insta::assert_snapshot!(err($src));
        }
    };
}

snapshot_error!(
    snapshot_mixed_arithmetic,
    "fn main():\n    total = 1 + 2.0\n    print(total)\n"
);
snapshot_error!(
    snapshot_reassignment_with_another_type,
    "fn main():\n    x = 1\n    x = 1.5\n    print(x)\n"
);
snapshot_error!(snapshot_undefined_name, "fn main():\n    print(total)\n");
snapshot_error!(
    snapshot_possibly_uninitialized,
    "fn main():\n    n = 1\n    if n > 0:\n        answer = 1\n    print(answer)\n"
);
snapshot_error!(
    snapshot_non_bool_condition,
    "fn main():\n    n = 3\n    if n:\n        print(n)\n"
);
snapshot_error!(snapshot_not_on_int, "fn main():\n    print(not 1)\n");
snapshot_error!(
    snapshot_wrong_arity,
    "fn add(a: int, b: int) -> int:\n    return a + b\n\nfn main():\n    print(add(1, 2, 3))\n"
);
snapshot_error!(
    snapshot_unknown_keyword_argument,
    "fn add(a: int, b: int) -> int:\n    return a + b\n\nfn main():\n    print(add(a=1, c=2))\n"
);
snapshot_error!(
    snapshot_missing_return,
    "fn classify(n: int) -> int:\n    if n > 0:\n        return 1\n\nfn main():\n    print(classify(0))\n"
);
snapshot_error!(
    snapshot_return_value_from_unit_function,
    "fn log(n: int):\n    return n\n\nfn main():\n    log(1)\n"
);
snapshot_error!(
    snapshot_return_without_value,
    "fn pick(n: int) -> int:\n    return\n\nfn main():\n    print(pick(1))\n"
);
snapshot_error!(snapshot_missing_main, "fn helper() -> int:\n    return 1\n");
snapshot_error!(
    snapshot_main_with_parameters,
    "fn main(argc: int):\n    print(argc)\n"
);
snapshot_error!(
    snapshot_class_is_a_later_milestone,
    "class Point:\n    x: float\n\nfn main():\n    pass\n"
);
snapshot_error!(
    snapshot_comparison_chain_is_a_later_milestone,
    "fn main():\n    a = 1\n    b = 2\n    c = 3\n    print(a < b < c)\n"
);
snapshot_error!(
    snapshot_range_outside_a_for_loop,
    "fn main():\n    r = range(10)\n"
);
snapshot_error!(
    snapshot_bad_format_spec,
    "fn main():\n    n = 3\n    print(f\"{n:.2f}\")\n"
);
snapshot_error!(
    snapshot_non_constant_initializer,
    "fn compute() -> int:\n    return 42\n\nLIMIT: int = compute()\n\nfn main():\n    print(LIMIT)\n"
);
snapshot_error!(
    snapshot_shadowing,
    "fn main():\n    x: int = 1\n    x: float = 2.0\n    print(x)\n"
);
snapshot_error!(
    snapshot_duplicate_definition,
    "fn value() -> int:\n    return 1\n\nfn value() -> int:\n    return 2\n\nfn main():\n    print(value())\n"
);
