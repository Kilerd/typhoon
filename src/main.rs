use lalrpop_util::lalrpop_mod;
use llvm_sys::core;
use std::ptr;
pub mod ast;
lalrpop_mod!(pub parser);

macro_rules! c_str {
    ($s:expr) => {
        concat!($s, "\0").as_ptr() as *const i8
    };
}

fn main() {
    dbg!(parser::ExprParser::new().parse("((22+2)*3%2)|1"));
    dbg!(parser::ExprParser::new().parse("22"));
    dbg!(parser::ExprParser::new().parse("a"));
    dbg!(parser::ExprParser::new().parse("_a+2"));
    dbg!(parser::StatementParser::new().parse("let b : i32 = a+2"));
    dbg!(parser::StatementParser::new().parse("let a :i32 = ((22+dDS)*3%c)|1"));
}
