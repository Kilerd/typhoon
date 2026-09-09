//! The typed high-level IR that `sema` hands to `codegen`.
//!
//! The HIR is what is left of the AST after name resolution, type checking and
//! desugaring:
//!
//! * every name is resolved — locals are [`LocalId`]s, calls carry a
//!   [`FuncId`], top-level constants are folded into literals;
//! * every expression carries its [`TypeId`], and every operator is already
//!   split per operand type (`IntBin` / `FloatBin` / …), so `codegen` never
//!   has to ask "what type is this?";
//! * `elif` chains become nested [`Stmt::If`]s, `for x in range(...)` becomes
//!   [`Stmt::ForRange`], `print(...)` becomes [`ExprKind::Print`], keyword and
//!   defaulted call arguments are reordered into parameter order.
//!
//! `codegen` consumes this and nothing else (DESIGN §6.1).

use typhoon_diag::Span;

use crate::types::{ClassId, TypeId};

/// Index of a function in [`Program::functions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FuncId(pub u32);

impl FuncId {
    /// The raw index.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Index of a local variable in [`Function::locals`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocalId(pub u32);

impl LocalId {
    /// The raw index.
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A whole type-checked program.
#[derive(Debug, Clone)]
pub struct Program {
    /// The type table every [`TypeId`] in this program refers to.
    pub types: crate::types::TypeTable,
    /// Every user function, in source order.
    pub functions: Vec<Function>,
    /// The `main` function (DESIGN §3.3).
    pub main: FuncId,
}

impl Program {
    /// Looks a function up by id.
    pub fn function(&self, id: FuncId) -> &Function {
        &self.functions[id.index()]
    }
}

/// One user function.
#[derive(Debug, Clone)]
pub struct Function {
    /// The name as written in the source.
    pub name: String,
    /// The mangled symbol name (`ty_user_<name>`), so that user code can never
    /// collide with a C symbol.
    pub symbol: String,
    /// The parameters, as locals, in declaration order.
    pub params: Vec<LocalId>,
    /// The return type; [`TypeId::UNIT`] for a function without `->`.
    pub ret: TypeId,
    /// Every local of the function, parameters first.
    pub locals: Vec<Local>,
    /// The body.
    pub body: Block,
    /// Span of the declaration.
    pub span: Span,
}

/// One local variable (or parameter) of a function.
#[derive(Debug, Clone)]
pub struct Local {
    /// The name as written in the source.
    pub name: String,
    /// Its type, fixed at the first assignment (DESIGN §3.4).
    pub ty: TypeId,
    /// Span of the declaration (the first assignment).
    pub span: Span,
    /// Whether this local is a parameter.
    pub is_param: bool,
}

/// A sequence of statements.
#[derive(Debug, Clone, Default)]
pub struct Block {
    /// The statements.
    pub stmts: Vec<Stmt>,
}

/// A statement of the typed IR.
#[derive(Debug, Clone)]
pub enum Stmt {
    /// Store into a local. The local's type always equals the value's type.
    Assign {
        /// The assigned local.
        local: LocalId,
        /// The value.
        value: Expr,
    },
    /// An expression evaluated for its side effects.
    Expr(Expr),
    /// `if cond: then else: else_`; `elif` chains are nested in `else_`.
    If {
        /// The condition, always of type `bool`.
        cond: Expr,
        /// The taken branch.
        then: Block,
        /// The `else` branch, if any.
        else_: Option<Block>,
    },
    /// `while cond: body`.
    While {
        /// The condition, always of type `bool`.
        cond: Expr,
        /// The body.
        body: Block,
    },
    /// `for var in range(start, stop, step): body`, a counting loop that
    /// allocates nothing (DESIGN §3.5).
    ForRange {
        /// The loop variable, an ordinary `int` local.
        var: LocalId,
        /// First value.
        start: Expr,
        /// Exclusive bound.
        stop: Expr,
        /// Increment; must not be zero (checked at runtime when it is not a
        /// literal).
        step: Expr,
        /// The body.
        body: Block,
    },
    /// `for var in <list or str>: body` (DESIGN §3.5).
    ForEach {
        /// The loop variable, a normal local of the element type.
        var: LocalId,
        /// The iterated `list<T>` or `str`.
        iter: Expr,
        /// What is being iterated.
        over: IterKind,
        /// The body.
        body: Block,
    },
    /// `xs[i] = value`, with the bounds check of DESIGN §4.3.
    SetIndex {
        /// The indexed `list<T>`.
        list: Expr,
        /// The index; negative counts from the end.
        index: Expr,
        /// The stored value, of the element type.
        value: Expr,
    },
    /// `obj.field = value`.
    SetField {
        /// The receiver.
        obj: Expr,
        /// Its class.
        class: ClassId,
        /// Index of the field in the class's declaration order.
        field: u32,
        /// The stored value.
        value: Expr,
    },
    /// Several statements standing in for one source statement; produced by
    /// tuple unpacking (`a, b = t` evaluates `t` once into a temporary).
    Group(Block),
    /// `return` or `return value`.
    Return(Option<Expr>),
    /// `break`.
    Break,
    /// `continue`.
    Continue,
}

/// What a [`Stmt::ForEach`] walks over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IterKind {
    /// A `list<T>`; the loop variable has type `T`.
    List,
    /// A `str`; the loop variable is a length-1 `str` holding one code point
    /// (DESIGN §3.5, §4.3).
    Str,
}

/// A typed expression.
#[derive(Debug, Clone)]
pub struct Expr {
    /// What it computes.
    pub kind: ExprKind,
    /// Its type.
    pub ty: TypeId,
    /// Where it was written.
    pub span: Span,
}

/// Arithmetic and bitwise operations on `int` (DESIGN §4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntOp {
    /// Wrapping `+`.
    Add,
    /// Wrapping `-`.
    Sub,
    /// Wrapping `*`.
    Mul,
    /// `//`, flooring; panics on a zero divisor.
    FloorDiv,
    /// `%`, flooring; panics on a zero divisor.
    Mod,
    /// `**`, via `ty_int_pow`; panics on a negative exponent.
    Pow,
    /// `&`.
    BitAnd,
    /// `|`.
    BitOr,
    /// `^`.
    BitXor,
    /// `<<`; panics on a negative shift, yields 0 past 63 bits.
    Shl,
    /// `>>`, arithmetic; panics on a negative shift.
    Shr,
}

/// Arithmetic on `float` (DESIGN §4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatOp {
    /// `+`.
    Add,
    /// `-`.
    Sub,
    /// `*`.
    Mul,
    /// `/`, IEEE-754, no panic on zero.
    Div,
    /// `//`, `floor(a / b)`.
    FloorDiv,
    /// `%`, `a - b * floor(a / b)`.
    Mod,
    /// `**`, via `llvm.pow.f64`.
    Pow,
}

/// The comparison operators that survive into the HIR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
}

/// `min` / `max` (DESIGN §4.6), two arguments only in M1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MinMax {
    /// `min(a, b)`
    Min,
    /// `max(a, b)`
    Max,
}

/// One piece of an f-string after lowering.
#[derive(Debug, Clone)]
pub enum FStringPart {
    /// Literal text.
    Literal(String),
    /// An interpolated value, rendered with `spec`.
    Value {
        /// The value.
        expr: Expr,
        /// How to render it.
        spec: FormatSpec,
    },
}

/// The subset of Python's format mini-language typhoon accepts (DESIGN §3.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatSpec {
    /// No spec, or a spec that only restates the natural rendering
    /// (`d` for `int`, `s` for `str`): the value is printed as `print` would.
    Display,
    /// `.Nf`: a `float` with exactly `N` digits after the decimal point.
    Fixed(u32),
}

/// What an [`Expr`] computes.
#[derive(Debug, Clone)]
pub enum ExprKind {
    /// An `int` literal.
    Int(i64),
    /// A `float` literal.
    Float(f64),
    /// A `bool` literal.
    Bool(bool),
    /// A `str` literal.
    Str(String),
    /// The unit value `None`.
    Unit,
    /// Reading a local variable.
    Local(LocalId),
    /// Wrapping integer negation.
    IntNeg(Box<Expr>),
    /// Bitwise `~`.
    IntNot(Box<Expr>),
    /// Float negation.
    FloatNeg(Box<Expr>),
    /// Logical `not` on a `bool`.
    Not(Box<Expr>),
    /// An `int` binary operation.
    IntBin {
        /// The operation.
        op: IntOp,
        /// Left operand.
        lhs: Box<Expr>,
        /// Right operand.
        rhs: Box<Expr>,
    },
    /// `int ** 2`, lowered to a multiplication.
    IntSquare(Box<Expr>),
    /// A `float` binary operation.
    FloatBin {
        /// The operation.
        op: FloatOp,
        /// Left operand.
        lhs: Box<Expr>,
        /// Right operand.
        rhs: Box<Expr>,
    },
    /// `x ** 0.5`, lowered to `llvm.sqrt.f64`.
    FloatSqrt(Box<Expr>),
    /// `x ** 2.0`, lowered to a multiplication.
    FloatSquare(Box<Expr>),
    /// `int / int`, which yields a `float` (DESIGN §3.6); panics on a zero
    /// divisor.
    IntDiv {
        /// Left operand.
        lhs: Box<Expr>,
        /// Right operand.
        rhs: Box<Expr>,
    },
    /// Comparison of two `int`s.
    IntCmp {
        /// The operator.
        op: CmpOp,
        /// Left operand.
        lhs: Box<Expr>,
        /// Right operand.
        rhs: Box<Expr>,
    },
    /// Comparison of two `float`s, with ordered predicates (NaN compares
    /// false, except for `!=`).
    FloatCmp {
        /// The operator.
        op: CmpOp,
        /// Left operand.
        lhs: Box<Expr>,
        /// Right operand.
        rhs: Box<Expr>,
    },
    /// `==` / `!=` on `bool`.
    BoolCmp {
        /// The operator.
        op: CmpOp,
        /// Left operand.
        lhs: Box<Expr>,
        /// Right operand.
        rhs: Box<Expr>,
    },
    /// `==` / `!=` on `str`, via `ty_str_eq`.
    StrCmp {
        /// The operator.
        op: CmpOp,
        /// Left operand.
        lhs: Box<Expr>,
        /// Right operand.
        rhs: Box<Expr>,
    },
    /// `str + str`, via `ty_str_concat`.
    StrConcat {
        /// Left operand.
        lhs: Box<Expr>,
        /// Right operand.
        rhs: Box<Expr>,
    },
    /// Short-circuiting `and`.
    And {
        /// Left operand.
        lhs: Box<Expr>,
        /// Right operand.
        rhs: Box<Expr>,
    },
    /// Short-circuiting `or`.
    Or {
        /// Left operand.
        lhs: Box<Expr>,
        /// Right operand.
        rhs: Box<Expr>,
    },
    /// A call to a user function.
    Call {
        /// The callee.
        func: FuncId,
        /// The arguments, in *parameter* order, defaults already substituted.
        args: Vec<Expr>,
        /// Indices into `args` in source evaluation order (DESIGN §3.2:
        /// keyword arguments are matched by name but evaluated as written).
        eval_order: Vec<u32>,
    },
    /// `float(x)` on an `int`: `sitofp`.
    IntToFloat(Box<Expr>),
    /// `int(x)` on a `float`: a saturating `fptosi`.
    FloatToInt(Box<Expr>),
    /// `float(x)` on a `float`: an explicit conversion, which is a
    /// floating-point contraction barrier (DESIGN §4.3).
    FloatFence(Box<Expr>),
    /// `abs(x)` on an `int` (wrapping: `abs(-2**63)` is `-2**63`).
    IntAbs(Box<Expr>),
    /// `abs(x)` on a `float`.
    FloatAbs(Box<Expr>),
    /// `min` / `max` of two `int`s.
    IntMinMax {
        /// Which one.
        op: MinMax,
        /// Left operand.
        lhs: Box<Expr>,
        /// Right operand.
        rhs: Box<Expr>,
    },
    /// `min` / `max` of two `float`s.
    FloatMinMax {
        /// Which one.
        op: MinMax,
        /// Left operand.
        lhs: Box<Expr>,
        /// Right operand.
        rhs: Box<Expr>,
    },
    /// `print(...)`: the arguments separated by single spaces, then a newline.
    Print(Vec<Expr>),
    /// An f-string.
    FString(Vec<FStringPart>),

    // -- str ---------------------------------------------------------------
    /// `len(s)`, the number of code points, read from the string header.
    StrLen(Box<Expr>),
    /// One of the `str` methods of DESIGN §4.6.
    StrMethod {
        /// Which method.
        op: StrMethod,
        /// The receiver.
        recv: Box<Expr>,
        /// The arguments, already checked against the method's signature.
        args: Vec<Expr>,
    },
    /// `str(x)` on an `int`, `float` or `bool`.
    StrFrom(Box<Expr>),

    // -- list --------------------------------------------------------------
    /// A list literal; `elem` is the element type even when `items` is empty.
    ListNew {
        /// The element type.
        elem: TypeId,
        /// The elements, in order.
        items: Vec<Expr>,
    },
    /// `xs[i]`, with the bounds check of DESIGN §4.3.
    ListGet {
        /// The list.
        list: Box<Expr>,
        /// The index; negative counts from the end.
        index: Box<Expr>,
    },
    /// `len(xs)`, read from the list header.
    ListLen(Box<Expr>),
    /// `xs.append(v)`.
    ListAppend {
        /// The list.
        list: Box<Expr>,
        /// The appended value.
        value: Box<Expr>,
    },
    /// `xs.pop()`; panics on an empty list.
    ListPop(Box<Expr>),
    /// `xs.insert(i, v)`, with Python's index clamping.
    ListInsert {
        /// The list.
        list: Box<Expr>,
        /// Where to insert.
        index: Box<Expr>,
        /// The inserted value.
        value: Box<Expr>,
    },
    /// `xs.clear()`.
    ListClear(Box<Expr>),
    /// `xs + ys`, a fresh list.
    ListConcat {
        /// Left operand.
        lhs: Box<Expr>,
        /// Right operand.
        rhs: Box<Expr>,
    },
    /// `v in xs` / `v in s` (and their `not in` forms).
    Contains {
        /// The `list<T>` or `str` searched.
        haystack: Box<Expr>,
        /// The element or substring looked for.
        needle: Box<Expr>,
        /// Whether the operator was `not in`.
        negated: bool,
    },

    // -- tuple -------------------------------------------------------------
    /// A tuple literal, a value aggregate (DESIGN §4.2).
    TupleNew(Vec<Expr>),
    /// `t[k]`, where `k` is a constant checked against the arity.
    TupleGet {
        /// The tuple.
        tuple: Box<Expr>,
        /// The member index.
        index: u32,
    },

    // -- class -------------------------------------------------------------
    /// `C(field=…)`: the generated keyword constructor (DESIGN §3.8).
    New {
        /// The class.
        class: ClassId,
        /// One initializer per field, in declaration order, defaults filled in.
        fields: Vec<Expr>,
        /// Indices into `fields` in source evaluation order: the constructor
        /// matches by name but evaluates as written (DESIGN §3.2). Field
        /// defaults are constants and are not listed.
        eval_order: Vec<u32>,
    },
    /// `obj.field`.
    GetField {
        /// The receiver.
        obj: Box<Expr>,
        /// Its class.
        class: ClassId,
        /// Index of the field in the class's declaration order.
        field: u32,
    },
    /// The `None` of a `C | None`: a null reference.
    NoneRef,
    /// `x is None` / `x is not None`.
    IsNone {
        /// The tested reference.
        value: Box<Expr>,
        /// Whether the operator was `is not`.
        negated: bool,
    },
    /// `a is b` / `a is not b` on two class references: pointer identity.
    RefEq {
        /// Left operand.
        lhs: Box<Expr>,
        /// Right operand.
        rhs: Box<Expr>,
        /// Whether the operator was `is not`.
        negated: bool,
    },
    /// `==` / `!=` on two lists or two tuples, compared member by member.
    StructEq {
        /// The operator.
        op: CmpOp,
        /// Left operand.
        lhs: Box<Expr>,
        /// Right operand.
        rhs: Box<Expr>,
    },
}

/// The `str` methods of DESIGN §4.6. `str` is immutable, so each one returns a
/// fresh value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrMethod {
    /// `s.upper()`, ASCII-only case mapping.
    Upper,
    /// `s.lower()`, ASCII-only case mapping.
    Lower,
    /// `s.strip()`, ASCII whitespace on both ends.
    Strip,
    /// `s.split(sep)` into a `list<str>`; an empty separator panics.
    Split,
    /// `sep.join(parts)`.
    Join,
    /// `s.startswith(prefix)`.
    StartsWith,
    /// `s.endswith(suffix)`.
    EndsWith,
    /// `s.find(sub)`, a code-point index or `-1`.
    Find,
    /// `s.replace(old, new)`.
    Replace,
}

impl StrMethod {
    /// The name as written in source.
    pub fn as_str(self) -> &'static str {
        match self {
            StrMethod::Upper => "upper",
            StrMethod::Lower => "lower",
            StrMethod::Strip => "strip",
            StrMethod::Split => "split",
            StrMethod::Join => "join",
            StrMethod::StartsWith => "startswith",
            StrMethod::EndsWith => "endswith",
            StrMethod::Find => "find",
            StrMethod::Replace => "replace",
        }
    }
}
