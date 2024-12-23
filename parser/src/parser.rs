use ast::*;
use nom::{
    branch::alt,
    bytes::complete::{tag, take_while1},
    character::complete::{alpha1, alphanumeric1, char, digit1, multispace0, one_of},
    combinator::{map, opt, recognize},
    error::ParseError,
    multi::{many0, many1, separated_list0},
    sequence::{delimited, pair, preceded, terminated, tuple},
    IResult, InputLength, Parser,
};
use nom_locate::{position, LocatedSpan};

type Span<'a> = LocatedSpan<&'a str>;

/// Parses a comma-separated list with optional trailing comma
/// Example: "a, b, c," or "a, b, c" both parse to vec!["a", "b", "c"]
fn separated_list0_trailing<I, O, O2, E, F, G, G2>(
    mut sep: G,
    mut f: F,
    mut trailing: G2,
) -> impl FnMut(I) -> IResult<I, Vec<O>, E>
where
    I: Clone + InputLength,
    F: Parser<I, O, E>,
    G: Parser<I, O2, E>,
    G2: Parser<I, O2, E>,
    E: ParseError<I>,
{
    terminated(separated_list0(sep, f), opt(trailing))
}

/// Identifier parser
/// rule: [a-zA-Z_][a-zA-Z0-9_]*
fn identifier(input: Span) -> IResult<Span, String> {
    let (input, first) = alt((alpha1, tag("_")))(input)?;
    let (input, rest) = many0(alt((alphanumeric1, tag("_"))))(input)?;
    let mut ident = first.to_string();
    for r in rest {
        ident.push_str(&*r);
    }
    Ok((input, ident))
}

/// Type parser
/// rule:
///  - NAMED: [a-zA-Z_][a-zA-Z0-9_]*
///  - UNIT: ()
fn ttype(input: Span) -> IResult<Span, Type> {
    alt((
        map(identifier, |i| Type::named(i)),
        map(delimited(char('('), multispace0, char(')')), |_| {
            Type::void()
        }),
    ))(input)
}

// Number parser
// todo: Parses numeric literals with optional type suffixes:
// - Signed integers: i8, i16, i32 (default)
// - Unsigned integers: u8, u16, u32
// Examples:
//   42i8    -> Number::Integer8(42)
//   123u16  -> Number::UnSignInteger16(123)
//   456     -> Number::Integer32(456)
// Underscores are allowed between digits for readability
fn number(input: Span) -> IResult<Span, Number> {
    let (input, neg) = opt(char('-'))(input)?;
    let (input, num) = recognize(pair(digit1, many0(alt((digit1, tag("_"))))))(input)?;
    let parsed = num.replace("_", "").parse::<i64>().unwrap();
    Ok((
        input,
        Number::Integer64(if neg.is_some() { -parsed } else { parsed }),
    ))
}

// String parser
fn string_literal(input: Span) -> IResult<Span, String> {
    let (input, result) = delimited(
        char('"'),
        many0(alt((
            map(take_while1(|c| c != '"' && c != '\\'), |s: Span| {
                s.to_string()
            }),
            map(preceded(char('\\'), one_of("\"\\/")), |c| c.to_string()),
            map(preceded(char('\\'), one_of("bfnrt")), |c| match c {
                'b' => "\x08".to_string(),
                'f' => "\x0C".to_string(),
                'n' => "\n".to_string(),
                'r' => "\r".to_string(),
                't' => "\t".to_string(),
                _ => unreachable!(),
            }),
        ))),
        char('"'),
    )(input)?;
    Ok((input, result.concat()))
}

// Expression parsers
fn atom(input: Span) -> IResult<Span, Expr> {
    alt((
        map(identifier, |i| Expr::Identifier(i)),
        map(number, |n| Expr::Number(n)),
        map(string_literal, |s| Expr::String(s)),
        delimited(char('('), expression, char(')')),
        block_expression,
    ))(input)
}

fn field_access(input: Span) -> IResult<Span, Expr> {
    let (input, first) = atom(input)?;
    let (input, rest) = many0(preceded(
        delimited(multispace0, char('.'), multispace0),
        atom,
    ))(input)?;

    Ok((
        input,
        rest.into_iter().fold(first, |acc, expr| {
            Expr::Field(Box::new(acc), Box::new(expr))
        }),
    ))
}

fn call_parameters(input: Span) -> IResult<Span, Vec<Expr>> {
    separated_list0(delimited(multispace0, char(','), multispace0), expression)(input)
}

fn call(input: Span) -> IResult<Span, Expr> {
    alt((
        map(
            tuple((
                field_access,
                delimited(
                    delimited(multispace0, char('('), multispace0),
                    call_parameters,
                    delimited(multispace0, char(')'), multispace0),
                ),
            )),
            |(expr, params)| Expr::Call(Box::new(expr), params.into_iter().map(Box::new).collect()),
        ),
        field_access,
    ))(input)
}

fn multiple(input: Span) -> IResult<Span, Expr> {
    let (input, first) = call(input)?;
    let (input, rest) = many0(tuple((
        delimited(multispace0, alt((tag("*"), tag("/"))), multispace0),
        call,
    )))(input)?;

    Ok((
        input,
        rest.into_iter().fold(first, |acc, (op, expr)| {
            Expr::BinOperation(Opcode::from(*op), Box::new(acc), Box::new(expr))
        }),
    ))
}

fn sum(input: Span) -> IResult<Span, Expr> {
    let (input, first) = multiple(input)?;
    let (input, rest) = many0(tuple((
        delimited(multispace0, alt((tag("+"), tag("-"))), multispace0),
        multiple,
    )))(input)?;

    Ok((
        input,
        rest.into_iter().fold(first, |acc, (op, expr)| {
            Expr::BinOperation(Opcode::from(*op), Box::new(acc), Box::new(expr))
        }),
    ))
}

fn expression(input: Span) -> IResult<Span, Expr> {
    alt((sum, call))(input)
}

// Statement parsers
fn let_statement(input: Span) -> IResult<Span, Statement> {
    map(
        tuple((
            preceded(tag("let"), multispace0),
            identifier,
            delimited(multispace0, char(':'), multispace0),
            ttype,
            delimited(multispace0, char('='), multispace0),
            expression,
            delimited(multispace0, char(';'), multispace0),
        )),
        |(_, name, _, typ, _, expr, _)| Statement::Declare(name, typ, Box::new(expr)),
    )(input)
}

fn return_statement(input: Span) -> IResult<Span, Statement> {
    map(
        tuple((
            tag("return"),
            multispace0,
            expression,
            delimited(multispace0, char(';'), multispace0),
        )),
        |(_, _, expr, _)| Statement::Return(Box::new(expr)),
    )(input)
}

fn expression_statement(input: Span) -> IResult<Span, Statement> {
    map(
        terminated(expression, delimited(multispace0, char(';'), multispace0)),
        |expr| Statement::Expr(Box::new(expr)),
    )(input)
}

fn statement(input: Span) -> IResult<Span, Statement> {
    alt((let_statement, return_statement, expression_statement))(input)
}

fn block_expression(input: Span) -> IResult<Span, Expr> {
    map(
        delimited(
            char('{'),
            tuple((
                many0(preceded(multispace0, statement)),
                opt(preceded(multispace0, expression)),
            )),
            preceded(multispace0, char('}')),
        ),
        |(statements, expr)| {
            let mut statements: Vec<Box<Statement>> =
                statements.into_iter().map(Box::new).collect();
            Expr::Block(statements, expr.map(Box::new))
        },
    )(input)
}

// Function and struct parsers
fn function_parameter(input: Span) -> IResult<Span, (String, Type)> {
    tuple((
        identifier,
        preceded(
            tuple((delimited(multispace0, char(':'), multispace0),)),
            ttype,
        ),
    ))(input)
}

fn function_parameters(input: Span) -> IResult<Span, Vec<(String, Type)>> {
    separated_list0(
        delimited(multispace0, char(','), multispace0),
        function_parameter,
    )(input)
}

fn function_declare(input: Span) -> IResult<Span, FunctionDeclare> {
    map(
        tuple((
            tag("fn"),
            multispace0,
            identifier,
            delimited(multispace0, char('('), multispace0),
            function_parameters,
            delimited(multispace0, char(')'), multispace0),
            delimited(multispace0, tag("->"), multispace0),
            ttype,
            preceded(multispace0, block_expression),
        )),
        |(_, _, name, _, params, _, _, return_type, body)| {
            FunctionDeclare::new(name, params, return_type, Box::new(body))
        },
    )(input)
}

fn struct_item(input: Span) -> IResult<Span, (String, Type)> {
    tuple((
        identifier,
        preceded(
            tuple((delimited(multispace0, char(':'), multispace0),)),
            ttype,
        ),
    ))(input)
}

fn struct_items(input: Span) -> IResult<Span, Vec<(String, Type)>> {
    separated_list0_trailing(
        delimited(multispace0, char(','), multispace0),
        struct_item,
        delimited(multispace0, char(','), multispace0),
    )(input)
}

fn struct_define(input: Span) -> IResult<Span, StructDeclare> {
    map(
        tuple((
            tag("struct"),
            multispace0,
            identifier,
            delimited(
                delimited(multispace0, char('{'), multispace0),
                struct_items,
                delimited(multispace0, char('}'), multispace0),
            ),
        )),
        |(_, _, name, items)| StructDeclare::new(name, items),
    )(input)
}

fn module_item(input: Span) -> IResult<Span, ModuleItem> {
    alt((
        map(struct_define, |s| ModuleItem::StructDeclare(s)),
        map(function_declare, |f| ModuleItem::FunctionDeclare(f)),
    ))(input)
}

pub fn parse_module(input: &str) -> Result<Module, nom::Err<nom::error::Error<Span>>> {
    let input = Span::new(input);
    let (_, items) = many1(preceded(multispace0, module_item))(input)?;
    Ok(Module::new(items.into_iter().map(Box::new).collect()))
}
