//! The generic surface the M3 type checker relies on (DESIGN 3.9): generic
//! declarations, explicit instantiation of functions and constructors, and the
//! `>>` / `>=` splitting where a type-argument list ends next to `=`, `(` or
//! another `>`.

mod common;

use common::{expr, module, stype};
use typhoon_ast::{Item, StmtKind};

#[test]
fn explicit_constructor_instantiation() {
    assert_eq!(
        expr("Stack<int>(items=[])"),
        "(call Stack<int> items=(list))"
    );
    assert_eq!(
        expr("Pair<str, list<int>>(key=\"a\", value=[1])"),
        "(call Pair<str,list<int>> key=\"a\" value=(list 1))"
    );
}

#[test]
fn an_explicit_instantiation_is_a_postfix_operand() {
    assert_eq!(
        expr("make<int>().size()"),
        "(call (. (call make<int>) size))"
    );
    assert_eq!(expr("first<int>(xs)[0]"), "(index (call first<int> xs) 0)");
    assert_eq!(expr("-f<int>(1)"), "(- (call f<int> 1))");
    assert_eq!(expr("f<int>(g<str>(x))"), "(call f<int> (call g<str> x))");
}

#[test]
fn the_csharp_rule_inside_an_argument_list() {
    // DESIGN 3.9 point 3: `a < b, c > (d)` reads as a generic call with two
    // type arguments, exactly as in C#; parenthesise to compare.
    assert_eq!(expr("f(a < b, c > (d))"), "(call f (call a<b,c> d))");
    assert_eq!(
        expr("f((a < b), c > (d))"),
        "(call f (cmp a < b) (cmp c > d))"
    );
}

#[test]
fn generic_declarations_keep_their_parameters_in_order() {
    let module = module(
        "class Pair<K, V>:\n    key: K\n    value: V\n\n    fn flip(self) -> Pair<V, K>:\n        return Pair(key=self.value, value=self.key)\n\n\
         fn zip<A, B>(xs: list<A>, ys: list<B>) -> list<tuple<A, B>>:\n    return []\n",
    );
    let Item::Class(class) = &module.items[0] else {
        panic!("expected a class")
    };
    let names: Vec<&str> = class.generics.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, ["K", "V"]);
    assert_eq!(
        stype(class.methods[0].ret.as_ref().expect("a return type")),
        "Pair<V,K>"
    );
    let Item::Fn(func) = &module.items[1] else {
        panic!("expected a function")
    };
    let names: Vec<&str> = func.generics.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, ["A", "B"]);
    assert_eq!(
        stype(func.ret.as_ref().expect("a return type")),
        "list<tuple<A,B>>"
    );
}

#[test]
fn nested_instances_close_next_to_equals() {
    // `>>=` lexes as `>>` then `=`; both `>` close a list, `=` stays.
    let module = module(
        "fn main():\n    s: Stack<list<int>>= Stack(items=[])\n    t: Box<Stack<list<int>>>= make()\n",
    );
    let Item::Fn(main) = &module.items[0] else {
        panic!("expected a function")
    };
    let types: Vec<String> = main
        .body
        .stmts
        .iter()
        .map(|s| match &s.kind {
            StmtKind::AnnAssign { ty, value, .. } => {
                assert!(value.is_some(), "the initializer survives the split");
                stype(ty)
            }
            other => panic!("expected an annotated assignment, got {other:?}"),
        })
        .collect();
    assert_eq!(types, ["Stack<list<int>>", "Box<Stack<list<int>>>"]);
}
