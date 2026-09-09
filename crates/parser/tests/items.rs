//! Top-level declarations: functions, classes and constants
//! (DESIGN 3.2, 3.3, 3.8, 3.9).

mod common;

use common::{module, stype};
use typhoon_ast::{Item, StmtKind};

fn only_fn(src: &str) -> typhoon_ast::FnDecl {
    let module = module(src);
    assert_eq!(module.items.len(), 1);
    match module.items.into_iter().next().expect("one item") {
        Item::Fn(decl) => decl,
        other => panic!("expected a function, got {other:?}"),
    }
}

fn only_class(src: &str) -> typhoon_ast::ClassDecl {
    let module = module(src);
    assert_eq!(module.items.len(), 1);
    match module.items.into_iter().next().expect("one item") {
        Item::Class(decl) => decl,
        other => panic!("expected a class, got {other:?}"),
    }
}

#[test]
fn minimal_function() {
    let decl = only_fn("fn main():\n    pass\n");
    assert_eq!(decl.name.name, "main");
    assert!(decl.generics.is_empty());
    assert!(decl.params.is_empty());
    assert!(decl.ret.is_none());
    assert_eq!(decl.body.stmts.len(), 1);
    assert!(!decl.is_method());
}

#[test]
fn parameters_and_return_type() {
    let decl = only_fn("fn add(a: int, b: int) -> int:\n    return a + b\n");
    assert_eq!(decl.params.len(), 2);
    assert_eq!(decl.params[0].name.name, "a");
    assert_eq!(stype(decl.params[0].ty.as_ref().expect("type")), "int");
    assert_eq!(stype(decl.ret.as_ref().expect("return type")), "int");
}

#[test]
fn default_arguments() {
    let decl = only_fn("fn scale(x: float, factor: float = 1.0) -> float:\n    return x\n");
    assert!(decl.params[0].default.is_none());
    assert!(decl.params[1].default.is_some());
}

#[test]
fn trailing_comma_in_the_parameter_list() {
    let decl = only_fn("fn f(a: int, b: int,):\n    pass\n");
    assert_eq!(decl.params.len(), 2);
}

#[test]
fn no_return_type_means_unit() {
    let decl = only_fn("fn log(message: str):\n    print(message)\n");
    assert!(decl.ret.is_none());
}

#[test]
fn an_explicit_none_return_type_parses() {
    let decl = only_fn("fn log(message: str) -> None:\n    pass\n");
    assert_eq!(stype(decl.ret.as_ref().expect("return type")), "None");
}

#[test]
fn optional_return_type() {
    let decl = only_fn("fn find(xs: list<int>) -> int | None:\n    return None\n");
    assert_eq!(stype(decl.ret.as_ref().expect("return type")), "int|None");
}

#[test]
fn generic_function() {
    let decl = only_fn("fn max<T>(a: T, b: T) -> T:\n    return a\n");
    assert_eq!(decl.generics.len(), 1);
    assert_eq!(decl.generics[0].name.name, "T");
    assert_eq!(stype(decl.params[0].ty.as_ref().expect("type")), "T");
}

#[test]
fn function_with_two_generic_parameters() {
    let decl = only_fn("fn pair<K, V>(k: K, v: V) -> dict<K, V>:\n    return {}\n");
    assert_eq!(decl.generics.len(), 2);
    assert_eq!(stype(decl.ret.as_ref().expect("return type")), "dict<K,V>");
}

#[test]
fn class_with_fields_and_methods() {
    let decl = only_class(
        "class Point:\n    x: float\n    y: float = 0.0\n\n    fn dist(self) -> float:\n        return self.x\n\n    fn scale(self, factor: float):\n        self.x = self.x * factor\n",
    );
    assert_eq!(decl.name.name, "Point");
    assert_eq!(decl.fields.len(), 2);
    assert_eq!(decl.fields[0].name.name, "x");
    assert_eq!(stype(&decl.fields[0].ty), "float");
    assert!(decl.fields[0].default.is_none());
    assert!(decl.fields[1].default.is_some());
    assert_eq!(decl.methods.len(), 2);
    assert!(decl.methods[0].is_method());
    assert!(decl.methods[0].params[0].is_self);
    assert!(decl.methods[0].params[0].ty.is_none());
    assert_eq!(decl.methods[1].params.len(), 2);
    assert!(!decl.methods[1].params[1].is_self);
}

#[test]
fn generic_class() {
    let decl = only_class(
        "class Stack<T>:\n    items: list<T>\n\n    fn push(self, value: T):\n        self.items.append(value)\n",
    );
    assert_eq!(decl.generics.len(), 1);
    assert_eq!(stype(&decl.fields[0].ty), "list<T>");
    assert_eq!(decl.methods.len(), 1);
}

#[test]
fn empty_class_body_with_pass() {
    let decl = only_class("class Empty:\n    pass\n");
    assert!(decl.fields.is_empty());
    assert!(decl.methods.is_empty());
}

#[test]
fn constant_declaration() {
    let module = module("PI: float = 3.14159\n");
    assert_eq!(module.items.len(), 1);
    match &module.items[0] {
        Item::Const(decl) => {
            assert_eq!(decl.name.name, "PI");
            assert_eq!(stype(&decl.ty), "float");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn several_items_in_one_file() {
    let module = module(
        "PI: float = 3.0\n\nclass Point:\n    x: float\n\nfn area(r: float) -> float:\n    return PI * r\n\nfn main():\n    print(area(2.0))\n",
    );
    assert_eq!(module.items.len(), 4);
    assert!(matches!(module.items[0], Item::Const(_)));
    assert!(matches!(module.items[1], Item::Class(_)));
    assert!(matches!(module.items[2], Item::Fn(_)));
    assert_eq!(module.items[3].name().name, "main");
}

#[test]
fn item_spans_cover_the_declaration() {
    let src = "fn main():\n    pass\n";
    let module = module(src);
    assert_eq!(&src[module.items[0].span().range()], "fn main():\n    pass");
}

#[test]
fn module_span_covers_the_file() {
    let src = "fn main():\n    pass\n";
    let module = module(src);
    assert_eq!(module.span.start, 0);
    assert_eq!(module.span.end as usize, src.len());
}

#[test]
fn a_method_body_sees_self() {
    let decl = only_class("class C:\n    fn get(self) -> int:\n        return self.x\n");
    let StmtKind::Return(Some(expr)) = &decl.methods[0].body.stmts[0].kind else {
        panic!("expected a return");
    };
    assert_eq!(common::sexpr(expr), "(. self x)");
}

#[test]
fn class_snapshot() {
    insta::assert_snapshot!(typhoon_ast::dump(&module(
        "class Stack<T>:\n    items: list<T>\n    count: int = 0\n\n    fn push(self, value: T):\n        self.items.append(value)\n\n    fn pop(self) -> T:\n        return self.items.pop()\n"
    )));
}

#[test]
fn function_snapshot() {
    insta::assert_snapshot!(typhoon_ast::dump(&module(
        "fn scale<T>(x: T, factor: float = 1.0, name: str = \"x\") -> T:\n    return x\n"
    )));
}

#[test]
fn methods_may_precede_fields() {
    let decl = only_class("class C:\n    fn m(self):\n        pass\n\n    x: int\n");
    assert_eq!(decl.fields.len(), 1);
    assert_eq!(decl.methods.len(), 1);
}

#[test]
fn self_is_only_special_as_the_first_parameter_of_a_method() {
    // A free function may still call a parameter `self`; sema decides what that
    // means. The parser only records `is_self`.
    let decl = only_fn("fn f(self):\n    pass\n");
    assert!(decl.params[0].is_self);
    assert!(decl.is_method());
}

#[test]
fn a_method_may_have_defaults_and_generics() {
    let decl = only_class(
        "class Box<T>:\n    fn put<U>(self, value: U, times: int = 1) -> T:\n        return value\n",
    );
    let method = &decl.methods[0];
    assert_eq!(method.generics.len(), 1);
    assert_eq!(method.params.len(), 3);
    assert!(method.params[0].is_self);
    assert!(method.params[2].default.is_some());
}

#[test]
fn constant_of_a_generic_type() {
    let module = module("EMPTY: list<int> = []\n");
    match &module.items[0] {
        Item::Const(decl) => assert_eq!(stype(&decl.ty), "list<int>"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn several_functions_in_a_row_without_blank_lines() {
    let module = module("fn a():\n    pass\nfn b():\n    pass\nfn c():\n    pass\n");
    assert_eq!(module.items.len(), 3);
}

#[test]
fn a_base_class_list_is_rejected_with_a_dedicated_message() {
    let messages = common::messages("class B(A):\n    x: int\n");
    assert!(
        messages
            .iter()
            .any(|m| m.contains("inheritance is not supported")),
        "{messages:?}"
    );
    // Recovery still yields the class, so its body is checked as usual.
    let (module, _) = common::module_with_diags("class B(A):\n    x: int\n");
    match &module.items[0] {
        Item::Class(decl) => {
            assert_eq!(decl.name.as_str(), "B");
            assert_eq!(decl.fields.len(), 1);
        }
        other => panic!("{other:?}"),
    }
}
