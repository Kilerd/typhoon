//! Differential tests: the same program, both backends, byte-identical output.
//!
//! The golden suite already runs every file in `tests/run` and `examples` on
//! both backends (DESIGN §7.1, §6.2.1). This file goes after the corners those
//! programs do not reach — `i64::MIN // -1`, a shift of 70, NaN on both sides
//! of every comparison, saturating `int(1e300)`, `-0.0`, a one-member tuple —
//! because that is where two independent lowerings of the same semantics
//! actually drift apart.
//!
//! Each case compiles twice: to a native binary through LLVM and `clang`, and
//! to a wasm module run under `wasmtime`. The assertion is that stdout and the
//! exit status match each other exactly; the expected text is written down too,
//! so that a case where *both* backends are wrong still fails.

mod common;

use common::wasm::{compile_and_run, is_available};
use common::{Scratch, ensure_runtime_lib, unified_diff};
use typhoon_driver::{BuildOptions, compile_source, run_binary};

/// One differential case: a program, and what both backends must print.
struct Case {
    name: &'static str,
    source: &'static str,
    expect: &'static str,
    exit: i32,
}

/// Runs `source` natively: compile, link, execute.
fn run_native(name: &str, source: &str) -> (i32, String) {
    ensure_runtime_lib();
    let scratch = Scratch::new("differential");
    let opts = BuildOptions {
        output: Some(scratch.join(name)),
        ..BuildOptions::default()
    };
    let artifact = compile_source(&format!("{name}.ty"), source, &opts)
        .unwrap_or_else(|err| panic!("{name} did not compile natively:\n{err}"));
    let output = run_binary(artifact.binary(), &[])
        .unwrap_or_else(|err| panic!("{name} did not run natively: {err}"));
    (output.status, output.stdout)
}

fn check(case: &Case) {
    let (native_status, native_stdout) = run_native(case.name, case.source);
    assert_eq!(
        native_stdout,
        case.expect,
        "{}: the native backend printed something unexpected\n{}",
        case.name,
        unified_diff(case.expect, &native_stdout)
    );
    assert_eq!(
        native_status, case.exit,
        "{}: native exit status",
        case.name
    );

    if !is_available() {
        return;
    }
    let wasm = compile_and_run(&format!("{}.ty", case.name), case.source)
        .unwrap_or_else(|err| panic!("{} did not run on wasm: {err}", case.name));
    assert_eq!(
        wasm.stdout,
        native_stdout,
        "{}: the two backends disagree\n{}",
        case.name,
        unified_diff(&native_stdout, &wasm.stdout)
    );
    assert_eq!(
        wasm.status, native_status,
        "{}: exit status differs (native {native_status}, wasm {})\nstderr: {}",
        case.name, wasm.status, wasm.stderr
    );
}

macro_rules! differential {
    ($name:ident, $source:expr, $expect:expr) => {
        differential!($name, $source, $expect, 0);
    };
    ($name:ident, $source:expr, $expect:expr, $exit:expr) => {
        #[test]
        fn $name() {
            check(&Case {
                name: stringify!($name),
                source: $source,
                expect: $expect,
                exit: $exit,
            });
        }
    };
}

// -- integers ---------------------------------------------------------------

differential!(
    int_min_division_wraps,
    "\
fn div(a: int, b: int) -> int:
    return a // b

fn rem(a: int, b: int) -> int:
    return a % b

fn main():
    lo = -9223372036854775807 - 1
    print(div(lo, -1))
    print(rem(lo, -1))
    print(lo // -1, lo % -1)
    print(-lo)
    print(abs(lo))
    print(lo * -1)
",
    "-9223372036854775808\n0\n-9223372036854775808 0\n-9223372036854775808\n-9223372036854775808\n-9223372036854775808\n"
);

differential!(
    flooring_division_and_modulo_signs,
    "\
fn show(a: int, b: int):
    print(a // b, a % b, a / b)

fn main():
    show(7, 2)
    show(-7, 2)
    show(7, -2)
    show(-7, -2)
    show(6, 3)
    show(-6, 3)
    show(0, 5)
",
    "3 1 3.5\n-4 1 -3.5\n-4 -1 -3.5\n3 -1 3.5\n2 0 2.0\n-2 0 -2.0\n0 0 0.0\n"
);

differential!(
    shifts_beyond_the_word,
    "\
fn shl(a: int, b: int) -> int:
    return a << b

fn shr(a: int, b: int) -> int:
    return a >> b

fn main():
    print(1 << 62, 1 << 63)
    print(shl(1, 64), shl(1, 70), shl(-1, 64))
    print(shr(-1, 70), shr(1024, 70), shr(-1024, 3))
    print(1 << 64, 5 >> 64, -5 >> 64)
    print(~0, ~-1, 5 & 3, 5 | 8, 5 ^ 1)
",
    "4611686018427387904 -9223372036854775808\n0 0 0\n-1 0 -128\n0 0 -1\n-1 0 1 13 4\n"
);

differential!(
    a_negative_shift_panics,
    "\
fn shl(a: int, b: int) -> int:
    return a << b

fn main():
    print(1)
    print(shl(1, -1))
",
    "1\n",
    101
);

differential!(
    integer_powers,
    "\
fn pow(a: int, b: int) -> int:
    return a ** b

fn main():
    print(2 ** 10, 2 ** 62, 0 ** 0, (0 - 2) ** 3)
    # `3 ** 40` overflows an i64 and wraps (DESIGN 4.3).
    print(pow(2, 63), pow(3, 40), pow(1, 100))
    n = 5
    print(n ** 2, n ** 3)
",
    "1024 4611686018427387904 1 -8\n-9223372036854775808 -6289078614652622815 1\n25 125\n"
);

// -- floats -----------------------------------------------------------------

differential!(
    float_edge_values,
    "\
fn div(a: float, b: float) -> float:
    return a / b

fn main():
    zero = 0.0
    print(div(1.0, zero), div(-1.0, zero))
    nan = div(zero, zero)
    print(nan)
    print(nan == nan, nan != nan, nan < 1.0, nan > 1.0, nan <= nan)
    # `min`/`max` select on an ordered comparison, so a NaN on the left wins
    # and a NaN on the right loses; wasm's own f64.min/f64.max would propagate
    # the NaN in both directions instead.
    print(min(nan, 1.0), max(nan, 1.0), min(1.0, nan), max(1.0, nan))
    print(-zero, zero == -zero)
    print(1e308 * 10.0)
",
    "inf -inf\nnan\nFalse True False False False\nnan nan 1.0 1.0\n-0.0 True\ninf\n"
);

differential!(
    float_division_and_modulo_signs,
    "\
fn show(a: float, b: float):
    print(a // b, a % b)

fn main():
    show(7.0, 2.0)
    show(-7.0, 2.0)
    show(7.0, -2.0)
    show(-7.0, -2.0)
    show(4.0, -2.0)
    show(-4.0, 2.0)
    show(5.5, 2.5)
",
    "3.0 1.0\n-4.0 1.0\n-4.0 -1.0\n3.0 -1.0\n-2.0 -0.0\n-2.0 0.0\n2.0 0.5\n"
);

differential!(
    float_powers_and_roots,
    "\
fn pow(a: float, b: float) -> float:
    return a ** b

fn main():
    x = 2.0
    print(x ** 0.5, x ** 2.0, x ** 3.0)
    print(pow(9.0, 0.5), pow(2.0, 1024.0), pow(0.0, 0.0))
    print(pow(-8.0, 2.0), pow(1.0, 0.0))
",
    "1.4142135623730951 4.0 8.0\n3.0 inf 1.0\n64.0 1.0\n"
);

differential!(
    conversions_saturate,
    "\
fn to_int(x: float) -> int:
    return int(x)

fn main():
    print(to_int(1.9), to_int(-1.9), to_int(0.0))
    print(to_int(1e300), to_int(-1e300))
    zero = 0.0
    print(to_int(zero / zero))
    print(float(3), float(-3))
    big = 9007199254740993
    print(float(big))
",
    "1 -1 0\n9223372036854775807 -9223372036854775808\n0\n3.0 -3.0\n9007199254740992.0\n"
);

differential!(
    float_repr_matches,
    "\
fn main():
    print(0.1 + 0.2)
    print(1.0, 100.0, 1e16, 1e17, 1e-5, 1.5e-7)
    print(1.0 / 3.0)
    print(f\"{1.0 / 3.0:.6f} {0.5:.0f} {2.675:.2f}\")
",
    "0.30000000000000004\n1.0 100.0 1e+16 1e+17 1e-05 1.5e-07\n0.3333333333333333\n0.333333 0 2.67\n"
);

// -- containers -------------------------------------------------------------

differential!(
    list_indexing_at_the_boundaries,
    "\
fn main():
    xs = [10, 20, 30]
    print(xs[0], xs[2], xs[-1], xs[-3])
    xs.insert(-100, 5)
    xs.insert(100, 40)
    print(xs)
    xs[-1] = 41
    print(xs, len(xs))
    print(xs.pop(), xs)
    ys: list<int> = []
    ys.append(1)
    print(ys + xs, [1, 2] == [1, 2], [1, 2] == [1, 3])
",
    "10 30 30 10\n[5, 10, 20, 30, 40]\n[5, 10, 20, 30, 41] 5\n41 [5, 10, 20, 30]\n[1, 5, 10, 20, 30] True False\n"
);

differential!(
    an_out_of_range_index_panics,
    "\
fn get(xs: list<int>, i: int) -> int:
    return xs[i]

fn main():
    xs = [1, 2]
    print(get(xs, -2))
    print(get(xs, -3))
",
    "1\n",
    101
);

differential!(
    tuples_nested_and_printed,
    "\
fn one() -> tuple<int>:
    return (7,)

fn nested() -> tuple<int, tuple<bool, str>>:
    return (1, (True, \"x\"))

fn main():
    print(one(), one() == (7,))
    print(nested())
    a, b = nested()
    print(a, b)
    print(nested() == (1, (True, \"x\")), nested() != (1, (False, \"x\")))
    pairs = [(1, True), (2, False)]
    print(pairs, pairs[1], pairs == [(1, True), (2, False)])
",
    "(7,) True\n(1, (True, \"x\"))\n1 (True, \"x\")\nTrue True\n[(1, True), (2, False)] (2, False) True\n"
);

differential!(
    tuples_as_parameters_fields_and_elements,
    "\
class Pair:
    p: tuple<int, bool>
    n: int

fn add(p: tuple<int, int>, q: tuple<int, int>) -> tuple<int, int>:
    a, b = p
    c, d = q
    return (a + c, b + d)

fn label(t: tuple<tuple<int, bool>, str>) -> str:
    inner, name = t
    v, flag = inner
    return f\"{name}:{v}:{flag}\"

fn main():
    print(add((1, 2), (3, 4)))
    print(label(((7, True), \"x\")))
    box = Pair(p=(1, False), n=9)
    print(box.p, box.n)
    box.p = (2, True)
    print(box.p, box.p == (2, True))
    grid = [(1, (2, 3)), (4, (5, 6))]
    print(grid, grid[1], grid[0] == (1, (2, 3)))
",
    "(4, 6)\nx:7:True\n(1, False) 9\n(2, True) True\n[(1, (2, 3)), (4, (5, 6))] (4, (5, 6)) True\n"
);

differential!(
    bools_are_one_byte_in_containers,
    "\
class Flags:
    a: bool
    n: int
    b: bool

fn main():
    flags = [True, False, True]
    print(flags, flags[0] and flags[2], flags[1] or flags[0])
    f = Flags(a=True, n=-1, b=False)
    print(f.a, f.n, f.b)
    f.b = True
    print(f.a and f.b, (True, False, True))
    print(True in flags, False in flags)
",
    "[True, False, True] True True\nTrue -1 False\nTrue (True, False, True)\nTrue True\n"
);

differential!(
    unicode_strings,
    "\
fn main():
    s = \"héllo, 台风!\"
    print(s, len(s))
    # `upper` is ASCII-only (DESIGN 4.6), so the `é` is left alone.
    print(s.upper(), s.find(\"台\"), \"台\" in s)
    print(s.replace(\"台风\", \"typhoon\"))
    for ch in \"aé台\":
        print(ch, len(ch))
    parts = \"a,é,台\".split(\",\")
    print(parts, \"-\".join(parts))
    print(\"a\" < \"b\", \"é\" < \"台\", \"\" == \"\")
",
    "héllo, 台风! 10\nHéLLO, 台风! 7 True\nhéllo, typhoon!\na 1\né 1\n台 1\n[\"a\", \"é\", \"台\"] a-é-台\nTrue True True\n"
);

differential!(
    class_identity_and_aliasing,
    "\
class Node:
    value: int
    tag: str = \"n\"

fn main():
    a = Node(value=1)
    b = a
    c = Node(value=1)
    b.value = 2
    print(a.value, b.value, c.value)
    print(a is b, a is not c, a is c)
    print(a.tag, c.tag)
",
    "2 2 1\nTrue True False\nn n\n"
);

differential!(
    an_optional_class_reference_is_a_null_pointer,
    "\
class Node:
    value: int

fn pick(flag: bool) -> Node | None:
    if flag:
        return Node(value=7)
    return None

fn describe(n: Node | None) -> str:
    if n is None:
        return \"none\"
    return \"some\"

fn main():
    a = pick(True)
    b = pick(False)
    print(describe(a), describe(b))
    print(a is None, b is None, a is not None)
    if a is not None:
        print(a.value)
",
    "some none\nFalse True True\n7\n"
);

// -- control flow and calls --------------------------------------------------

differential!(
    argument_evaluation_order,
    "\
fn note(n: int) -> int:
    print(n)
    return n

fn three(a: int, b: int, c: int = 9) -> int:
    return a * 100 + b * 10 + c

fn main():
    print(three(b=note(1), a=note(2)))
    print(three(note(3), note(4), note(5)))
",
    "1\n2\n219\n3\n4\n5\n345\n"
);

differential!(
    loops_with_every_exit,
    "\
fn main():
    total = 0
    for i in range(10, 0, -3):
        if i == 4:
            continue
        total = total + i
    print(total)
    step = -3
    for j in range(10, 0, step):
        total = total + j
    print(total)
    i = 0
    while True == True:
        i = i + 1
        if i > 3:
            break
    print(i)
    xs = [1, 2, 3, 4]
    seen = 0
    for x in xs:
        if x == 2:
            continue
        if x == 4:
            break
        seen = seen + x
    print(seen)
",
    "18\n40\n4\n4\n"
);

differential!(
    a_zero_range_step_panics,
    "\
fn main():
    print(1)
    step = 0
    for i in range(0, 10, step):
        print(i)
",
    "1\n",
    101
);

differential!(
    short_circuit_evaluation_order,
    "\
fn note(n: int, value: bool) -> bool:
    print(n)
    return value

fn main():
    print(note(1, False) and note(2, True))
    print(note(3, True) or note(4, False))
    print(note(5, True) and note(6, False))
    print(not note(7, True))
",
    "1\nFalse\n3\nTrue\n5\n6\nFalse\n7\nFalse\n"
);
