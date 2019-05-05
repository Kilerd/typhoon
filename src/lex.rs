use combine::parser::char::{alpha_num, char, digit, letter, spaces, string};

use crate::enums::{Atom, BinOperation, BinOperator};
use combine::parser::choice::or;
use combine::parser::combinator::recognize;
use combine::parser::range::range;
use combine::{choice, many, many1, optional, skip_many, ParseError, Parser, Stream};

pub fn integer<I>() -> impl Parser<Input = I, Output = i64>
where
    I: Stream<Item = char>,
    I::Error: ParseError<I::Item, I::Range, I::Position>,
{
    optional(char('-'))
        .and(
            many1((digit(), optional(char('_'))).map(|(d, _)| d)).map(|s: String| {
                let mut n = 0;
                for c in s.chars() {
                    n = n * 10 + (c as i64 - '0' as i64);
                }
                n
            }),
        )
        .map(|(sign, n)| if sign.is_some() { -n } else { n })
}

/// TODO scientific mathematical supported
/// TODO should return int or float
///
pub fn number<I>() -> impl Parser<Input = I, Output = f64>
where
    I: Stream<Item = char>,
    I::Error: ParseError<I::Item, I::Range, I::Position>,
{
    let i = char('0').map(|_| 0.0).or(integer().map(|x| x as f64));
    let fractional = many(digit()).map(|digits: String| {
        let mut magnitude = 1.0;
        digits.chars().fold(0.0, |acc, d| {
            magnitude /= 10.0;
            match d.to_digit(10) {
                Some(d) => acc + (d as f64) * magnitude,
                None => panic!("Not a digit"),
            }
        })
    });
    i.and(optional(char('.')).with(fractional))
        .map(|(x, y)| if x > 0.0 { x + y } else { x - y })
}

pub fn identifier<I>() -> impl Parser<Input = I, Output = String>
where
    I: Stream<Item = char>,
    I::Error: ParseError<I::Item, I::Range, I::Position>,
{
    recognize((
        choice((letter(), char('_'))),
        skip_many(choice((alpha_num(), char('_')))),
    ))
}

pub fn atom<I>() -> impl Parser<Input = I, Output = Atom>
where
    I: Stream<Item = char>,
    I::Error: ParseError<I::Item, I::Range, I::Position>,
{
    choice((
        string("False").map(|_| Atom::Boolean(false)),
        string("True").map(|_| Atom::Boolean(true)),
        string("None").map(|_| Atom::None),
        number().map(Atom::Float),
        integer().map(Atom::Int),
        identifier().map(Atom::Identifier),
    ))
}

#[inline(always)]
pub fn factor<I>() -> impl Parser<Input = I, Output = BinOperation>
where
    I: Stream<Item = char>,
    I::Error: ParseError<I::Item, I::Range, I::Position>,
{
    factor_()
}

parser! {

#[inline(always)]
pub fn factor_[I]()(I) -> BinOperation
    where [ I: Stream<Item = char> ]
{
        choice((

            choice((
                char('+'),
                char('-'),
                char('~')
            ))
                .and(factor())
                .map(|(c, f)| {
                    match c {
                        '+' => BinOperation::Factor(BinOperator::Add, Box::new(f)),
                        '-' => BinOperation::Factor(BinOperator::Sub, Box::new(f)),
                        '~' => BinOperation::Factor(BinOperator::Invert, Box::new(f)),
                        _ => unreachable!()
                    }
                }),
                power()
    ))
}

}

#[inline(always)]
pub fn power<I>() -> impl Parser<Input = I, Output = BinOperation>
where
    I: Stream<Item = char>,
    I::Error: ParseError<I::Item, I::Range, I::Position>,
{
    power_()
}

parser! {

#[inline(always)]
pub fn power_[I]()(I) -> BinOperation
    where [ I: Stream<Item = char> ]
{
        (
        atom().map(BinOperation::Power),
        optional(
            (
            string("**"),
            factor()
            )
        )
        )
        .map(|(a, f)| if f.is_some() {BinOperation::Expresion(Box::new(a), BinOperator::Pow, Box::new(f.unwrap().1))} else {a})

}

}

#[cfg(test)]
mod test {
    mod integer {
        use crate::lex::{integer, number};
        use combine::parser::Parser;

        #[test]
        fn should_return_123_when_given_123_in_integer() {
            match integer().easy_parse("123") {
                Ok((n, _)) => {
                    assert_eq!(123i64, n);
                }
                Err(e) => {
                    println!("{}", e);
                    assert!(false);
                }
            }
        }

        #[test]
        fn should_return_0_when_given_0_in_integer() {
            match integer().easy_parse("0") {
                Ok((n, _)) => {
                    assert_eq!(0i64, n);
                }
                Err(e) => {
                    println!("{}", e);
                    assert!(false);
                }
            }
        }

        #[test]
        fn should_return_123_when_given_123_with_underline_in_integer() {
            match integer().easy_parse("12_3") {
                Ok((n, _)) => {
                    assert_eq!(123i64, n);
                }
                _ => {
                    assert!(false);
                }
            }
        }
    }

    mod float {
        use crate::lex::{integer, number};
        use combine::parser::Parser;

        #[test]
        fn should_return_123_dot_1_when_given_negative_123_dot_1() {
            match number().easy_parse("123.1") {
                Ok((n, _)) => {
                    assert_eq!(123.1, n);
                }
                Err(e) => {
                    println!("{}", e);
                    assert!(false);
                }
            }
        }

        #[test]
        fn should_return_negative_123_when_given_negative_123_in_integer() {
            match integer().easy_parse("-123") {
                Ok((n, _)) => {
                    assert_eq!(-123i64, n);
                }
                Err(e) => {
                    println!("{}", e);
                    assert!(false);
                }
            }
        }

        #[test]
        fn should_return_i32_when_given_123() {
            let mut parser = number();
            match parser.easy_parse("123") {
                Ok((n, _)) => {
                    assert_eq!(123f64, n);
                }
                _ => {
                    assert!(false);
                }
            }
        }
    }

    mod identifier {
        use crate::lex::identifier;
        use combine::parser::Parser;

        #[test]
        fn should_return_itself() {
            assert_eq!(Ok(("abc".into(), "")), identifier().easy_parse("abc"));
            assert_eq!(Ok(("abc123".into(), "")), identifier().easy_parse("abc123"));
        }

        #[test]
        fn should_accept_identifier_with_underline() {
            assert_eq!(Ok(("_abc".into(), "")), identifier().easy_parse("_abc"));
            assert_eq!(
                Ok(("_123abc".into(), "")),
                identifier().easy_parse("_123abc")
            );
        }

        #[test]
        fn should_not_accept_identifier_with_digit() {
            assert!(identifier().easy_parse("123").is_err());
        }
    }

    mod atom {
        use crate::enums::Atom;
        use crate::lex::atom;
        use combine::parser::Parser;

        #[test]
        fn should_return_none() {
            assert_eq!(Ok((Atom::None, "")), atom().easy_parse("None"));
        }

        #[test]
        fn should_return_boolean() {
            assert_eq!(Ok((Atom::Boolean(true), "")), atom().easy_parse("True"));
            assert_eq!(Ok((Atom::Boolean(false), "")), atom().easy_parse("False"));
        }

        #[test]
        fn should_return_float() {
            assert_eq!(Ok((Atom::Float(1.001f64), "")), atom().easy_parse("1.001"));
        }

        #[test]
        fn should_return_identifier() {
            assert_eq!(
                Ok((Atom::Identifier("hello".into()), "")),
                atom().easy_parse("hello")
            );
        }
    }

    mod pow {
        use crate::enums::{Atom, BinOperation, BinOperator};
        use crate::lex::power;
        use combine::parser::Parser;

        #[test]
        fn should_work() {
            let result = power().easy_parse("123.1**-1");
            assert!(result.is_ok());
            assert_eq!(
                BinOperation::Expresion(
                    Box::new(BinOperation::Power(Atom::Float(123.1f64))),
                    BinOperator::Pow,
                    Box::new(BinOperation::Factor(
                        BinOperator::Sub,
                        Box::new(BinOperation::Power(Atom::Float(1f64)))
                    ))
                ),
                result.unwrap().0
            );
        }

    }
}
