#[macro_export]
macro_rules! c_str {
    ($s:expr) => {
        concat!($s, "\0").as_ptr() as *const i8
    };
}

pub mod build;
pub mod build_in;
pub mod literal;
pub mod typ;
