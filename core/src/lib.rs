#[macro_use]
extern crate log;


macro_rules! c_str {
    ($s:expr) => {
        concat!($s, "\0").as_ptr() as *const i8
    };
}

pub mod codegen;
pub mod context;
pub mod error;
pub mod program;

pub(crate) mod llvm_wrapper;