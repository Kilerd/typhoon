//! Tests for the AST pretty-printer, on hand-built trees (the parser has its
//! own snapshot tests that go through real source).

use typhoon_ast::*;
use typhoon_diag::{FileId, Span};

const F: FileId = FileId(0);

fn sp(a: u32, b: u32) -> Span {
    Span::new(F, a, b)
}

fn ident(name: &str, a: u32) -> Ident {
    Ident::new(name, sp(a, a + name.len() as u32))
}

fn name(n: &str, a: u32) -> Expr {
    Expr::new(ExprKind::Name(ident(n, a)), sp(a, a + n.len() as u32))
}

fn int(v: i64, a: u32) -> Expr {
    Expr::new(ExprKind::Int(v), sp(a, a + 1))
}

fn named_ty(n: &str, a: u32, args: Vec<TypeExpr>) -> TypeExpr {
    let end = args
        .last()
        .map(|t| t.span.end + 1)
        .unwrap_or(a + n.len() as u32);
    TypeExpr::new(
        TypeExprKind::Named {
            name: ident(n, a),
            args,
        },
        sp(a, end),
    )
}

#[test]
fn dumps_literals() {
    assert_eq!(dump_expr(&int(42, 0)), "Int 42 @0..1");
    assert_eq!(
        dump_expr(&Expr::new(ExprKind::Float(1.0), sp(0, 3))),
        "Float 1.0 @0..3"
    );
    assert_eq!(
        dump_expr(&Expr::new(ExprKind::Float(1.5e-3), sp(0, 6))),
        "Float 0.0015 @0..6"
    );
    assert_eq!(
        dump_expr(&Expr::new(ExprKind::Str("a\nb".into()), sp(0, 6))),
        "Str \"a\\nb\" @0..6"
    );
    assert_eq!(
        dump_expr(&Expr::new(ExprKind::Bool(true), sp(0, 4))),
        "Bool True @0..4"
    );
    assert_eq!(
        dump_expr(&Expr::new(ExprKind::Bool(false), sp(0, 5))),
        "Bool False @0..5"
    );
    assert_eq!(
        dump_expr(&Expr::new(ExprKind::None, sp(0, 4))),
        "None @0..4"
    );
    assert_eq!(dump_expr(&name("total", 0)), "Name `total` @0..5");
    assert_eq!(
        dump_expr(&Expr::new(ExprKind::Error, sp(0, 1))),
        "Error @0..1"
    );
}

#[test]
fn dumps_dummy_span_as_question_mark() {
    assert_eq!(
        dump_expr(&Expr::new(ExprKind::Int(1), Span::dummy())),
        "Int 1 @?"
    );
}

#[test]
fn dumps_binary_expression_tree() {
    // a + b * 2
    let mul = Expr::new(
        ExprKind::Binary {
            op: BinOp::Mul,
            op_span: sp(6, 7),
            lhs: Box::new(name("b", 4)),
            rhs: Box::new(int(2, 8)),
        },
        sp(4, 9),
    );
    let add = Expr::new(
        ExprKind::Binary {
            op: BinOp::Add,
            op_span: sp(2, 3),
            lhs: Box::new(name("a", 0)),
            rhs: Box::new(mul),
        },
        sp(0, 9),
    );
    insta::assert_snapshot!(dump_expr(&add), @r"
    Binary + @0..9
      op Op @2..3
      lhs Name `a` @0..1
      rhs Binary * @4..9
        op Op @6..7
        lhs Name `b` @4..5
        rhs Int 2 @8..9
    ");
}

#[test]
fn dumps_types() {
    // dict<str, list<int>>
    let inner = named_ty("list", 10, vec![named_ty("int", 15, vec![])]);
    let outer = named_ty("dict", 0, vec![named_ty("str", 5, vec![]), inner]);
    insta::assert_snapshot!(dump_type(&outer), @r"
    Named `dict` @0..20
      arg Named `str` @5..8
      arg Named `list` @10..19
        arg Named `int` @15..18
    ");

    let union = TypeExpr::new(
        TypeExprKind::Union(vec![
            named_ty("int", 0, vec![]),
            TypeExpr::new(TypeExprKind::None, sp(6, 10)),
        ]),
        sp(0, 10),
    );
    insta::assert_snapshot!(dump_type(&union), @r"
    Union @0..10
      Named `int` @0..3
      NoneType @6..10
    ");

    assert_eq!(
        dump_type(&TypeExpr::new(TypeExprKind::Error, sp(0, 1))),
        "ErrorType @0..1"
    );
}

#[test]
fn dumps_a_full_module() {
    // A hand-built module covering every node kind the dumper knows about.
    let mut items = Vec::new();

    // PI: float = 3.5
    items.push(Item::Const(ConstDecl {
        name: ident("PI", 0),
        ty: named_ty("float", 4, vec![]),
        value: Expr::new(ExprKind::Float(3.5), sp(12, 15)),
        span: sp(0, 15),
    }));

    // class Stack<T>: items: list<T> = []; fn push(self, value: T = 1): ...
    let push = FnDecl {
        name: ident("push", 100),
        generics: vec![],
        params: vec![
            Param {
                name: ident("self", 105),
                ty: None,
                default: None,
                is_self: true,
                span: sp(105, 109),
            },
            Param {
                name: ident("value", 111),
                ty: Some(named_ty("T", 118, vec![])),
                default: Some(int(1, 122)),
                is_self: false,
                span: sp(111, 123),
            },
        ],
        ret: None,
        body: Block {
            stmts: vec![Stmt {
                kind: StmtKind::Pass,
                span: sp(130, 134),
            }],
            span: sp(130, 134),
        },
        span: sp(97, 134),
    };
    items.push(Item::Class(ClassDecl {
        name: ident("Stack", 22),
        generics: vec![GenericParam {
            name: ident("T", 28),
            span: sp(28, 29),
        }],
        fields: vec![Field {
            name: ident("items", 36),
            ty: named_ty("list", 43, vec![named_ty("T", 48, vec![])]),
            default: Some(Expr::new(ExprKind::List(vec![]), sp(53, 55))),
            span: sp(36, 55),
        }],
        methods: vec![push],
        span: sp(16, 134),
    }));

    // fn main(): every statement kind
    let stmts = vec![
        Stmt {
            kind: StmtKind::AnnAssign {
                name: ident("x", 150),
                ty: named_ty("int", 153, vec![]),
                value: Some(int(1, 159)),
            },
            span: sp(150, 160),
        },
        Stmt {
            kind: StmtKind::Assign {
                target: name("x", 165),
                value: int(2, 169),
            },
            span: sp(165, 170),
        },
        Stmt {
            kind: StmtKind::If {
                cond: Expr::new(
                    ExprKind::Compare {
                        left: Box::new(name("x", 175)),
                        tail: vec![CompareTail {
                            op: CmpOp::Lt,
                            op_span: sp(177, 178),
                            rhs: int(3, 179),
                        }],
                    },
                    sp(175, 180),
                ),
                then: Block {
                    stmts: vec![Stmt {
                        kind: StmtKind::Break,
                        span: sp(185, 190),
                    }],
                    span: sp(185, 190),
                },
                elifs: vec![ElifBranch {
                    cond: Expr::new(ExprKind::Bool(true), sp(200, 204)),
                    body: Block {
                        stmts: vec![Stmt {
                            kind: StmtKind::Continue,
                            span: sp(210, 218),
                        }],
                        span: sp(210, 218),
                    },
                    span: sp(195, 218),
                }],
                else_: Some(Block {
                    stmts: vec![Stmt {
                        kind: StmtKind::Return(None),
                        span: sp(230, 236),
                    }],
                    span: sp(230, 236),
                }),
            },
            span: sp(172, 236),
        },
        Stmt {
            kind: StmtKind::While {
                cond: Expr::new(
                    ExprKind::BoolOp {
                        op: BoolOp::And,
                        op_span: sp(248, 251),
                        lhs: Box::new(Expr::new(ExprKind::Bool(true), sp(243, 247))),
                        rhs: Box::new(Expr::new(
                            ExprKind::Unary {
                                op: UnaryOp::Not,
                                op_span: sp(252, 255),
                                expr: Box::new(Expr::new(ExprKind::Bool(false), sp(256, 261))),
                            },
                            sp(252, 261),
                        )),
                    },
                    sp(243, 261),
                ),
                body: Block {
                    stmts: vec![Stmt {
                        kind: StmtKind::Pass,
                        span: sp(270, 274),
                    }],
                    span: sp(270, 274),
                },
            },
            span: sp(237, 274),
        },
        Stmt {
            kind: StmtKind::For {
                var: ident("i", 280),
                iter: Expr::new(
                    ExprKind::Call {
                        callee: Box::new(name("range", 285)),
                        type_args: Some(vec![named_ty("int", 291, vec![])]),
                        args: vec![
                            Arg {
                                name: None,
                                value: int(0, 296),
                                span: sp(296, 297),
                            },
                            Arg {
                                name: Some(ident("stop", 299)),
                                value: int(9, 304),
                                span: sp(299, 305),
                            },
                        ],
                    },
                    sp(285, 306),
                ),
                body: Block {
                    stmts: vec![Stmt {
                        kind: StmtKind::Expr(Expr::new(
                            ExprKind::FString(vec![
                                FStringPart::Literal {
                                    value: "x=".into(),
                                    span: sp(315, 317),
                                },
                                FStringPart::Expr {
                                    expr: Box::new(name("x", 318)),
                                    spec: Some(".2f".into()),
                                    spec_span: Some(sp(320, 323)),
                                    span: sp(317, 324),
                                },
                            ]),
                            sp(313, 325),
                        )),
                        span: sp(313, 325),
                    }],
                    span: sp(313, 325),
                },
            },
            span: sp(276, 325),
        },
        Stmt {
            kind: StmtKind::Return(Some(Expr::new(
                ExprKind::Dict(vec![(
                    Expr::new(ExprKind::Str("k".into()), sp(340, 343)),
                    Expr::new(
                        ExprKind::Index {
                            base: Box::new(Expr::new(
                                ExprKind::Attribute {
                                    base: Box::new(name("self", 345)),
                                    attr: ident("items", 350),
                                },
                                sp(345, 355),
                            )),
                            index: Box::new(int(0, 356)),
                        },
                        sp(345, 358),
                    ),
                )]),
                sp(339, 359),
            ))),
            span: sp(332, 359),
        },
        Stmt {
            kind: StmtKind::Expr(Expr::new(
                ExprKind::Tuple(vec![
                    Expr::new(ExprKind::List(vec![int(1, 366)]), sp(365, 368)),
                    Expr::new(ExprKind::Set(vec![int(2, 371)]), sp(370, 373)),
                ]),
                sp(364, 374),
            )),
            span: sp(364, 374),
        },
    ];
    items.push(Item::Fn(FnDecl {
        name: ident("main", 143),
        generics: vec![],
        params: vec![],
        ret: Some(TypeExpr::new(TypeExprKind::None, sp(147, 151))),
        body: Block {
            stmts,
            span: sp(150, 374),
        },
        span: sp(140, 374),
    }));

    let module = Module {
        items,
        span: sp(0, 374),
    };
    insta::assert_snapshot!(dump(&module));
    // `Display` is the same as `dump`.
    assert_eq!(module.to_string(), dump(&module));
}

#[test]
fn dumps_a_statement_on_its_own() {
    let stmt = Stmt {
        kind: StmtKind::Pass,
        span: sp(4, 8),
    };
    assert_eq!(dump_stmt(&stmt), "Pass @4..8");
}

#[test]
fn dump_has_no_trailing_newline() {
    let module = Module {
        items: vec![],
        span: sp(0, 0),
    };
    assert_eq!(dump(&module), "Module @0..0");
}
