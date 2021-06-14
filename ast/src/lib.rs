#[macro_use]
extern crate log;


#[macro_export]
macro_rules! c_str {
    ($s:expr) => {
        concat!($s, "\0").as_ptr() as *const i8
    };
}

mod complex_struct;
mod expresion;
mod function;
mod module;
mod statement;
mod ttype;



pub use complex_struct::*;
pub use expresion::{Expr, Number, Opcode};
pub use function::FunctionDeclare;
pub use module::Module;
pub use statement::Statement;
pub use ttype::*;
