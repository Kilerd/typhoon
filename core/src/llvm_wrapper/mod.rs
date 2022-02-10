macro_rules! c_str {
    ($s:expr) => {
        concat!($s, "\0").as_ptr() as *const i8
    };
}
pub mod types;
pub mod values;

pub mod context;
pub mod module;
pub mod builder;

pub mod basic_block;