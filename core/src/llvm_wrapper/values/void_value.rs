use crate::llvm_wrapper::values::BasicValue;

pub struct VoidValue {}

impl VoidValue {

    pub fn into_basic_value(self) -> BasicValue {
        BasicValue::new(None)
    }
}
