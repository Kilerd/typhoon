#[macro_use]
extern crate combine;

use crate::lex::{atom, factor, term};
use combine::char::string;
use combine::parser::combinator::recognize;
use combine::{choice, many, optional, skip_many, ParseError, Parser, Stream};

pub mod enums;
pub mod lex;

//pub fn power<I>() -> impl Parser<Input = I, Output = String>
//where
//    I: Stream<Item = char>,
//    I::Error: ParseError<I::Item, I::Range, I::Position>,
//{
//    recognize((string("1"), optional((string("**"), string("1")))))
//}
//
//pub fn mul<I>() -> impl Parser<Input = I, Output = String>
//where
//    I: Stream<Item = char>,
//    I::Error: ParseError<I::Item, I::Range, I::Position>,
//{
//    recognize((string("1"), skip_many((string("*"), factor()))))
//}

fn main() {
    dbg!(atom().easy_parse("1"));
    dbg!(factor().easy_parse("1**2"));
    dbg!(factor().easy_parse("1"));
    dbg!(term().easy_parse("2*3"));
}
