//! User-defined generics (DESIGN 3.9, 4.4, 4.5): what monomorphization
//! produces, how type arguments are inferred, and the diagnostics of generic
//! code, including the instantiation stack.

use typhoon_diag::{Diagnostics, SourceMap, render_all};
use typhoon_sema::hir::Program;
use typhoon_sema::{check_module, dump_program};

/// Type-checks `src`, returning the program or the rendered diagnostics.
fn check(src: &str) -> Result<Program, String> {
    let mut sources = SourceMap::new();
    let file = sources.add("main.ty", src);
    let mut diags = Diagnostics::new();
    let module = typhoon_parser::parse_module(file, src, &mut diags);
    assert!(
        !diags.has_errors(),
        "the test program must parse:\n{}",
        render_all(&diags, &sources, false)
    );
    check_module(&module, file, &mut diags).ok_or_else(|| render_all(&diags, &sources, false))
}

/// Type-checks `src`, which must succeed, and dumps its IR.
#[track_caller]
fn ok(src: &str) -> String {
    match check(src) {
        Ok(program) => dump_program(&program),
        Err(err) => panic!("expected the program to check:\n{err}"),
    }
}

/// Type-checks `src`, which must fail with diagnostics mentioning every
/// `wanted` substring; returns the rendered diagnostics.
#[track_caller]
fn rejects(src: &str, wanted: &[&str]) -> String {
    let rendered = match check(src) {
        Ok(_) => panic!("expected the program to be rejected"),
        Err(rendered) => rendered,
    };
    for want in wanted {
        assert!(
            rendered.contains(want),
            "diagnostics do not mention {want:?}:\n{rendered}"
        );
    }
    rendered
}

/// The number of `error:` headlines.
fn errors(rendered: &str) -> usize {
    rendered.matches("error: ").count()
}

const MAX: &str = "fn max<T>(a: T, b: T) -> T:\n    if a > b:\n        return a\n    return b\n\n";

const STACK: &str = "class Stack<T>:
    items: list<T>

    fn push(self, value: T):
        self.items.append(value)

    fn pop(self) -> T:
        return self.items.pop()

    fn size(self) -> int:
        return len(self.items)

";

fn with_max(main_body: &str) -> String {
    format!("{MAX}fn main():\n{}", indent(main_body))
}

fn with_stack(main_body: &str) -> String {
    format!("{STACK}fn main():\n{}", indent(main_body))
}

fn indent(body: &str) -> String {
    body.lines().map(|l| format!("    {l}\n")).collect()
}

// ---------------------------------------------------------------------------
// Monomorphization
// ---------------------------------------------------------------------------

#[test]
fn every_instance_is_an_ordinary_function() {
    let dump = ok(&with_max("print(max(1, 3))\nprint(max<float>(2.5, 1.5))"));
    assert!(
        dump.contains("fn max<int>(a: int, b: int) -> int"),
        "{dump}"
    );
    assert!(
        dump.contains("fn max<float>(a: float, b: float) -> float"),
        "{dump}"
    );
    // Calls name the instance, and each body is lowered with the concrete
    // operator of its instance.
    assert!(dump.contains("max<int>(1:int, 3:int):int"), "{dump}");
    assert!(dump.contains("(a:float > b:float):bool"), "{dump}");
}

#[test]
fn instances_are_memoized() {
    let dump = ok(&with_max(
        "print(max(1, 3))\nprint(max(4, 2))\nprint(max<int>(5, 6))",
    ));
    assert_eq!(dump.matches("fn max<int>(").count(), 1, "{dump}");
}

#[test]
fn instance_symbols_are_plain_identifiers_that_cannot_collide() {
    let src = "fn id<T>(x: T) -> T:\n    return x\n\n\
               fn main():\n    print(id(1))\n    print(id([1]))\n    print(id((1, \"a\")))\n";
    let program = check(src).unwrap();
    let symbol = |name: &str| {
        program
            .functions
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("no function {name}"))
            .symbol
            .clone()
    };
    assert_eq!(symbol("id<int>"), "ty_user_id.Lint.G");
    assert_eq!(symbol("id<list<int>>"), "ty_user_id.Llist.Lint.G.G");
    assert_eq!(
        symbol("id<tuple<int, str>>"),
        "ty_user_id.Ltuple.Lint.Cstr.G.G"
    );
    assert_eq!(symbol("main"), "ty_user_main");
}

#[test]
fn unused_templates_and_methods_produce_nothing() {
    let dump = ok(&format!(
        "{MAX}{STACK}fn main():\n    s: Stack<int> = Stack(items=[])\n    s.push(1)\n"
    ));
    assert!(
        dump.contains("fn Stack<int>.push(self: Stack<int>, value: int)"),
        "{dump}"
    );
    assert!(!dump.contains("pop"), "{dump}");
    assert!(!dump.contains("size"), "{dump}");
    assert!(!dump.contains("fn max"), "{dump}");
}

#[test]
fn a_generic_function_calls_a_generic_function() {
    let src = format!(
        "{MAX}fn max3<T>(a: T, b: T, c: T) -> T:\n    return max(max(a, b), c)\n\n\
         fn main():\n    print(max3(1, 2, 3))\n    print(max3(\"a\", \"b\", \"c\"))\n"
    );
    let dump = ok(&src);
    for name in ["max3<int>", "max<int>", "max3<str>", "max<str>"] {
        assert!(dump.contains(&format!("fn {name}(")), "{name}:\n{dump}");
    }
}

#[test]
fn class_instances_are_distinct_classes() {
    let dump = ok(&with_stack(
        "a: Stack<int> = Stack(items=[])\nb: Stack<str> = Stack(items=[])\na.push(1)\nb.push(\"x\")",
    ));
    assert!(dump.contains("local a: Stack<int>"), "{dump}");
    assert!(dump.contains("local b: Stack<str>"), "{dump}");
    assert!(
        dump.contains("fn Stack<str>.push(self: Stack<str>, value: str)"),
        "{dump}"
    );

    rejects(
        &with_stack("a: Stack<int> = Stack(items=[])\nb: Stack<str> = a"),
        &["mismatched types: expected `Stack<str>`, found `Stack<int>`"],
    );
}

#[test]
fn nested_type_arguments_split_the_shift_token() {
    let dump = ok(&with_stack(
        "s: Stack<list<int>>= Stack(items=[])\ns.push([1])\nt: Stack<tuple<str, list<int>>> = Stack(items=[])",
    ));
    assert!(dump.contains("local s: Stack<list<int>>"), "{dump}");
    assert!(
        dump.contains("local t: Stack<tuple<str, list<int>>>"),
        "{dump}"
    );
}

#[test]
fn a_self_referential_generic_class() {
    let src = "class Node<T>:\n    value: T\n    next: Node<T> | None = None\n\n\
               fn main():\n    n = Node(value=1, next=Node(value=2))\n    print(n.value)\n";
    let dump = ok(src);
    assert!(dump.contains("local n: Node<int>"), "{dump}");
}

// ---------------------------------------------------------------------------
// Inference (DESIGN 4.4)
// ---------------------------------------------------------------------------

#[test]
fn type_arguments_come_from_the_arguments() {
    let src = "fn first<A, B>(pair: tuple<A, B>) -> A:\n    return pair[0]\n\n\
               fn main():\n    x = first((1, \"a\"))\n    y = first((\"b\", 2.5))\n";
    let dump = ok(src);
    assert!(dump.contains("local x: int"), "{dump}");
    assert!(dump.contains("local y: str"), "{dump}");
    assert!(dump.contains("fn first<int, str>("), "{dump}");
}

#[test]
fn type_arguments_come_from_the_expected_type() {
    let src = "fn empty<T>() -> list<T>:\n    xs: list<T> = []\n    return xs\n\n\
               fn main():\n    xs: list<str> = empty()\n    print(xs)\n";
    let dump = ok(src);
    assert!(dump.contains("fn empty<str>() -> list<str>"), "{dump}");

    // `[]` is checked last, once the annotation has fixed `T`.
    let dump = ok(&with_stack("s: Stack<float> = Stack(items=[])"));
    assert!(dump.contains("local s: Stack<float>"), "{dump}");
}

#[test]
fn constructor_type_arguments_come_from_the_fields() {
    let dump = ok(&with_stack(
        "s = Stack(items=[1.5])\nt = Stack<str>(items=[])",
    ));
    assert!(dump.contains("local s: Stack<float>"), "{dump}");
    assert!(dump.contains("local t: Stack<str>"), "{dump}");
}

#[test]
fn an_optional_parameter_binds_the_class() {
    let src = "class Node:\n    v: int\n\n\
               fn or_else<T>(x: T | None, fallback: T) -> T:\n    if x is None:\n        return fallback\n    return x\n\n\
               fn main():\n    n: Node | None = None\n    print(or_else(n, Node(v=1)).v)\n";
    let dump = ok(src);
    assert!(
        dump.contains("fn or_else<Node>(x: Node | None, fallback: Node) -> Node"),
        "{dump}"
    );
}

#[test]
fn an_optional_parameter_binds_any_type() {
    // DESIGN 3.10: every `T` may be optional, so `T` is inferred through
    // `T | None` from an `int | None`, a `str` or a `list<float | None>`.
    let src = "fn or_else<T>(x: T | None, fallback: T) -> T:\n    if x is None:\n        return fallback\n    return x\n\n\
               fn present<T>(xs: list<T | None>) -> list<T>:\n    return [x for x in xs if x is not None]\n\n\
               fn main():\n    n: int | None = None\n    fs: list<float | None> = [1.5, None]\n    print(or_else(n, 1), or_else(\"s\", \"t\"), present(fs))\n";
    let dump = ok(src);
    for signature in [
        "fn or_else<int>(x: int | None, fallback: int) -> int",
        "fn or_else<str>(x: str | None, fallback: str) -> str",
        "fn present<float>(xs: list<float | None>) -> list<float>",
    ] {
        assert!(dump.contains(signature), "{signature} missing:\n{dump}");
    }
    // An explicit instantiation may name an optional type argument ...
    let dump = ok("fn keep<T>(x: T) -> T:\n    return x\n\n\
                   fn main():\n    print(keep<int | None>(None) is None)\n");
    assert!(
        dump.contains("fn keep<int | None>(x: int | None) -> int | None"),
        "{dump}"
    );
    // ... and the instance is checked like any other: no ordering of `None`.
    rejects(
        &format!("{MAX}fn main():\n    print(max<int | None>(None, 1))\n"),
        &[
            "`int | None` may be `None` here",
            "in instantiation of `max<int | None>`",
        ],
    );
}

#[test]
fn defaults_of_generic_functions_are_filled_in() {
    let src = "fn repeat<T>(value: T, times: int = 2) -> list<T>:\n    out: list<T> = []\n    for i in range(times):\n        out.append(value)\n    return out\n\n\
               fn main():\n    print(repeat(\"a\"))\n    print(repeat(1, times=3))\n";
    let dump = ok(src);
    assert!(dump.contains("repeat<str>(\"a\":str, 2:int)"), "{dump}");
    assert!(dump.contains("repeat<int>(1:int, 3:int)"), "{dump}");
}

#[test]
fn an_uninferrable_type_argument_asks_for_an_explicit_one() {
    let src = "fn empty<T>() -> list<T>:\n    xs: list<T> = []\n    return xs\n\n\
               fn main():\n    print(len(empty()))\n";
    let rendered = rejects(
        src,
        &[
            "cannot infer the type argument `T` of `empty`",
            "write the type arguments explicitly, e.g. `empty<int>(...)`",
        ],
    );
    assert_eq!(errors(&rendered), 1, "{rendered}");
}

#[test]
fn a_conflicting_argument_points_at_where_the_type_was_inferred() {
    let rendered = rejects(
        &with_max("print(max(1, 2.5))"),
        &[
            "mismatched types: expected `int`, found `float`",
            "`T` is inferred as `int` from this argument",
        ],
    );
    assert_eq!(errors(&rendered), 1, "{rendered}");
}

// ---------------------------------------------------------------------------
// Errors after instantiation (DESIGN 3.9: no constraints in M3)
// ---------------------------------------------------------------------------

#[test]
fn an_operator_is_checked_per_instance_with_the_instantiation_stack() {
    let src = format!(
        "class Point:\n    x: float\n\n{MAX}fn main():\n    print(max(1, 2))\n    p = Point(x=1.0)\n    print(max(p, p).x)\n"
    );
    let rendered = rejects(
        &src,
        &[
            "`Point` values cannot be compared with `>`",
            "in instantiation of `max<Point>`",
            "`max<Point>` instantiated here",
        ],
    );
    // `max<int>` is fine; only the `Point` instance is wrong.
    assert_eq!(errors(&rendered), 1, "{rendered}");
}

#[test]
fn the_stack_lists_every_enclosing_instantiation() {
    let src = "fn add<T>(a: T, b: T) -> T:\n    return a + b\n\n\
               fn twice<T>(x: T) -> T:\n    return add(x, x)\n\n\
               fn main():\n    print(twice(True))\n";
    let rendered = rejects(
        src,
        &[
            "`+` cannot be applied to `bool` and `bool`",
            "in instantiation of `add<bool>`",
            "in instantiation of `twice<bool>`",
        ],
    );
    let inner = rendered.find("in instantiation of `add<bool>`").unwrap();
    let outer = rendered.find("in instantiation of `twice<bool>`").unwrap();
    assert!(inner < outer, "innermost first:\n{rendered}");
}

#[test]
fn a_mistake_in_a_template_is_reported_once() {
    // The same mistake in two instances is one diagnostic, not two.
    let src = "fn f<T>(x: T) -> T:\n    print(nope)\n    return x\n\n\
               fn main():\n    print(f(1))\n    print(f(\"a\"))\n";
    let rendered = rejects(src, &["cannot find value `nope` in this scope"]);
    assert_eq!(errors(&rendered), 1, "{rendered}");
}

#[test]
fn a_bad_signature_poisons_the_template() {
    let src = "fn f<T>(x: Nope) -> T:\n    return x\n\n\
               fn main():\n    print(f(1))\n    print(f<int>(2))\n";
    let rendered = rejects(src, &["cannot find type `Nope` in this scope"]);
    assert_eq!(errors(&rendered), 1, "{rendered}");

    let src = "class Box<T>:\n    item: T\n    item: int\n\n\
               fn main():\n    b = Box(item=1)\n    c: Box<str> = Box(item=\"a\")\n";
    let rendered = rejects(src, &["the field `item` is declared twice"]);
    assert_eq!(errors(&rendered), 1, "{rendered}");
}

#[test]
fn polymorphic_recursion_hits_the_depth_limit() {
    let src = "fn nest<T>(x: T, n: int) -> int:\n    if n == 0:\n        return 0\n    return nest([x], n - 1)\n\n\
               fn main():\n    print(nest(1, 3))\n";
    let rendered = rejects(
        src,
        &[
            "exceeds the depth limit of 64 nested instantiations",
            "polymorphic recursion",
            "in instantiation of `nest<int>`",
            "more instantiations",
        ],
    );
    assert_eq!(errors(&rendered), 1, "{rendered}");
}

#[test]
fn recursion_with_the_same_type_arguments_is_fine() {
    let src = "fn count<T>(xs: list<T>, i: int) -> int:\n    if i == len(xs):\n        return 0\n    return 1 + count(xs, i + 1)\n\n\
               fn main():\n    print(count([1, 2, 3], 0))\n";
    let dump = ok(src);
    assert_eq!(dump.matches("fn count<int>(").count(), 1, "{dump}");
}

#[test]
fn a_template_body_is_only_checked_through_its_instances() {
    // Like a C++ template: an instance nobody asks for is never checked.
    let src = "fn unused<T>(x: T) -> T:\n    return x + nope\n\nfn main():\n    pass\n";
    ok(src);
}

// ---------------------------------------------------------------------------
// Declarations and uses
// ---------------------------------------------------------------------------

#[test]
fn explicit_type_arguments_are_counted() {
    rejects(
        &with_max("print(max<int, str>(1, 2))"),
        &[
            "`max` takes 1 type argument but 2 were supplied",
            "write `max<T>(...)`",
        ],
    );
    rejects(
        &with_stack("s: Stack<int, str> = Stack(items=[])"),
        &["`Stack` takes 1 type argument but 2 were supplied"],
    );
    rejects(
        &with_stack("s: Stack = Stack(items=[1])"),
        &[
            "`Stack` takes 1 type argument but 0 were supplied",
            "write `Stack<T>`",
        ],
    );
}

#[test]
fn type_arguments_on_something_that_is_not_generic() {
    let src = "fn identity(a: int) -> int:\n    return a\n\nclass P:\n    x: int\n\n\
               fn main():\n    print(identity<int>(1))\n    p: P<int> = P(x=1)\n    print(len<int>([1]))\n";
    rejects(
        src,
        &[
            "`identity` is not generic",
            "`P` takes no type arguments; it is not generic",
            "`len` is not generic",
        ],
    );
    rejects(
        "fn main():\n    x: int<str> = 1\n",
        &["`int` takes no type arguments"],
    );
    rejects(
        &with_stack("s: Stack<int> = Stack(items=[])\ns.push<int>(1)"),
        &["the method `push` takes no type arguments"],
    );
}

#[test]
fn none_is_not_a_type_argument() {
    rejects(
        &with_stack("s: Stack<None> = Stack(items=[])"),
        &["`None` cannot be a type argument, in `Stack<None>`"],
    );
    rejects(
        &with_max("print(max(print(1), print(2)))"),
        &["`None` cannot be a type argument, in `max<None>`"],
    );
}

#[test]
fn type_parameters_are_checked_at_the_declaration() {
    rejects(
        "fn f<int>(x: int) -> int:\n    return x\n\nfn main():\n    pass\n",
        &["the type parameter `int` would hide the builtin type `int`"],
    );
    rejects(
        "fn f<T, T>(x: T) -> T:\n    return x\n\nfn main():\n    pass\n",
        &["the type parameter `T` is declared twice"],
    );
    rejects(
        "fn f<T>(x: T<int>) -> int:\n    return 1\n\nfn main():\n    pass\n",
        &["the type parameter `T` takes no type arguments"],
    );
    rejects("fn main<T>():\n    pass\n", &["`main` cannot be generic"]);
}

#[test]
fn methods_take_the_type_parameters_of_their_class() {
    rejects(
        "class Box<T>:\n    item: T\n\n    fn map<U>(self, u: U) -> U:\n        return u\n\nfn main():\n    pass\n",
        &["the method `map` cannot have type parameters of its own"],
    );
    rejects(
        "class Box:\n    item: int\n\n    fn map<U>(self, u: U) -> U:\n        return u\n\nfn main():\n    pass\n",
        &["the method `map` cannot have type parameters of its own"],
    );
    rejects(
        "class Box<T>:\n    item: T\n\n    fn get() -> int:\n        return 1\n\nfn main():\n    pass\n",
        &["the method `get` must take `self` as its first parameter"],
    );
}

#[test]
fn generic_names_share_the_namespace() {
    rejects(
        "fn f<T>(x: T) -> T:\n    return x\n\nfn f(x: int) -> int:\n    return x\n\nfn main():\n    pass\n",
        &["the name `f` is defined multiple times"],
    );
    rejects(
        "class f<T>:\n    x: T\n\nfn f(x: int) -> int:\n    return x\n\nfn main():\n    pass\n",
        &["the name `f` is defined multiple times"],
    );
    rejects(&with_max("m = max"), &["`max` is a function, not a value"]);
    // A generic `max` shadows the builtin of the same name.
    let dump = ok(&with_max("print(max(\"a\", \"b\"))"));
    assert!(dump.contains("fn max<str>("), "{dump}");
}

#[test]
fn members_of_an_instance() {
    rejects(
        &with_stack("s: Stack<int> = Stack(items=[])\ns.nope()"),
        &["`Stack<int>` has no method `nope`"],
    );
    rejects(
        &with_stack("s: Stack<int> = Stack(items=[])\nprint(s.size)"),
        &[
            "`Stack<int>` has no field `size`",
            "`size` is a method; call it",
        ],
    );
    rejects(
        &with_stack("s: Stack<int> = Stack(items=[])\ns.push(\"a\")"),
        &["mismatched types: expected `int`, found `str`"],
    );
}

#[test]
fn constructor_mistakes() {
    rejects(
        &with_stack("s = Stack([1])"),
        &["the constructor of `Stack` only takes keyword arguments"],
    );
    rejects(
        &with_stack("s = Stack(items=[1], extra=2)"),
        &["`Stack` has no field `extra`"],
    );
    rejects(
        &with_stack("s: Stack<int> = Stack()"),
        &["missing field `items` in the constructor of `Stack<int>`"],
    );
    rejects(
        &with_stack("s: Stack<float> = Stack(items=[1])"),
        &["mismatched types: expected `float`, found `int`"],
    );
}
