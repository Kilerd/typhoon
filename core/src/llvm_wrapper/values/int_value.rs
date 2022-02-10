use crate::llvm_wrapper::values::BasicValue;
use llvm_sys::prelude::LLVMValueRef;

#[derive(Debug)]
pub struct IntValue {
    value: LLVMValueRef,
}

impl IntValue {
    pub fn new(value: LLVMValueRef) -> Self {
        IntValue { value }
    }
    pub fn as_llvm_ref(&self) -> LLVMValueRef {
        self.value
    }

    pub fn into_basic_value(self) -> BasicValue {
        BasicValue::new(self.value)
    }
}
