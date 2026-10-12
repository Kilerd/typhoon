//! Nominal traits, bounds and operator overloading (DESIGN 3.11): what the
//! checker lowers them to, and what the definition-time check of a bounded
//! item leaves behind (nothing).

use typhoon_diag::{Diagnostics, SourceMap, render_all};
use typhoon_sema::hir::{ExprKind, Program, Stmt};
use typhoon_sema::types::Type;
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

#[track_caller]
fn ok(src: &str) -> Program {
    match check(src) {
        Ok(program) => program,
        Err(err) => panic!("expected the program to check:\n{err}"),
    }
}

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

fn errors(rendered: &str) -> usize {
    rendered.matches("error: ").count()
}

const POINT: &str = "class Point(Eq, Ord, Hash, Repr):
    x: int
    y: int

";

#[test]
fn derived_methods_are_ordinary_functions() {
    let program = ok(&format!(
        "{POINT}fn main():\n    p = Point(x=1, y=2)\n    print(p == p, p < p, hash(p))\n"
    ));
    let names: Vec<&str> = program.functions.iter().map(|f| f.name.as_str()).collect();
    for method in ["__eq__", "__lt__", "__hash__", "__repr__"] {
        let name = format!("Point.{method}");
        assert!(names.contains(&name.as_str()), "{names:?}");
    }
    // `p == p` is a direct call of the derived `__eq__`.
    let dump = dump_program(&program);
    assert!(dump.contains("Point.__eq__"), "{dump}");
}

#[test]
fn the_definition_check_leaves_nothing_behind() {
    let program = ok("trait Shape:
    fn area(self) -> float
    fn twice(self) -> float:
        return 2.0 * self.area()

class Box<T: Shape>:
    item: T

    fn area(self) -> float:
        return self.item.area()

fn total<T: Shape, U>(xs: list<T>, extra: U) -> float:
    s = 0.0
    for x in xs:
        s = s + x.twice()
    return s

fn main():
    print(1)
");
    // No pseudo-class for `T`, `U` or `Self`, no `Box<T>` instance and no
    // function of the check survives into the program.
    for class in program.types.classes() {
        assert!(
            !["T", "U", "Self"].contains(&class.name.as_str()) && !class.name.contains('<'),
            "{}",
            class.name
        );
    }
    assert_eq!(program.functions.len(), 1, "only `main` is used");
    assert!(program.protocols.eq.is_empty() && program.protocols.repr.is_empty());
}

#[test]
fn containers_of_classes_register_their_protocols() {
    let program = ok(&format!(
        "{POINT}fn main():\n    xs = [Point(x=1, y=2)]\n    print(xs, xs == xs, Point(x=0, y=0) in xs)\n"
    ));
    let point = program
        .types
        .classes()
        .iter()
        .position(|c| c.name == "Point")
        .expect("Point is declared");
    let eq = program
        .protocols
        .eq
        .iter()
        .find(|(class, _)| class.index() == point)
        .map(|(_, func)| *func)
        .expect("`==` on a list<Point> needs Point.__eq__");
    assert_eq!(program.function(eq).name, "Point.__eq__");
    assert!(program.protocols.repr.keys().any(|c| c.index() == point));
}

#[test]
fn reversed_comparisons_keep_left_to_right_evaluation() {
    let program = ok(&format!(
        "{POINT}fn main():\n    a = Point(x=1, y=2)\n    b = Point(x=2, y=0)\n    print(a > b)\n"
    ));
    let main = program.function(program.main);
    let Some(Stmt::Expr(print)) = main.body.stmts.last() else {
        panic!("main ends with the print");
    };
    let ExprKind::Print(args) = &print.kind else {
        panic!("a print");
    };
    // `a > b` is `b.__lt__(a)`: the receiver is `b`, `a` is evaluated first.
    let ExprKind::Call {
        args, eval_order, ..
    } = &args[0].kind
    else {
        panic!("a call of __lt__: {:?}", args[0].kind);
    };
    assert_eq!(args.len(), 2);
    assert_eq!(eval_order, &[1, 0]);
}

#[test]
fn bounds_hold_for_builtin_types_per_the_table() {
    let ok_src = "fn h<T: Hash>(x: T) -> int:
    return hash(x)

fn o<T: Ord>(a: T, b: T) -> bool:
    return a < b

fn r<T: Repr>(x: T) -> str:
    return repr(x)

fn ad<T: Add>(a: T, b: T) -> T:
    return a + b

fn main():
    o_int: int | None = 1
    print(h(1), h(True), h(\"s\"), h((1, \"a\")), h(o_int))
    print(o(1, 2), o(1.5, 2.5), o(\"a\", \"b\"), o(False, True), o((1, 2), (1, 3)))
    print(r([1]), r((1, \"a\")), r(o_int), r({\"k\": 1}))
    print(ad(1, 2), ad(1.5, 2.5), ad(\"a\", \"b\"), ad([1], [2]))
";
    let program = ok(ok_src);
    assert!(
        program.types.classes().iter().all(|c| c.name != "T"),
        "no opaque type survives"
    );
    for (call, ty) in [
        ("h(1.5)", "float"),
        ("h([1])", "list<int>"),
        ("o([1], [2])", "list<int>"),
        ("ad(1 == 1, True)", "bool"),
    ] {
        let src = ok_src.replace("fn main():\n", &format!("fn main():\n    print({call})\n"));
        rejects(
            &src,
            &[&format!("the type `{ty}` does not implement the trait")],
        );
    }
}

#[test]
fn a_bounded_template_with_an_error_reports_it_once() {
    let rendered = rejects(
        "fn f<T: Eq>(a: T) -> T:
    return a + a

fn main():
    print(f(1), f(2), f(\"x\"))
",
        &["`+` cannot be applied to `T`", "add the bound `T: Add`"],
    );
    assert_eq!(errors(&rendered), 1, "{rendered}");
}

#[test]
fn self_names_the_class() {
    let program = ok("class Counter:
    n: int

    fn same(self, other: Self) -> bool:
        return self.n == other.n

fn main():
    print(Counter(n=1).same(Counter(n=1)))
");
    let same = program
        .functions
        .iter()
        .find(|f| f.name == "Counter.same")
        .expect("the method");
    let other = same.locals[same.params[1].index()].ty;
    assert!(matches!(program.types.get(other), Type::Class(_)));
}
