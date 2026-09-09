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
    // `str` gained the ordering operators in M2 (code point order).
    ok_main("print(\"a\" < \"b\", \"a\" >= \"b\")");
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
            "x = {1: 2}",
            "a dict literal is not supported yet (planned for M3)",
        ),
        (
            "x = {1, 2}",
            "a set literal is not supported yet (planned for M3)",
        ),
        ("x = 1\ny = x[0]", "`int` cannot be indexed"),
        ("x = 1\ny = x.field", "`int` has no field `field`"),
        (
            "x = 1 < 2 < 3",
            "a comparison chain is not supported yet (planned for M3)",
        ),
        ("x = 1\ny = x is None", "`int` can never be `None`"),
        (
            "x = 1\ny = x in \"a\"",
            "`in` on a `str` tests for a substring, found `int`",
        ),
        (
            "print(sorted(1))",
            "the builtin `sorted` is not supported yet (planned for M3)",
        ),
        (
            "d: dict<str, int> = {}",
            "the type `dict` is not supported yet (planned for M3)",
        ),
        (
            "x: i32 = 1",
            "the fixed-width type `i32` is not supported yet (planned for M3)",
        ),
        ("for c in 5:\n    print(1)", "cannot iterate over an `int`"),
    ];
    for (body, wanted) in cases {
        rejects_main(body, [*wanted].as_slice());
    }

    rejects(
        "class Stack<T>:\n    n: int\n\nfn main():\n    pass\n",
        ["a generic class is not supported yet (planned for M3)"].as_slice(),
    );
    rejects(
        "fn f<T>(a: T) -> T:\n    return a\n\nfn main():\n    pass\n",
        ["a generic function is not supported yet (planned for M3)"].as_slice(),
    );
    rejects(
        "fn f(a: int) -> int | None:\n    return a\n\nfn main():\n    pass\n",
        ["an optional type (`int | None`) is not supported yet (planned for M3)"].as_slice(),
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

    let rendered = err("fn main():\n    xs = [1]\n    print(xs[\"a\"])\n");
    assert_eq!(rendered.matches("error: ").count(), 1, "{rendered}");
}

// ---------------------------------------------------------------------------
// M2: `list<T>` (DESIGN 4.1, 4.4, 4.6)
// ---------------------------------------------------------------------------

#[test]
fn a_list_literal_infers_its_element_type() {
    let dump = ok_main("xs = [1, 2, 3]\nys = [\"a\"]\nzs = [1.5]\nbs = [True]");
    assert!(dump.contains("local xs: list<int>"), "{dump}");
    assert!(dump.contains("local ys: list<str>"), "{dump}");
    assert!(dump.contains("local zs: list<float>"), "{dump}");
    assert!(dump.contains("local bs: list<bool>"), "{dump}");
    assert!(
        dump.contains("xs = [1:int, 2:int, 3:int]:list<int>"),
        "{dump}"
    );
}

#[test]
fn a_list_of_lists_nests() {
    let dump = ok_main("grid = [[1, 2], [3]]\nrow = grid[0]\nn = row[1]");
    assert!(dump.contains("local grid: list<list<int>>"), "{dump}");
    assert!(dump.contains("local row: list<int>"), "{dump}");
    assert!(dump.contains("local n: int"), "{dump}");
}

#[test]
fn an_empty_list_literal_takes_its_type_from_the_annotation() {
    // DESIGN 4.4: inference is local, so an empty container needs the hint.
    let dump = ok_main(
        "xs: list<int> = []\nxs.append(1)\ngrid: list<list<int>> = []\ngrid.append(xs)\nprint(len(grid))",
    );
    assert!(dump.contains("local xs: list<int>"), "{dump}");
    assert!(dump.contains("local grid: list<list<int>>"), "{dump}");
    assert!(dump.contains("xs = []:list<int>"), "{dump}");
    rejects_main(
        "xs = []",
        [
            "cannot infer the element type of an empty list",
            "annotate the variable, e.g. `xs: list<int> = []`",
        ]
        .as_slice(),
    );
}

#[test]
fn the_elements_of_a_list_literal_must_agree() {
    rejects_main(
        "xs = [1, 2.0]",
        [
            "mismatched types: expected `int`, found `float`",
            "every element of a list literal must have the same type",
        ]
        .as_slice(),
    );
    rejects_main(
        "xs = [\"a\", 1]",
        ["expected `str`, found `int`"].as_slice(),
    );
    // The inferred element type of the first row is the expected type of the
    // rows after it, so the mismatch is reported at the offending element.
    rejects_main(
        "xs = [[1], [\"a\"]]",
        ["expected `int`, found `str`"].as_slice(),
    );
    // An inner empty literal takes its type from the row before it.
    ok_main(
        "xs = [[1], []]
print(xs)",
    );
}

#[test]
fn a_list_of_ints_is_not_a_list_of_floats() {
    rejects_main(
        "xs = [1]\nys: list<float> = xs",
        ["expected `list<float>`, found `list<int>`"].as_slice(),
    );
    // The hint reaches the elements of a literal argument, so the complaint is
    // about the element, not the list.
    rejects(
        "fn total(xs: list<float>) -> float:\n    return xs[0]\n\nfn main():\n    print(total([1]))\n",
        ["expected `float`, found `int`"].as_slice(),
    );
    rejects(
        "fn total(xs: list<float>) -> float:\n    return xs[0]\n\nfn main():\n    ys = [1]\n    print(total(ys))\n",
        ["expected `list<float>`, found `list<int>`"].as_slice(),
    );
}

#[test]
fn the_list_methods_have_the_documented_types() {
    let dump = ok_main(
        "xs = [1, 2]\nxs.append(3)\nlast = xs.pop()\nxs.insert(0, 9)\nn = len(xs)\nfirst = xs[0]\nxs[1] = 7\nboth = xs + [4]\nhas = 3 in xs\nmissing = 3 not in xs\nxs.clear()",
    );
    for (name, ty) in [
        ("last", "int"),
        ("n", "int"),
        ("first", "int"),
        ("both", "list<int>"),
        ("has", "bool"),
        ("missing", "bool"),
    ] {
        assert!(dump.contains(&format!("local {name}: {ty}")), "{dump}");
    }
    // `append`, `insert` and `clear` produce no value, so they are statements.
    assert!(dump.contains(".append(3:int):None"), "{dump}");
    assert!(dump.contains(".insert(0:int, 9:int):None"), "{dump}");
    assert!(dump.contains(".clear():None"), "{dump}");
    assert!(dump.contains("(3:int not in xs:list<int>):bool"), "{dump}");
}

#[test]
fn the_list_methods_check_their_arguments() {
    rejects_main(
        "xs = [1]\nxs.append(\"a\")",
        ["expected `int`, found `str`"].as_slice(),
    );
    rejects_main(
        "xs = [1]\nxs.append(1, 2)",
        ["`list<int>.append` takes 1 argument but 2 were supplied"].as_slice(),
    );
    rejects_main(
        "xs = [1]\nxs.insert(\"a\", 1)",
        ["expected `int`, found `str`"].as_slice(),
    );
    rejects_main(
        "xs = [1]\nprint(xs.first())",
        [
            "`list<int>` has no method `first`",
            "`list` has `append`, `pop`, `insert` and `clear`",
        ]
        .as_slice(),
    );
    rejects_main(
        "xs = [1]\nprint(xs[\"a\"])",
        ["a list index must be an `int`, found `str`"].as_slice(),
    );
    rejects_main(
        "xs = [1]\nys = [\"a\"]\nprint(xs + ys)",
        ["expected `list<int>`, found `list<str>`"].as_slice(),
    );
    rejects_main(
        "xs = [1]\nprint(xs < xs)",
        ["`list<int>` values cannot be ordered"].as_slice(),
    );
    rejects_main(
        "xs = [1]\nprint(\"a\" in xs)",
        ["expected `int`, found `str`"].as_slice(),
    );
    // A method that yields no value is not a value.
    rejects_main(
        "xs = [1]\nn = xs.append(2)",
        ["cannot declare `n` with type `None`"].as_slice(),
    );
}

#[test]
fn a_container_of_none_has_no_values() {
    rejects_main(
        "xs: list<None> = []",
        ["`list<None>` has no values"].as_slice(),
    );
    rejects_main(
        "t: tuple<int, None> = (1, 2)",
        ["a tuple member cannot have type `None`"].as_slice(),
    );
    rejects_main(
        "xs: list = []",
        ["`list` takes exactly one type argument, e.g. `list<int>`"].as_slice(),
    );
    rejects_main(
        "t: tuple = (1,)",
        ["`tuple` takes at least one type argument, e.g. `tuple<int, str>`"].as_slice(),
    );
}

// ---------------------------------------------------------------------------
// M2: `tuple<A, B, …>` (DESIGN 4.1, 4.2)
// ---------------------------------------------------------------------------

#[test]
fn tuple_literals_and_annotations_agree() {
    let dump = ok_main(
        "t = (1, \"a\", True)\nu: tuple<int, str> = (1, \"a\")\nnested = ((1, 2), 3)\nsingle = (1,)",
    );
    assert!(dump.contains("local t: tuple<int, str, bool>"), "{dump}");
    assert!(dump.contains("local u: tuple<int, str>"), "{dump}");
    assert!(
        dump.contains("local nested: tuple<tuple<int, int>, int>"),
        "{dump}"
    );
    // Python spells a one-member tuple `(1,)`, and so does typhoon.
    assert!(dump.contains("local single: tuple<int>"), "{dump}");
}

#[test]
fn a_tuple_index_is_a_constant_checked_at_compile_time() {
    let dump = ok_main("t = (1, \"a\")\nn = t[0]\ns = t[1]\nlast = t[-1]");
    assert!(dump.contains("local n: int"), "{dump}");
    assert!(dump.contains("local s: str"), "{dump}");
    assert!(dump.contains("local last: str"), "{dump}");
    // A negative index is resolved at compile time.
    assert!(dump.contains("last = t:tuple<int, str>[1]:str"), "{dump}");

    rejects_main(
        "t = (1, \"a\")\ni = 0\nprint(t[i])",
        [
            "a tuple index must be a constant",
            "has to be known at compile time",
        ]
        .as_slice(),
    );
    rejects_main(
        "t = (1, \"a\")\nprint(t[2])",
        ["tuple index 2 is out of range for `tuple<int, str>`"].as_slice(),
    );
    rejects_main(
        "t = (1, \"a\")\nprint(t[-3])",
        ["tuple index -3 is out of range"].as_slice(),
    );
    rejects_main(
        "t = (1, \"a\")\nt[0] = 2",
        [
            "cannot assign through an index into `tuple<int, str>`",
            "tuples are values and cannot be mutated",
        ]
        .as_slice(),
    );
}

#[test]
fn len_of_a_tuple_is_folded_into_a_literal() {
    // A tuple's arity is part of its type, so `len(t)` costs nothing.
    let dump = ok_main("t = (1, \"a\", True)\nn = len(t)");
    assert!(dump.contains("n = 3:int"), "{dump}");
}

#[test]
fn the_empty_tuple_is_rejected() {
    rejects_main(
        "t = ()",
        [
            "the empty tuple `()` has no type",
            "a tuple needs at least one member",
        ]
        .as_slice(),
    );
    let dump = ok_main("t = (1,)\nprint(len(t), t[0])");
    assert!(dump.contains("local t: tuple<int>"), "{dump}");
    assert!(dump.contains("t = (1:int):tuple<int>"), "{dump}");
}

#[test]
fn a_tuple_is_a_value_that_is_passed_and_returned() {
    let dump = ok(
        "fn swap(t: tuple<int, str>) -> tuple<str, int>:\n    return (t[1], t[0])\n\nfn main():\n    a, b = swap((1, \"x\"))\n    print(a, b)\n",
    );
    assert!(
        dump.contains("fn swap(t: tuple<int, str>) -> tuple<str, int>"),
        "{dump}"
    );
    assert!(dump.contains("local a: str"), "{dump}");
    assert!(dump.contains("local b: int"), "{dump}");
}

// ---------------------------------------------------------------------------
// M2: `str` (DESIGN 4.6)
// ---------------------------------------------------------------------------

#[test]
fn the_str_methods_have_the_documented_result_types() {
    let dump = ok_main(
        "s = \"Hello World\"\nn = len(s)\nu = s.upper()\nl = s.lower()\nstripped = s.strip()\nparts = s.split(\" \")\njoined = \",\".join(parts)\nstarts = s.startswith(\"He\")\nends = s.endswith(\"ld\")\nat = s.find(\"o\")\nreplaced = s.replace(\"l\", \"L\")",
    );
    for (name, ty) in [
        ("n", "int"),
        ("u", "str"),
        ("l", "str"),
        ("stripped", "str"),
        ("parts", "list<str>"),
        ("joined", "str"),
        ("starts", "bool"),
        ("ends", "bool"),
        ("at", "int"),
        ("replaced", "str"),
    ] {
        assert!(dump.contains(&format!("local {name}: {ty}")), "{dump}");
    }
    assert!(dump.contains("n = len(s:str):int"), "{dump}");
    assert!(dump.contains("s:str.split(\" \":str):list<str>"), "{dump}");
}

#[test]
fn the_str_methods_check_their_arguments() {
    rejects_main(
        "s = \"a\"\nprint(s.upper(1))",
        ["`str.upper` takes 0 arguments but 1 was supplied"].as_slice(),
    );
    rejects_main(
        "s = \"a\"\nprint(s.replace(\"a\"))",
        ["`str.replace` takes 2 arguments but 1 was supplied"].as_slice(),
    );
    rejects_main(
        "s = \"a\"\nprint(s.split(1))",
        ["expected `str`, found `int`"].as_slice(),
    );
    rejects_main(
        "s = \"a\"\nprint(s.join([1]))",
        ["expected `str`, found `int`"].as_slice(),
    );
    rejects_main(
        "s = \"a\"\nprint(s.nope())",
        ["`str` has no method `nope`", "`str` has `upper`, `lower`"].as_slice(),
    );
}

#[test]
fn strings_are_ordered_and_searched() {
    // DESIGN 3.6: `str` compares in code point order, `in` is a substring test.
    let dump = ok_main(
        "a = \"a\" < \"b\"\nb = \"a\" >= \"b\"\nc = \"b\" in \"abc\"\nd = \"z\" not in \"abc\"",
    );
    for name in ["a", "b", "c", "d"] {
        assert!(dump.contains(&format!("local {name}: bool")), "{dump}");
    }
    assert!(dump.contains("(\"b\":str in \"abc\":str):bool"), "{dump}");
}

#[test]
fn str_converts_every_scalar() {
    let dump = ok_main("a = str(1)\nb = str(1.5)\nc = str(True)\nd = str(\"x\")");
    assert!(dump.contains("a = str(1:int):str"), "{dump}");
    assert!(dump.contains("b = str(1.5:float):str"), "{dump}");
    assert!(dump.contains("c = str(True:bool):str"), "{dump}");
    // `str(s)` on a `str` is the identity, so it leaves no node behind.
    assert!(dump.contains("d = \"x\":str"), "{dump}");
    rejects_main(
        "xs = [1]\nprint(str(xs))",
        ["`str` is not defined for `list<int>`"].as_slice(),
    );
}

#[test]
fn a_str_cannot_be_indexed_by_position() {
    rejects_main(
        "s = \"abc\"\nprint(s[0])",
        ["`str` has no positional indexing"].as_slice(),
    );
    rejects_main(
        "s = \"abc\"\ns[0] = \"x\"",
        [
            "cannot assign through an index into `str`",
            "`str` is immutable",
        ]
        .as_slice(),
    );
}

// ---------------------------------------------------------------------------
// M2: classes (DESIGN 3.8)
// ---------------------------------------------------------------------------

/// A class with a default, a method that calls another one, and a mutator.
const POINT: &str = "class Point:
    x: float
    y: float = 0.0

    fn len2(self) -> float:
        return self.x * self.x + self.y * self.y

    fn norm(self) -> float:
        return self.len2() ** 0.5

    fn shift(self, dx: float):
        self.x = self.x + dx

";

/// A self-referential class, for the `C | None` tests.
const NODE: &str = "class Node:
    value: int
    next: Node | None = None

    fn get(self) -> int:
        return self.value

";

#[test]
fn a_class_has_a_generated_keyword_constructor() {
    let dump = ok(&format!(
        "{POINT}fn main():\n    p = Point(x=3.0, y=4.0)\n    q = Point(y=1.0, x=2.0)\n    r = Point(x=1.0)\n    print(p.x, q.y, r.y)\n"
    ));
    assert!(dump.contains("local p: Point"), "{dump}");
    // Keyword arguments are order independent, and a default is filled in.
    assert!(
        dump.contains("q = Point(x=2.0:float, y=1.0:float):Point"),
        "{dump}"
    );
    assert!(
        dump.contains("r = Point(x=1.0:float, y=0.0:float):Point"),
        "{dump}"
    );
}

#[test]
fn a_method_may_call_another_method_on_self() {
    let dump = ok(&format!(
        "{POINT}fn main():\n    p = Point(x=3.0, y=4.0)\n    print(p.norm())\n"
    ));
    assert!(
        dump.contains("fn Point.norm(self: Point) -> float"),
        "{dump}"
    );
    // `self` is the first argument of an ordinary function.
    assert!(dump.contains("Point.len2(self:Point):float"), "{dump}");
    assert!(dump.contains("Point.norm(p:Point):float"), "{dump}");
}

#[test]
fn fields_are_read_and_written() {
    let dump = ok(&format!(
        "{POINT}fn main():\n    p = Point(x=1.0)\n    p.x = p.x + 1.0\n    p.shift(2.0)\n    print(p.x)\n"
    ));
    assert!(
        dump.contains("p:Point.x = (p:Point.x:float + 1.0:float):float"),
        "{dump}"
    );
    assert!(
        dump.contains("Point.shift(p:Point, 2.0:float):None"),
        "{dump}"
    );
    rejects(
        &format!("{POINT}fn main():\n    p = Point(x=1.0)\n    p.x = 1\n"),
        ["expected `float`, found `int`"].as_slice(),
    );
}

#[test]
fn a_class_field_may_have_any_m2_type() {
    let dump = ok(
        "class Bag:\n    tag: str\n    counts: list<int>\n    pair: tuple<int, bool>\n    flag: bool = False\n\nfn main():\n    b = Bag(tag=\"a\", counts=[1], pair=(1, True))\n    print(b.tag, b.counts, b.pair, b.flag)\n    b.flag = True\n    b.counts.append(2)\n",
    );
    assert!(dump.contains("local b: Bag"), "{dump}");
    assert!(dump.contains("b:Bag.counts:list<int>"), "{dump}");
    assert!(dump.contains("b:Bag.pair:tuple<int, bool>"), "{dump}");
}

#[test]
fn class_instances_have_identity_but_no_equality() {
    let dump = ok(&format!(
        "{POINT}fn main():\n    p = Point(x=1.0)\n    q = Point(x=1.0)\n    print(p is q, p is not q)\n"
    ));
    assert!(dump.contains("(p:Point is q:Point):bool"), "{dump}");
    assert!(dump.contains("(p:Point is not q:Point):bool"), "{dump}");
    rejects(
        &format!("{POINT}fn main():\n    p = Point(x=1.0)\n    print(p == p)\n"),
        [
            "`Point` values cannot be compared with `==`",
            "use `is` to compare identity",
        ]
        .as_slice(),
    );
    rejects(
        &format!("{POINT}fn main():\n    print(Point(x=1.0))\n"),
        ["cannot print a `Point`"].as_slice(),
    );
}

#[test]
fn the_generated_constructor_checks_its_arguments() {
    let program = format!("{POINT}fn main():\n");
    rejects(
        &format!("{program}    p = Point(1.0)\n"),
        [
            "the constructor of `Point` only takes keyword arguments",
            "name the field, e.g. `x=1`",
        ]
        .as_slice(),
    );
    rejects(
        &format!("{program}    p = Point(x=1.0, x=2.0)\n"),
        ["field `x` specified twice"].as_slice(),
    );
    rejects(
        &format!("{program}    p = Point()\n"),
        ["missing field `x` in the constructor of `Point`"].as_slice(),
    );
    rejects(
        &format!("{program}    p = Point(x=1.0, z=1.0)\n"),
        ["`Point` has no field `z`"].as_slice(),
    );
    rejects(
        &format!("{program}    p = Point(x=1)\n"),
        ["expected `float`, found `int`"].as_slice(),
    );
}

#[test]
fn unknown_fields_and_methods_are_rejected() {
    let program = format!("{POINT}fn main():\n    p = Point(x=1.0)\n");
    rejects(
        &format!("{program}    print(p.z)\n"),
        [
            "`Point` has no field `z`",
            "`Point` has the fields `x`, `y`",
        ]
        .as_slice(),
    );
    rejects(
        &format!("{program}    print(p.dist())\n"),
        ["`Point` has no method `dist`"].as_slice(),
    );
    // A field is not a method, and a method is not a field.
    rejects(
        &format!("{program}    print(p.x())\n"),
        ["`Point` has no method `x`", "`x` is a field; drop the `()`"].as_slice(),
    );
    rejects(
        &format!("{program}    print(p.norm)\n"),
        ["`Point` has no field `norm`", "`norm` is a method; call it"].as_slice(),
    );
    rejects_main(
        "n = 1\nprint(n.bit_count())",
        ["`int` has no method `bit_count`"].as_slice(),
    );
}

#[test]
fn a_class_declaration_is_checked() {
    rejects(
        "class Bad:\n    x: None\n\nfn main():\n    pass\n",
        ["the field `x` cannot have type `None`"].as_slice(),
    );
    rejects(
        "class Bad:\n    x: int\n    x: int\n\nfn main():\n    pass\n",
        ["the field `x` is declared twice"].as_slice(),
    );
    rejects(
        "class Bad:\n    x: int = 1.5\n\nfn main():\n    pass\n",
        ["expected `int`, found `float`"].as_slice(),
    );
    rejects(
        "class Bad:\n    x: int\n\n    fn f() -> int:\n        return 1\n\nfn main():\n    pass\n",
        ["the method `f` must take `self` as its first parameter"].as_slice(),
    );
    rejects(
        "class Bad:\n    x: int\n\n    fn f(self) -> int:\n        return 1\n\n    fn f(self) -> int:\n        return 2\n\nfn main():\n    pass\n",
        ["the method `f` is declared twice on `Bad`"].as_slice(),
    );
}

#[test]
fn containers_of_class_instances_are_neither_printed_nor_compared() {
    ok_main("print([1, 2], [[1]], (1, \"a\"), [\"a\"])");
    let program = format!("{POINT}fn main():\n    ps = [Point(x=1.0)]\n");
    rejects(
        &format!("{program}    print(ps)\n"),
        ["cannot print a `list<Point>`"].as_slice(),
    );
    rejects(
        &format!("{program}    print(ps == ps)\n"),
        ["`list<Point>` values cannot be compared"].as_slice(),
    );
    rejects(
        &format!("{POINT}fn main():\n    print((1, Point(x=1.0)))\n"),
        ["cannot print a `tuple<int, Point>`"].as_slice(),
    );
    rejects_main(
        "xs = [1]\nprint(f\"{xs}\")",
        ["cannot interpolate a `list<int>`"].as_slice(),
    );
}

// ---------------------------------------------------------------------------
// M2: `C | None` and narrowing (DESIGN 3.8)
// ---------------------------------------------------------------------------

#[test]
fn none_and_a_class_both_convert_to_an_optional() {
    let dump = ok(&format!(
        "{NODE}fn main():\n    empty: Node | None = None\n    one = Node(value=1)\n    some: Node | None = one\n    linked = Node(value=2, next=one)\n    print(empty is None, some is None, linked.value)\n"
    ));
    assert!(dump.contains("local empty: Node | None"), "{dump}");
    assert!(dump.contains("local some: Node | None"), "{dump}");
    // `None` becomes the null reference, and a `Node` widens without a cast.
    assert!(dump.contains("empty = None:Node | None"), "{dump}");
    assert!(dump.contains("some = one:Node | None"), "{dump}");
    assert!(
        dump.contains("linked = Node(value=2:int, next=one:Node | None):Node"),
        "{dump}"
    );
}

#[test]
fn is_none_narrows_in_both_arms_of_an_if() {
    let dump = ok(&format!(
        "{NODE}fn main():\n    n: Node | None = Node(value=1)\n    if n is None:\n        print(0)\n    else:\n        print(n.value)\n    if n is not None:\n        print(n.get())\n"
    ));
    assert!(dump.contains("if (n:Node | None is None):bool:"), "{dump}");
    // Inside the narrowed arm the local reads as a plain `Node`.
    assert!(dump.contains("n:Node.value:int"), "{dump}");
    assert!(dump.contains("Node.get(n:Node):int"), "{dump}");
}

#[test]
fn an_early_return_narrows_the_rest_of_the_function() {
    let dump = ok(&format!(
        "{NODE}fn head(n: Node | None) -> int:\n    if n is None:\n        return 0\n    return n.value\n\nfn main():\n    print(head(Node(value=1)))\n    print(head(None))\n"
    ));
    assert!(dump.contains("return n:Node.value:int"), "{dump}");
    assert!(dump.contains("head(None:Node | None):int"), "{dump}");
}

#[test]
fn an_optional_must_be_narrowed_before_it_is_used() {
    let program = format!("{NODE}fn main():\n    n: Node | None = Node(value=1)\n");
    for snippet in ["print(n.value)", "print(n.get())", "n.value = 2"] {
        rejects(
            &format!("{program}    {snippet}\n"),
            [
                "`Node | None` may be `None` here",
                "this is `Node | None`, not `Node`",
            ]
            .as_slice(),
        );
    }
    // The `else` arm of `is not None` knows nothing.
    rejects(
        &format!(
            "{program}    if n is not None:\n        print(n.value)\n    else:\n        print(n.value)\n"
        ),
        ["`Node | None` may be `None` here"].as_slice(),
    );
}

#[test]
fn a_loop_that_reassigns_the_local_forgets_the_narrowing() {
    let program = format!("{NODE}fn main():\n    cur: Node | None = Node(value=1)\n");
    // DESIGN 3.8: the body may run again after the assignment, so what the
    // condition proved on entry no longer holds anywhere inside it.
    rejects(
        &format!(
            "{program}    while cur is not None:\n        print(cur.value)\n        cur = cur.next\n"
        ),
        ["`Node | None` may be `None` here"].as_slice(),
    );
    rejects(
        &format!(
            "{program}    if cur is not None:\n        for i in range(2):\n            print(cur.value)\n            cur = None\n"
        ),
        ["`Node | None` may be `None` here"].as_slice(),
    );
    // A body that never touches the local keeps what the `if` proved.
    ok(&format!(
        "{program}    if cur is not None:\n        for i in range(2):\n            print(cur.value)\n"
    ));
    ok(&format!(
        "{program}    if cur is not None:\n        while cur.value > 0:\n            print(cur.value)\n            break\n"
    ));
}

#[test]
fn only_a_class_may_be_optional_in_m2() {
    rejects_main(
        "x: int | None = None",
        [
            "an optional type (`int | None`) is not supported yet (planned for M3)",
            "only a class type may be optional in M2",
        ]
        .as_slice(),
    );
    rejects(
        "fn f(a: str | None) -> int:\n    return 1\n\nfn main():\n    print(f(None))\n",
        ["an optional type (`str | None`) is not supported yet"].as_slice(),
    );
    rejects(
        "class Bad:\n    xs: list<int> | None\n\nfn main():\n    pass\n",
        ["an optional type (`list<int> | None`) is not supported yet"].as_slice(),
    );
}

// ---------------------------------------------------------------------------
// M2: `for x in <list>` and `for c in <str>` (DESIGN 3.5)
// ---------------------------------------------------------------------------

#[test]
fn the_foreach_loop_variable_has_the_element_type() {
    let dump = ok_main(
        "xs = [1, 2]\nfor x in xs:\n    print(x + 1)\nfor c in \"ab\":\n    print(c.upper())\ngrid = [[1]]\nfor row in grid:\n    print(len(row))",
    );
    assert!(dump.contains("local x: int"), "{dump}");
    assert!(dump.contains("local c: str"), "{dump}");
    assert!(dump.contains("local row: list<int>"), "{dump}");
    assert!(dump.contains("for x in list xs:list<int>:"), "{dump}");
    assert!(dump.contains("for c in str \"ab\":str:"), "{dump}");
}

#[test]
fn only_a_list_a_str_or_a_range_can_be_iterated() {
    rejects_main(
        "for x in 5:\n    print(x)",
        [
            "cannot iterate over an `int`",
            "`for` walks a `range(...)`, a `list<T>` or a `str`",
        ]
        .as_slice(),
    );
    rejects_main(
        "t = (1, 2)\nfor x in t:\n    print(x)",
        ["cannot iterate over a `tuple<int, int>`"].as_slice(),
    );
    // The element type has to match a loop variable that already exists.
    rejects_main(
        "x = 1\nfor x in [\"a\"]:\n    print(x)",
        ["mismatched types", "expected `int`, found `str`"].as_slice(),
    );
    // The body of a rejected loop is still checked.
    rejects_main(
        "for x in 5:\n    print(nope)",
        ["cannot iterate over an `int`", "cannot find value `nope`"].as_slice(),
    );
}

#[test]
fn the_foreach_variable_is_an_ordinary_local() {
    ok_main("x = 0\nfor x in [1, 2]:\n    print(x)\nprint(x)");
    rejects_main(
        "for x in [1, 2]:\n    print(x)\nprint(x)",
        ["use of possibly-uninitialized variable `x`"].as_slice(),
    );
}

#[test]
fn a_foreach_body_may_not_run() {
    // DESIGN 3.4: the M1 rule holds for the new loop form too.
    rejects_main(
        "xs = [1]\nfor x in xs:\n    last = x\nprint(last)",
        ["use of possibly-uninitialized variable `last`"].as_slice(),
    );
    rejects_main(
        "for c in \"ab\":\n    seen = c\nprint(seen)",
        ["use of possibly-uninitialized variable `seen`"].as_slice(),
    );
    ok_main("xs = [1]\nlast = -1\nfor x in xs:\n    last = x\nprint(last)");
    // `break` and `continue` work in a `for … in` loop as well.
    ok_main("for x in [1, 2]:\n    if x == 1:\n        continue\n    break");
}

// ---------------------------------------------------------------------------
// M2: tuple unpacking (DESIGN 4.2)
// ---------------------------------------------------------------------------

#[test]
fn a_tuple_unpacks_into_new_and_existing_locals() {
    let dump = ok_main("t = (1, \"a\")\na, b = t\nc, d = 1, 2\nc, d = d, c\nprint(a, b, c, d)");
    assert!(dump.contains("local a: int"), "{dump}");
    assert!(dump.contains("local b: str"), "{dump}");
    assert!(dump.contains("local c: int"), "{dump}");
    assert!(dump.contains("local d: int"), "{dump}");
    // The right-hand side is evaluated once, into a compiler-made temporary.
    assert!(dump.contains("local unpack."), "{dump}");
}

#[test]
fn unpacking_checks_the_shape() {
    rejects_main(
        "t = (1, 2)\na, b, c = t",
        ["expected 3 values to unpack, found 2"].as_slice(),
    );
    rejects_main(
        "t = (1, 2, 3)\na, b = t",
        ["expected 2 values to unpack, found 3"].as_slice(),
    );
    rejects_main(
        "xs = [1, 2]\na, b = xs",
        [
            "cannot unpack a `list<int>`",
            "only a tuple can be unpacked",
        ]
        .as_slice(),
    );
    rejects_main("a, b = 1", ["cannot unpack an `int`"].as_slice());
    // The member types have to match locals that already exist.
    rejects_main(
        "a = 1\nb = 2\nt = (1, \"x\")\na, b = t",
        ["expected `int`, found `str`"].as_slice(),
    );
}

// ---------------------------------------------------------------------------
// M2: lowering
// ---------------------------------------------------------------------------

#[test]
fn a_foreach_loop_records_what_it_walks() {
    let dump = ok_main(
        "total = 0\nfor n in [1, 2, 3]:\n    total = total + n\nfor c in \"ab\":\n    print(c)",
    );
    insta::assert_snapshot!(dump);
}

#[test]
fn unpacking_lowers_to_a_temporary_and_member_reads() {
    let dump = ok_main("t = (1, \"a\")\nn, s = t");
    insta::assert_snapshot!(dump);
}

#[test]
fn a_class_lowers_to_a_constructor_call_and_plain_functions() {
    let dump = ok(
        "class Counter:\n    n: int = 0\n\n    fn bump(self, by: int):\n        self.n = self.n + by\n\n    fn value(self) -> int:\n        return self.n\n\nfn main():\n    c = Counter()\n    c.bump(2)\n    print(c.value())\n",
    );
    insta::assert_snapshot!(dump);
}

#[test]
fn narrowing_lowers_to_a_null_test() {
    let dump = ok(
        "class Node:\n    value: int\n\nfn head(n: Node | None) -> int:\n    if n is None:\n        return 0\n    return n.value\n\nfn main():\n    print(head(None))\n",
    );
    insta::assert_snapshot!(dump);
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
    snapshot_unknown_field,
    "class Point:\n    x: float\n\nfn main():\n    p = Point(x=1.0)\n    print(p.z)\n"
);
snapshot_error!(
    snapshot_missing_constructor_field,
    "class Point:\n    x: float\n    y: float\n\nfn main():\n    p = Point(x=1.0)\n    print(p.x)\n"
);
snapshot_error!(
    snapshot_optional_needs_narrowing,
    "class Node:\n    next: Node | None\n\nfn main():\n    n = Node(next=None)\n    print(n.next.next)\n"
);
snapshot_error!(
    snapshot_empty_list_without_annotation,
    "fn main():\n    xs = []\n    print(len(xs))\n"
);
snapshot_error!(
    snapshot_printing_a_class_instance,
    "class Point:\n    x: float\n\nfn main():\n    print(Point(x=1.0))\n"
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
snapshot_error!(
    snapshot_constructor_takes_keywords_only,
    "class Point:\n    x: float\n    y: float\n\nfn main():\n    p = Point(1.0, y=2.0)\n    print(p.x)\n"
);
snapshot_error!(
    snapshot_unknown_method,
    "class Point:\n    x: float\n\n    fn norm(self) -> float:\n        return self.x\n\nfn main():\n    p = Point(x=1.0)\n    print(p.dist())\n"
);
snapshot_error!(
    snapshot_list_element_mismatch,
    "fn main():\n    xs = [1, 2.0, 3]\n    print(xs)\n"
);
snapshot_error!(
    snapshot_tuple_index_out_of_range,
    "fn main():\n    t = (1, \"a\")\n    print(t[2])\n"
);
snapshot_error!(
    snapshot_unpack_arity_mismatch,
    "fn main():\n    t = (1, 2)\n    a, b, c = t\n"
);
snapshot_error!(
    snapshot_unpack_of_a_non_tuple,
    "fn main():\n    xs = [1, 2]\n    a, b = xs\n"
);
snapshot_error!(
    snapshot_narrowing_lost_in_a_loop,
    "class Node:\n    value: int\n    next: Node | None = None\n\nfn main():\n    cur: Node | None = Node(value=1)\n    while cur is not None:\n        cur = cur.next\n"
);
snapshot_error!(
    snapshot_optional_of_a_non_class,
    "fn main():\n    n: int | None = None\n    print(n)\n"
);
snapshot_error!(
    snapshot_str_has_no_positional_indexing,
    "fn main():\n    s = \"abc\"\n    print(s[0])\n"
);
snapshot_error!(
    snapshot_iterating_an_int,
    "fn main():\n    for x in 5:\n        pass\n"
);
