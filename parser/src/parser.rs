use ast::*;
use pest_consume::{match_nodes, Error, Parser};
use std::str::FromStr;

#[derive(Parser)]
#[grammar = "typhoon.pest"]
pub struct TyphoonParser;

type Result<T> = std::result::Result<T, Error<Rule>>;
type Node<'i> = pest_consume::Node<'i, Rule, ()>;

#[pest_consume::parser]
impl TyphoonParser {
    fn EOI(_input: Node) -> Result<()> {
        Ok(())
    }
    fn module(input:Node) -> Result<Module> {
        let items: Vec<ModuleItem> = match_nodes!(input.into_children();
            [module_item(items).., _] => {
                items.collect()
            },
        );
        let result = items.into_iter().map(Box::new).collect();
        let module = Module::new(result);
        Ok(module)
    }

    fn module_item(input:Node) ->Result<ModuleItem> {
        match_nodes!(input.into_children();
            [struct_define(item)] => {
                Ok(ModuleItem::StructDeclare(item))
            },
            [function_declare(item)] => {
                Ok(ModuleItem::FunctionDeclare(item))
            },
        )
    }

    fn struct_define(input:Node) ->Result<StructDeclare> {
       let (ident, items): (Identifier, Vec<(Identifier, Type)>) = match_nodes!(input.into_children();
            [identifier(ident), struct_items(items)] => {
                (ident, items)
            },
        );
        let declare = StructDeclare::new(ident, items);
        Ok(declare)
    }

    fn struct_items(input: Node) -> Result<Vec<(Identifier, Type)>> {
        match_nodes!(input.into_children();
            [struct_item(items)..] => {
                Ok(items.collect())
            },
        )
    }

    fn struct_item(input: Node) -> Result<(Identifier, Type)> {
        let (ident, ttype): (Identifier, Type) = match_nodes!(input.into_children();
            [identifier(ident), ttype(ttype)] => {
                (ident, ttype)
            },
        );
        Ok((ident, ttype))
    }

    fn function_declare(input: Node) -> Result<FunctionDeclare> {
        let (ident, params, return_type, body): (Identifier, Vec<(Identifier, Type)>, Type, Expr) = match_nodes!(input.into_children();
            [identifier(ident), function_parameters(params), ttype(return_type), block_expression(body)] => {
                (ident, params, return_type, body)
            },
            [identifier(ident), ttype(return_type), block_expression(body)] => {
                (ident, vec![], return_type, body)
            },
        );
        Ok(FunctionDeclare::new(ident, params, return_type, Box::new(body)))
    }
    fn function_parameters(input: Node) -> Result<Vec<(Identifier, Type)>> {
        match_nodes!(input.into_children();
            [function_parameter(params)..] => {
                Ok(params.collect())
            },
        )
    }

    fn function_parameter(input: Node) -> Result<(Identifier, Type)> {
        let (ident, ttype): (Identifier, Type) = match_nodes!(input.into_children();
            [identifier(ident), ttype(ttype)] => {
                (ident, ttype)
            },
        );
        Ok((ident, ttype))
    }

    fn block_expression(input: Node) -> Result<Expr> {
        let (stats, expr): (Vec<Statement>, Option<Expr>) = match_nodes!(input.into_children();
            [statements(stats), expression(expr)] => {
                (stats, Some(expr))
            },
            [statements(stats)] => {
                (stats, None)
            },
            [expression(expr)] => {
                (vec![], Some(expr))
            }
        );
        let stats = stats.into_iter().map(Box::new).collect();
        let expr = expr.map(Box::new);
        Ok(Expr::Block(stats, expr))
    }

    fn statements(input: Node) -> Result<Vec<Statement>> {
        match_nodes!(input.into_children();
            [statement(stats)..] => {
                Ok(stats.collect())
            },
        )
    }

    fn statement(input: Node) -> Result<Statement> {
        match_nodes!(input.into_children();
            [let_statement(stats)] => {
                Ok(stats)
            },
            [return_statement(stats)] => {
                Ok(stats)
            },
            [expression_statement(stats)] => {
                Ok(stats)
            },
        )
    }

    fn let_statement(input: Node) -> Result<Statement> {
        let (ident, ttype, expr): (Identifier, Type, Expr) = match_nodes!(input.into_children();
            [identifier(ident), ttype(ttype), expression(expr)] => {
                (ident, ttype, expr)
            },
        );
        Ok(Statement::Declare(ident, ttype, Box::new(expr)))
    }
    fn return_statement(input: Node) -> Result<Statement> {
        let expr: Expr = match_nodes!(input.into_children();
            [expression(expr),] => {
                expr
            },
        );
        Ok(Statement::Return(Box::new(expr)))
    }
    fn expression_statement(input: Node) -> Result<Statement> {
        let expr: Expr = match_nodes!(input.into_children();
            [expression(expr),] => {
                expr
            },
        );
        Ok(Statement::Expr(Box::new(expr)))
    }
    fn expression(input: Node) -> Result<Expr> {
        Ok(match_nodes!(input.into_children();
            [sum(one),] => {
                one
            },
        ))
    }
    fn sum(input: Node) -> Result<Expr> {
        let (one, more): (Expr, Vec<(Opcode, Expr)>) = match_nodes!(input.into_children();
            [multiple(one),] => {
                (one, vec![])
            },
            [multiple(one), sum_more(more)..] => {
                (one, more.collect())
            },
        );
        let result = more.into_iter().fold(one, |acc, next| {
            Expr::BinOperation(next.0, Box::new(acc), Box::new(next.1))
        });
        Ok(result)
    }

    fn sum_more(input: Node) -> Result<(Opcode, Expr)> {
        match_nodes!(input.into_children();
            [sum_op(op), multiple(next)] => {
                Ok((op, next))
            },

        )
    }
    fn multiple(input: Node) -> Result<Expr> {
        let (one, more): (Expr, Vec<(Opcode, Expr)>) = match_nodes!(input.into_children();
            [call(one),] => {
                (one, vec![])
            },
            [call(one), multiple_more(more)..] => {
                (one, more.collect())
            },
        );
        let result = more.into_iter().fold(one, |acc, next| {
            Expr::BinOperation(next.0, Box::new(acc), Box::new(next.1))
        });
        Ok(result)
    }

    fn multiple_more(input: Node) -> Result<(Opcode, Expr)> {
        match_nodes!(input.into_children();
            [time_op(op), call(next)] => {
                Ok((op, next))
            },
        )
    }

    fn sum_op(input: Node) -> Result<Opcode> {
        Ok(match input.as_str() {
            "+" => Opcode::Add,
            "-" => Opcode::Sub,
            _ => unreachable!(),
        })
    }
    fn time_op(input: Node) -> Result<Opcode> {
        Ok(match input.as_str() {
            "*" => Opcode::Mul,
            "/" => Opcode::Div,
            _ => unreachable!(),
        })
    }

    fn call(input: Node) -> Result<Expr> {
        let (field, params): (Expr, Vec<Expr>) = match_nodes!(input.into_children();
            [field_access(field),] => {
                (field, vec![])
            },
            [field_access(field), call_parameters(params)] => {
                (field, params)
            },
        );
        if params.is_empty() {
            return Ok(field);
        }
        let params = params.into_iter().map(|it| Box::new(it)).collect();
        Ok(Expr::Call(Box::new(field), params))
    }

    fn call_parameters(input: Node) -> Result<Vec<Expr>> {
        let exprs: Vec<Expr> = match_nodes!(input.into_children();
          [expression(exprs)..] => {
                exprs.collect()
            }
        );
        Ok(exprs)
    }

    fn field_access(input: Node) -> Result<Expr> {
        let mut atoms: Vec<Expr> = match_nodes!(input.into_children();
          [atom(atoms)..] => {
                atoms.collect()
            }
        );
        let first = atoms.remove(0);
        let result = atoms.into_iter().fold(first, |acc, next| {
            Expr::Field(Box::new(acc), Box::new(next))
        });
        Ok(result)
    }
    fn atom(input: Node) -> Result<Expr> {
        match_nodes!(input.into_children();
            [identifier(ident)] => {
                return Ok(Expr::Identifier(ident));
            },
            [number(number)] => {
                return Ok(number);
            },
            [expression(expr)] => {
                return Ok(expr);
            },
            [block_expression(block)] => {
                return Ok(block);
            },
            [string(string)] => {
                return Ok(Expr::String(string));
            }
        )
    }
    fn ttype(input: Node) -> Result<Type> {
        let ttype: Type = match_nodes!(input.into_children();
            [identifier(ident)] => {
                Type::new(ident)
            },
            [] => {
                Type::void()
            },
        );

        Ok(ttype)
    }

    fn string(input:Node) ->Result<String> {
        match_nodes!(input.into_children();
            [string_inner(str)] => {
                Ok(str)
            },
        )
    }

    fn string_inner(input:Node) ->Result<String> {
        Ok(input.as_str().to_owned())
    }

    fn identifier(input: Node) -> Result<Identifier> {
        Ok(input.as_str().to_owned())
    }
    fn number(input: Node) -> Result<Expr> {
        match_nodes!(input.into_children();
            [negative_number(number)] => {
                return Ok(number);
            },
            [positive_number(number)] => {
                return Ok(Expr::Number(number));
            }
        );
    }
    fn negative_number(input: Node) -> Result<Expr> {
        let number = match_nodes!(input.into_children();
            [positive_number(number)] => {
                number
            },
        );
        Ok(Expr::Negative(Box::new(number)))
    }

    fn positive_number(input: Node) -> Result<Number> {
        let value = input.as_str().replace("_", "");
        if value.ends_with("i8") {
            return Ok(Number::Integer8(
                i8::from_str(&value.replace("i8", "")).unwrap(),
            ))
        }
        if value.ends_with("i16") {
            return Ok(Number::Integer16(
                i16::from_str(&value.replace("i8", "")).unwrap(),
            ))
        }
        if value.ends_with("i32") {
            return Ok(Number::Integer32(
                i32::from_str(&value.replace("i8", "")).unwrap(),
            ))
        }

        if value.ends_with("u8") {
            return Ok(Number::UnSignInteger8(
                u8::from_str(&value.replace("i8", "")).unwrap(),
            ))
        }
        if value.ends_with("u16") {
            return Ok(Number::UnSignInteger16(
                u16::from_str(&value.replace("i8", "")).unwrap(),
            ))
        }
        if value.ends_with("u32") {
            return Ok(Number::UnSignInteger32(
                u32::from_str(&value.replace("i8", "")).unwrap(),
            ))
        }

        return Ok(Number::Integer32(
            i32::from_str(&value.replace("i8", "")).unwrap(),
        ))
    }
}

pub fn parse_module(input_str: &str) -> Result<Module> {
    let inputs = TyphoonParser::parse(Rule::module, input_str)?;
    let input = inputs.single()?;
    TyphoonParser::module(input)
}


#[cfg(test)]
mod test {
    use crate::parser::parse_module;

    #[test]
    fn test() {
        let result = parse_module(r#"
            struct A {
                inner: i32,
            }
            struct B {
                inner: A,
            }
            fn main() -> i32 {
                let a: i32 = {
                            let b : i8 = 1i8+{1};
                            b+1-1
                        };
                return a.b.c(1,{a},);
                    {c}
            }
        "#);
        dbg!(result);
    }
    #[test]
    fn test_print() {
        let result = parse_module(r#"
            fn main() -> () {
                print("hello world");
            }
        "#);
        dbg!(result);
    }
}