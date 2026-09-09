//! Type-expression parsing, including the `>>` splitting of DESIGN 3.9.

mod common;

use common::{messages, module, stype};
use typhoon_ast::{Item, StmtKind};

/// The type of the single annotated declaration in `fn main()`.
fn ann_type(decl: &str) -> String {
    let src = format!("fn main():\n    {decl}\n");
    let module = module(&src);
    let Item::Fn(decl) = &module.items[0] else {
        panic!("expected a function")
    };
    match &decl.body.stmts[0].kind {
        StmtKind::AnnAssign { ty, .. } => stype(ty),
        other => panic!("expected an annotated assignment, got {other:?}"),
    }
}

/// The return type of a function declared with `-> ty`.
fn ret_type(ty: &str) -> String {
    let src = format!("fn f() -> {ty}:\n    pass\n");
    let module = module(&src);
    let Item::Fn(decl) = &module.items[0] else {
        panic!("expected a function")
    };
    stype(decl.ret.as_ref().expect("a return type"))
}

#[test]
fn simple_named_types() {
    for name in ["int", "float", "bool", "str", "i32", "u64", "Point"] {
        assert_eq!(ret_type(name), name);
    }
}

#[test]
fn the_unit_type_is_written_none() {
    assert_eq!(ret_type("None"), "None");
}

#[test]
fn generic_types() {
    assert_eq!(ret_type("list<int>"), "list<int>");
    assert_eq!(ret_type("dict<str, int>"), "dict<str,int>");
    assert_eq!(ret_type("tuple<int, str, bool>"), "tuple<int,str,bool>");
    assert_eq!(ret_type("Stack<T>"), "Stack<T>");
}

#[test]
fn nested_generics_split_the_shift_token() {
    // `>>` and `>>>` close two and three nested lists.
    assert_eq!(ret_type("dict<str, list<int>>"), "dict<str,list<int>>");
    assert_eq!(ret_type("list<list<list<int>>>"), "list<list<list<int>>>");
    assert_eq!(
        ret_type("dict<str, dict<str, list<int>>>"),
        "dict<str,dict<str,list<int>>>"
    );
}

#[test]
fn a_generic_type_may_have_a_trailing_comma() {
    assert_eq!(ret_type("dict<str, int,>"), "dict<str,int>");
}

#[test]
fn union_types() {
    assert_eq!(ret_type("int | None"), "int|None");
    assert_eq!(ret_type("int | float | None"), "int|float|None");
    assert_eq!(ret_type("list<int> | None"), "list<int>|None");
    assert_eq!(
        ret_type("dict<str, list<int>> | None"),
        "dict<str,list<int>>|None"
    );
}

#[test]
fn annotated_declarations() {
    assert_eq!(ann_type("x: int = 1"), "int");
    assert_eq!(ann_type("xs: list<int> = []"), "list<int>");
    assert_eq!(
        ann_type("d: dict<str, list<int>> = {}"),
        "dict<str,list<int>>"
    );
    assert_eq!(ann_type("r: int | None = None"), "int|None");
}

#[test]
fn a_generic_annotation_directly_followed_by_equals() {
    // The lexer produces `>=` here; the parser has to split it.
    assert_eq!(ann_type("d: dict<str, int>= {}"), "dict<str,int>");
    assert_eq!(ann_type("xs: list<int>= []"), "list<int>");
}

#[test]
fn class_fields_and_parameters_use_the_same_type_grammar() {
    let module = module(
        "class Registry<K, V>:\n    entries: dict<K, list<V>>\n    fallback: V | None = None\n",
    );
    let Item::Class(decl) = &module.items[0] else {
        panic!("expected a class")
    };
    assert_eq!(stype(&decl.fields[0].ty), "dict<K,list<V>>");
    assert_eq!(stype(&decl.fields[1].ty), "V|None");
}

#[test]
fn missing_type_is_reported() {
    let messages = messages("fn main():\n    x: = 1\n");
    assert!(
        messages.iter().any(|m| m.contains("expected a type")),
        "{messages:?}"
    );
}

#[test]
fn a_literal_is_not_a_type() {
    let messages = messages("fn f() -> 1:\n    pass\n");
    assert!(
        messages.iter().any(|m| m.contains("expected a type")),
        "{messages:?}"
    );
}

#[test]
fn an_empty_type_argument_list_is_rejected() {
    let messages = messages("fn f() -> list<>:\n    pass\n");
    assert!(!messages.is_empty(), "expected a diagnostic");
}

#[test]
fn unclosed_type_argument_list_is_reported() {
    let messages = messages("fn f() -> list<int:\n    pass\n");
    assert!(!messages.is_empty(), "expected a diagnostic");
}
