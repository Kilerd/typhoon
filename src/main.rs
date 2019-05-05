#[macro_use]
extern crate combine;

use crate::lex::{atom, factor, power};
use combine::Parser;

pub mod enums;
pub mod lex;

fn main() {
    dbg!(atom().easy_parse("1"));
    dbg!(factor().easy_parse("+++++++1**2"));
}
