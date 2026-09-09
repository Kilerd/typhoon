//! The syntax tree: pure data, every node carries a [`Span`].

use typhoon_diag::Span;

use crate::ops::{BinOp, BoolOp, CmpOp, UnaryOp};

/// An identifier together with the span it was written at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ident {
    /// The identifier text.
    pub name: String,
    /// Where it was written.
    pub span: Span,
}

impl Ident {
    /// Creates an identifier.
    pub fn new(name: impl Into<String>, span: Span) -> Ident {
        Ident {
            name: name.into(),
            span,
        }
    }

    /// The identifier text.
    pub fn as_str(&self) -> &str {
        &self.name
    }
}

/// A whole source file.
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    /// The top-level declarations, in source order.
    pub items: Vec<Item>,
    /// The span of the whole file.
    pub span: Span,
}

/// A top-level declaration. DESIGN 3.3: only declarations may appear at the
/// top level, never statements.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// `fn name(...) -> T:`
    Fn(FnDecl),
    /// `class Name:`
    Class(ClassDecl),
    /// `NAME: T = value`
    Const(ConstDecl),
}

impl Item {
    /// The span of the whole declaration.
    pub fn span(&self) -> Span {
        match self {
            Item::Fn(d) => d.span,
            Item::Class(d) => d.span,
            Item::Const(d) => d.span,
        }
    }

    /// The declared name.
    pub fn name(&self) -> &Ident {
        match self {
            Item::Fn(d) => &d.name,
            Item::Class(d) => &d.name,
            Item::Const(d) => &d.name,
        }
    }
}

/// One generic parameter, e.g. the `T` of `fn max<T>(...)`.
///
/// MVP has no bounds (DESIGN 3.9), so a parameter is just a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericParam {
    /// The parameter name.
    pub name: Ident,
    /// Span of the parameter.
    pub span: Span,
}

/// One function parameter.
///
/// `self` is represented as a `Param` with [`Param::is_self`] set and
/// [`Param::ty`] `None` (DESIGN 3.8: `self` is never annotated). A non-`self`
/// parameter with `ty == None` only occurs after a syntax error; the parser has
/// already reported it.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    /// The parameter name.
    pub name: Ident,
    /// The declared type, `None` for `self` (or after an error).
    pub ty: Option<TypeExpr>,
    /// The default value, if any. Must be a constant expression (checked by sema).
    pub default: Option<Expr>,
    /// Whether this parameter is the `self` receiver of a method.
    pub is_self: bool,
    /// Span of the whole parameter, including type and default.
    pub span: Span,
}

/// A function or method declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct FnDecl {
    /// The function name.
    pub name: Ident,
    /// Generic parameters, empty for a non-generic function.
    pub generics: Vec<GenericParam>,
    /// Parameters in declaration order.
    pub params: Vec<Param>,
    /// The declared return type; `None` means the function returns unit.
    pub ret: Option<TypeExpr>,
    /// The function body.
    pub body: Block,
    /// Span from the `fn` keyword to the end of the body.
    pub span: Span,
}

impl FnDecl {
    /// Whether the first parameter is `self`, i.e. this is a method.
    pub fn is_method(&self) -> bool {
        self.params.first().is_some_and(|p| p.is_self)
    }
}

/// A class field declaration, e.g. `x: float` or `count: int = 0`.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    /// The field name.
    pub name: Ident,
    /// The declared type; fields are always annotated.
    pub ty: TypeExpr,
    /// The default value used by the generated keyword constructor.
    pub default: Option<Expr>,
    /// Span of the field declaration.
    pub span: Span,
}

/// A `class` declaration (DESIGN 3.8).
#[derive(Debug, Clone, PartialEq)]
pub struct ClassDecl {
    /// The class name.
    pub name: Ident,
    /// Generic parameters, empty for a non-generic class.
    pub generics: Vec<GenericParam>,
    /// Fields, in declaration order.
    pub fields: Vec<Field>,
    /// Methods, in declaration order.
    pub methods: Vec<FnDecl>,
    /// Span from the `class` keyword to the end of the body.
    pub span: Span,
}

/// A top-level constant, e.g. `PI: float = 3.14159`.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstDecl {
    /// The constant name.
    pub name: Ident,
    /// Its declared type; constants are always annotated (DESIGN 3.3).
    pub ty: TypeExpr,
    /// The initializer, which must be a constant expression (checked by sema).
    pub value: Expr,
    /// Span of the whole declaration.
    pub span: Span,
}

/// An indented block of statements.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    /// The statements of the block.
    pub stmts: Vec<Stmt>,
    /// Span covering the block body.
    pub span: Span,
}

/// A statement: a [`StmtKind`] plus its span.
#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    /// What kind of statement this is.
    pub kind: StmtKind,
    /// Span of the statement, excluding the terminating newline.
    pub span: Span,
}

/// The `elif cond:` arm of an `if` statement.
#[derive(Debug, Clone, PartialEq)]
pub struct ElifBranch {
    /// The condition.
    pub cond: Expr,
    /// The body.
    pub body: Block,
    /// Span from `elif` to the end of the body.
    pub span: Span,
}

/// The kinds of statement typhoon has (DESIGN 3.4, 3.5).
///
/// There is deliberately no augmented assignment (`x += 1`); the parser reports
/// a dedicated diagnostic for it.
#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    /// An expression evaluated for its effect, e.g. `print(x)`.
    Expr(Expr),
    /// `target = value`. `target` is a `Name`, `Attribute` or `Index`.
    Assign {
        /// The assignment target.
        target: Expr,
        /// The assigned value.
        value: Expr,
    },
    /// `name: T` or `name: T = value`.
    AnnAssign {
        /// The declared name.
        name: Ident,
        /// The declared type.
        ty: TypeExpr,
        /// The initializer; `None` only after an error inside a function body.
        value: Option<Expr>,
    },
    /// `return` or `return value`.
    Return(Option<Expr>),
    /// `if` / `elif` / `else`.
    If {
        /// The `if` condition.
        cond: Expr,
        /// The `if` body.
        then: Block,
        /// Zero or more `elif` arms.
        elifs: Vec<ElifBranch>,
        /// The optional `else` body.
        else_: Option<Block>,
    },
    /// `while cond:`.
    While {
        /// The loop condition.
        cond: Expr,
        /// The loop body.
        body: Block,
    },
    /// `for var in iter:`.
    For {
        /// The loop variable.
        var: Ident,
        /// The iterated expression.
        iter: Expr,
        /// The loop body.
        body: Block,
    },
    /// `break`.
    Break,
    /// `continue`.
    Continue,
    /// `pass`.
    Pass,
}

/// One part of an f-string.
#[derive(Debug, Clone, PartialEq)]
pub enum FStringPart {
    /// Literal text between the `{...}` holes; `{{` and `}}` are already
    /// decoded into single braces.
    Literal {
        /// The literal text.
        value: String,
        /// Span of the text inside the string literal.
        span: Span,
    },
    /// A `{expr}` or `{expr:spec}` hole.
    Expr {
        /// The interpolated expression.
        expr: Box<Expr>,
        /// The raw format spec after `:`, without the colon (DESIGN 3.7).
        spec: Option<String>,
        /// Span of the format spec text, if there is one.
        spec_span: Option<Span>,
        /// Span of the whole hole, including the braces.
        span: Span,
    },
}

/// One argument at a call site.
#[derive(Debug, Clone, PartialEq)]
pub struct Arg {
    /// `Some(name)` for a keyword argument `name=value`.
    pub name: Option<Ident>,
    /// The argument expression.
    pub value: Expr,
    /// Span of the whole argument.
    pub span: Span,
}

/// The `op rhs` tail of a comparison, e.g. the `< c` of `a < b < c`.
#[derive(Debug, Clone, PartialEq)]
pub struct CompareTail {
    /// The comparison operator.
    pub op: CmpOp,
    /// Span of the operator itself (`is not` spans both keywords).
    pub op_span: Span,
    /// The right-hand operand.
    pub rhs: Expr,
}

/// An expression: an [`ExprKind`] plus its span.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    /// What kind of expression this is.
    pub kind: ExprKind,
    /// Span of the expression.
    pub span: Span,
}

impl Expr {
    /// Creates an expression node.
    pub fn new(kind: ExprKind, span: Span) -> Expr {
        Expr { kind, span }
    }

    /// Whether this expression is a valid assignment target
    /// (`Name`, `Attribute`, `Index`, or a non-empty tuple of those, which is
    /// the unpacking form `a, b = ...`).
    pub fn is_assign_target(&self) -> bool {
        match &self.kind {
            ExprKind::Name(_) | ExprKind::Attribute { .. } | ExprKind::Index { .. } => true,
            ExprKind::Tuple(items) => !items.is_empty() && items.iter().all(Expr::is_assign_target),
            _ => false,
        }
    }
}

/// The kinds of expression typhoon has (DESIGN 3.6, 3.7).
///
/// Parenthesized expressions do **not** get their own node: `(a + b)` parses to
/// the `Binary` node for `a + b`, with the span of `a + b` (not including the
/// parentheses). `(1, "a")` is a [`ExprKind::Tuple`].
#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// An integer literal. Out-of-range literals are reported by the lexer.
    Int(i64),
    /// A float literal.
    Float(f64),
    /// A string literal with escapes already decoded.
    Str(String),
    /// An f-string literal.
    FString(Vec<FStringPart>),
    /// `True` / `False`.
    Bool(bool),
    /// `None`.
    None,
    /// A variable or function name.
    Name(Ident),
    /// `base.attr`.
    Attribute {
        /// The receiver.
        base: Box<Expr>,
        /// The attribute name.
        attr: Ident,
    },
    /// `callee(args)` or `callee<T>(args)`.
    Call {
        /// The called expression.
        callee: Box<Expr>,
        /// Explicit type arguments, `None` when none were written (DESIGN 3.9).
        type_args: Option<Vec<TypeExpr>>,
        /// The arguments, in source order.
        args: Vec<Arg>,
    },
    /// `base[index]`.
    Index {
        /// The indexed expression.
        base: Box<Expr>,
        /// The index expression.
        index: Box<Expr>,
    },
    /// A prefix operator application.
    Unary {
        /// The operator.
        op: UnaryOp,
        /// Span of the operator token.
        op_span: Span,
        /// The operand.
        expr: Box<Expr>,
    },
    /// An arithmetic or bitwise binary operation.
    Binary {
        /// The operator.
        op: BinOp,
        /// Span of the operator token.
        op_span: Span,
        /// Left operand.
        lhs: Box<Expr>,
        /// Right operand.
        rhs: Box<Expr>,
    },
    /// `lhs and rhs` / `lhs or rhs`.
    BoolOp {
        /// The operator.
        op: BoolOp,
        /// Span of the operator token.
        op_span: Span,
        /// Left operand.
        lhs: Box<Expr>,
        /// Right operand.
        rhs: Box<Expr>,
    },
    /// A comparison, possibly chained. Chains parse but sema rejects them
    /// until M3 (DESIGN 3.6).
    Compare {
        /// The left-most operand.
        left: Box<Expr>,
        /// One entry per `op rhs` pair; a chain has more than one.
        tail: Vec<CompareTail>,
    },
    /// `[a, b]`.
    List(Vec<Expr>),
    /// `{k: v}`. An empty `{}` is a dict, not a set (DESIGN 3.7).
    Dict(Vec<(Expr, Expr)>),
    /// `{a, b}`.
    Set(Vec<Expr>),
    /// `(a, b)`.
    Tuple(Vec<Expr>),
    /// A placeholder produced by parser error recovery. A diagnostic has
    /// already been reported; later phases should ignore these nodes.
    Error,
}

/// A type as written in the source, e.g. `int`, `list<int>`, `int | None`.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeExpr {
    /// What kind of type expression this is.
    pub kind: TypeExprKind,
    /// Span of the type expression.
    pub span: Span,
}

impl TypeExpr {
    /// Creates a type expression node.
    pub fn new(kind: TypeExprKind, span: Span) -> TypeExpr {
        TypeExpr { kind, span }
    }
}

/// The kinds of type expression (DESIGN 3.9, 3.10, 4.1).
#[derive(Debug, Clone, PartialEq)]
pub enum TypeExprKind {
    /// A named type with optional type arguments: `int`, `list<int>`,
    /// `dict<str, list<int>>`, `Stack<T>`.
    Named {
        /// The type name.
        name: Ident,
        /// Type arguments, empty when none were written.
        args: Vec<TypeExpr>,
    },
    /// `A | B | ...`, in practice `T | None` (DESIGN 3.10, M3).
    Union(Vec<TypeExpr>),
    /// The unit type, written `None`.
    None,
    /// A placeholder produced by parser error recovery.
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;
    use typhoon_diag::FileId;

    const F: FileId = FileId(0);

    fn sp(a: u32, b: u32) -> Span {
        Span::new(F, a, b)
    }

    fn ident(name: &str) -> Ident {
        Ident::new(name, sp(0, name.len() as u32))
    }

    fn name_expr(name: &str) -> Expr {
        Expr::new(ExprKind::Name(ident(name)), sp(0, name.len() as u32))
    }

    fn ty(name: &str) -> TypeExpr {
        TypeExpr::new(
            TypeExprKind::Named {
                name: ident(name),
                args: vec![],
            },
            sp(0, 3),
        )
    }

    fn body() -> Block {
        Block {
            stmts: vec![],
            span: sp(0, 0),
        }
    }

    fn fn_decl(name: &str, params: Vec<Param>) -> FnDecl {
        FnDecl {
            name: ident(name),
            generics: vec![],
            params,
            ret: None,
            body: body(),
            span: sp(0, 10),
        }
    }

    fn param(name: &str, is_self: bool) -> Param {
        Param {
            name: ident(name),
            ty: if is_self { None } else { Some(ty("int")) },
            default: None,
            is_self,
            span: sp(0, 1),
        }
    }

    #[test]
    fn ident_accessors() {
        let i = ident("total");
        assert_eq!(i.as_str(), "total");
        assert_eq!(i.name, "total");
        assert_eq!(i.span, sp(0, 5));
    }

    #[test]
    fn item_span_and_name() {
        let f = Item::Fn(fn_decl("main", vec![]));
        assert_eq!(f.name().as_str(), "main");
        assert_eq!(f.span(), sp(0, 10));

        let c = Item::Class(ClassDecl {
            name: ident("Point"),
            generics: vec![],
            fields: vec![],
            methods: vec![],
            span: sp(3, 20),
        });
        assert_eq!(c.name().as_str(), "Point");
        assert_eq!(c.span(), sp(3, 20));

        let k = Item::Const(ConstDecl {
            name: ident("PI"),
            ty: ty("float"),
            value: Expr::new(ExprKind::Float(3.0), sp(0, 3)),
            span: sp(1, 9),
        });
        assert_eq!(k.name().as_str(), "PI");
        assert_eq!(k.span(), sp(1, 9));
    }

    #[test]
    fn is_method_checks_the_first_param() {
        assert!(!fn_decl("free", vec![]).is_method());
        assert!(!fn_decl("free", vec![param("a", false)]).is_method());
        assert!(fn_decl("dist", vec![param("self", true)]).is_method());
        // `self` in a later position is not a receiver.
        assert!(!fn_decl("odd", vec![param("a", false), param("self", true)]).is_method());
    }

    #[test]
    fn self_param_has_no_type() {
        let p = param("self", true);
        assert!(p.is_self);
        assert!(p.ty.is_none());
    }

    #[test]
    fn assign_targets() {
        assert!(name_expr("x").is_assign_target());

        let attr = Expr::new(
            ExprKind::Attribute {
                base: Box::new(name_expr("p")),
                attr: ident("x"),
            },
            sp(0, 3),
        );
        assert!(attr.is_assign_target());

        let index = Expr::new(
            ExprKind::Index {
                base: Box::new(name_expr("xs")),
                index: Box::new(Expr::new(ExprKind::Int(0), sp(3, 4))),
            },
            sp(0, 5),
        );
        assert!(index.is_assign_target());

        assert!(!Expr::new(ExprKind::Int(1), sp(0, 1)).is_assign_target());
        let call = Expr::new(
            ExprKind::Call {
                callee: Box::new(name_expr("f")),
                type_args: None,
                args: vec![],
            },
            sp(0, 3),
        );
        assert!(!call.is_assign_target());
    }

    #[test]
    fn nodes_are_clone_and_eq() {
        let a = fn_decl("main", vec![param("a", false)]);
        let b = a.clone();
        assert_eq!(a, b);
        let mut c = a.clone();
        c.name = ident("other");
        assert_ne!(a, c);
    }
}
