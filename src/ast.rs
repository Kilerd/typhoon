


mod expresion;
mod function;
mod module;
mod statement;

mod ttype;

pub use expresion::{Expr, Number, Opcode};
pub use function::Function;
pub use module::Module;
pub use statement::Statement;
pub use ttype::*;
