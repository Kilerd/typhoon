pub enum CompareOperator {
    Greater,
    Less,
    Equal,
    GreaterEqual,
    LessEqual,
    NotEqual,
    In,
    NotIn,
    Is,
    IsNot,
}

#[derive(Debug, PartialEq)]
pub enum Atom {
    Identifier(String),
    Int(i64),
    Float(f64),
    String(String),
    Boolean(bool),
    None,
}

#[derive(Debug)]
pub enum Statement {
    Assignment(Atom),
    If,
}

#[derive(Debug, PartialEq)]
pub enum BinOperation {
    Number(Atom),
    Expresion(Box<BinOperation>, BinOperator, Box<BinOperation>),
    Factor(BinOperator, Box<BinOperation>),
    Power(Atom),
}

#[derive(Debug, PartialEq)]
pub enum BinOperator {
    Add,
    Sub,
    Div,
    Mul,
    Pow,
    FloorDiv,
    Invert,
    BitAnd,
    BitXor,
    BitOr,
    LeftShift,
    RightShift,
}
