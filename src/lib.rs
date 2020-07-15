#[macro_use]
extern crate log;

use lalrpop_util::lalrpop_mod;


#[macro_export]
macro_rules! c_str {
    ($s:expr) => {
        concat!($s, "\0").as_ptr() as *const i8
    };
}

lalrpop_mod!(pub parser);

pub mod llvm_wrapper;

pub mod ast;
pub mod program;
pub mod error;
