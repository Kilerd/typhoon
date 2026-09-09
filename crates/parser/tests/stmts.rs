//! Statement parsing (DESIGN 3.4, 3.5).

mod common;

use common::{expr_dump, module, stmts, stmts_dump, wrap_in_main};
use typhoon_ast::{Expr, ExprKind, Item, Stmt, StmtKind};

fn only(body: &str) -> Stmt {
    let mut stmts = stmts(body);
    assert_eq!(stmts.len(), 1, "expected exactly one statement in {body:?}");
    stmts.pop().expect("one statement")
}

fn name_of(expr: &Expr) -> &str {
    match &expr.kind {
        ExprKind::Name(name) => &name.name,
        other => panic!("expected a name, got {other:?}"),
    }
}

#[test]
fn expression_statement() {
    let stmt = only("print(1)");
    match stmt.kind {
        StmtKind::Expr(Expr {
            kind: ExprKind::Call { .. },
            ..
        }) => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn assignment_to_a_name() {
    let stmt = only("x = 1");
    match stmt.kind {
        StmtKind::Assign { target, value } => {
            assert_eq!(name_of(&target), "x");
            assert!(matches!(value.kind, ExprKind::Int(1)));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn assignment_targets_may_be_attributes_and_indexes() {
    for (body, expected) in [
        ("p.x = 1.0", "Attribute"),
        ("xs[0] = 1", "Index"),
        ("self.items[i] = v", "Index"),
    ] {
        let stmt = only(body);
        match stmt.kind {
            StmtKind::Assign { target, .. } => {
                let actual = match target.kind {
                    ExprKind::Attribute { .. } => "Attribute",
                    ExprKind::Index { .. } => "Index",
                    other => panic!("{other:?}"),
                };
                assert_eq!(actual, expected, "for {body:?}");
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn annotated_assignment() {
    let stmt = only("x: int = 1");
    match stmt.kind {
        StmtKind::AnnAssign { name, ty, value } => {
            assert_eq!(name.name, "x");
            assert_eq!(common::stype(&ty), "int");
            assert!(matches!(value.expect("initializer").kind, ExprKind::Int(1)));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn annotated_assignment_with_a_generic_type() {
    let stmt = only("d: dict<str, list<int>> = {}");
    match stmt.kind {
        StmtKind::AnnAssign { ty, .. } => {
            assert_eq!(common::stype(&ty), "dict<str,list<int>>");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn annotated_assignment_with_an_optional_type() {
    let stmt = only("r: int | None = None");
    match stmt.kind {
        StmtKind::AnnAssign { ty, .. } => assert_eq!(common::stype(&ty), "int|None"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn return_with_and_without_a_value() {
    match only("return").kind {
        StmtKind::Return(None) => {}
        other => panic!("{other:?}"),
    }
    match only("return 1 + 2").kind {
        StmtKind::Return(Some(value)) => assert!(matches!(value.kind, ExprKind::Binary { .. })),
        other => panic!("{other:?}"),
    }
}

#[test]
fn pass_break_and_continue() {
    assert!(matches!(only("pass").kind, StmtKind::Pass));
    assert!(matches!(only("break").kind, StmtKind::Break));
    assert!(matches!(only("continue").kind, StmtKind::Continue));
}

#[test]
fn if_without_else() {
    match only("if x:\n    pass").kind {
        StmtKind::If {
            elifs, else_, then, ..
        } => {
            assert_eq!(then.stmts.len(), 1);
            assert!(elifs.is_empty());
            assert!(else_.is_none());
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn if_elif_else() {
    match only("if a:\n    pass\nelif b:\n    pass\nelif c:\n    pass\nelse:\n    pass").kind {
        StmtKind::If { elifs, else_, .. } => {
            assert_eq!(elifs.len(), 2);
            assert!(else_.is_some());
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn while_loop() {
    match only("while i < 5:\n    i = i + 1").kind {
        StmtKind::While { cond, body } => {
            assert!(matches!(cond.kind, ExprKind::Compare { .. }));
            assert_eq!(body.stmts.len(), 1);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn for_loop() {
    match only("for i in range(10):\n    print(i)").kind {
        StmtKind::For { var, iter, body } => {
            assert_eq!(var.name, "i");
            assert!(matches!(iter.kind, ExprKind::Call { .. }));
            assert_eq!(body.stmts.len(), 1);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn nested_blocks() {
    let stmt = only("for i in range(3):\n    if i > 1:\n        while True:\n            break");
    let StmtKind::For { body, .. } = stmt.kind else {
        panic!("expected a for loop")
    };
    let StmtKind::If { then, .. } = &body.stmts[0].kind else {
        panic!("expected an if")
    };
    let StmtKind::While { body, .. } = &then.stmts[0].kind else {
        panic!("expected a while")
    };
    assert!(matches!(body.stmts[0].kind, StmtKind::Break));
}

#[test]
fn multi_level_dedent() {
    let stmts = stmts("if a:\n    if b:\n        pass\nx = 1");
    assert_eq!(stmts.len(), 2);
    assert!(matches!(stmts[0].kind, StmtKind::If { .. }));
    assert!(matches!(stmts[1].kind, StmtKind::Assign { .. }));
}

#[test]
fn blank_and_comment_lines_are_ignored() {
    let stmts = stmts("x = 1\n\n# a comment\n\ny = 2  # trailing comment");
    assert_eq!(stmts.len(), 2);
}

#[test]
fn statement_spans_exclude_the_newline() {
    // `x = 1` starts at offset 15 in `fn main():\n    x = 1\n`.
    let src = wrap_in_main("x = 1");
    let module = module(&src);
    let Item::Fn(decl) = &module.items[0] else {
        panic!("expected a function")
    };
    let stmt = &decl.body.stmts[0];
    assert_eq!(&src[stmt.span.range()], "x = 1");
}

#[test]
fn kitchen_sink_snapshot() {
    let body = "\
x: int = 1
x = x + 1
if x < 2:
    print(\"small\")
elif x < 10:
    print(\"medium\")
else:
    print(\"large\")
while x > 0:
    x = x - 1
    if x == 3:
        continue
    if x == 1:
        break
for i in range(0, 10, 2):
    print(f\"i={i:.2f}\")
return None";
    insta::assert_snapshot!(stmts_dump(body));
}

#[test]
fn index_and_attribute_assignment_snapshot() {
    insta::assert_snapshot!(stmts_dump("self.items[i] = v"));
}

#[test]
fn expression_spans_are_exact() {
    assert_eq!(expr_dump("1"), "Int 1 @0..1");
}

#[test]
fn calls_may_span_several_lines() {
    // Inside brackets the lexer suppresses layout tokens (DESIGN 3.1).
    let stmts = stmts("print(\n    1,\n    2,\n)");
    assert_eq!(stmts.len(), 1);
    let StmtKind::Expr(expr) = &stmts[0].kind else {
        panic!("expected an expression")
    };
    assert_eq!(common::sexpr(expr), "(call print 1 2)");
}

#[test]
fn a_file_without_a_trailing_newline_parses() {
    let module = module("fn main():\n    pass");
    assert_eq!(module.items.len(), 1);
}

#[test]
fn a_comment_may_end_the_file() {
    let module = module("fn main():\n    pass\n# done");
    assert_eq!(module.items.len(), 1);
}

#[test]
fn tuple_unpacking_parses_as_a_tuple_on_both_sides() {
    let stmt = only("a, b = 1, 2");
    let StmtKind::Assign { target, value } = &stmt.kind else {
        panic!("expected an assignment, got {:?}", stmt.kind)
    };
    assert_eq!(common::sexpr(target), "(tuple a b)");
    assert_eq!(common::sexpr(value), "(tuple 1 2)");
}

#[test]
fn a_bare_tuple_may_be_returned() {
    let stmt = only("return a, b");
    let StmtKind::Return(Some(value)) = &stmt.kind else {
        panic!("expected a return, got {:?}", stmt.kind)
    };
    assert_eq!(common::sexpr(value), "(tuple a b)");
}

#[test]
fn a_trailing_comma_still_makes_a_tuple() {
    let stmt = only("x = 1,");
    let StmtKind::Assign { value, .. } = &stmt.kind else {
        panic!("expected an assignment, got {:?}", stmt.kind)
    };
    assert_eq!(common::sexpr(value), "(tuple 1)");
}

#[test]
fn an_annotated_declaration_accepts_a_bare_tuple() {
    let stmt = only("t: tuple<int, int> = 1, 2");
    let StmtKind::AnnAssign { value, .. } = &stmt.kind else {
        panic!("expected a declaration, got {:?}", stmt.kind)
    };
    assert_eq!(common::sexpr(value.as_ref().unwrap()), "(tuple 1 2)");
}

#[test]
fn an_unpacking_target_must_be_assignable() {
    let messages = common::messages("fn main():\n    a, f() = 1, 2\n");
    assert!(
        messages
            .iter()
            .any(|m| m.contains("invalid assignment target")),
        "{messages:?}"
    );
}

#[test]
fn boolean_conditions_keep_their_structure() {
    let stmt = only("if a and not b or c:\n    pass");
    let StmtKind::If { cond, .. } = &stmt.kind else {
        panic!("expected an if")
    };
    assert_eq!(common::sexpr(cond), "(or (and a (not b)) c)");
}

#[test]
fn deeply_nested_statements_keep_their_shape() {
    let stmts = stmts(
        "for i in range(3):\n    for j in range(3):\n        if i == j:\n            print(i)\n        else:\n            continue\n",
    );
    assert_eq!(stmts.len(), 1);
    let StmtKind::For { body, .. } = &stmts[0].kind else {
        panic!("expected a for")
    };
    let StmtKind::For { body, .. } = &body.stmts[0].kind else {
        panic!("expected a for")
    };
    let StmtKind::If { else_, .. } = &body.stmts[0].kind else {
        panic!("expected an if")
    };
    assert!(else_.is_some());
}
