use llvm_sys::prelude::LLVMValueRef;

pub struct IntValue{
    value: LLVMValueRef
}

impl IntValue {
    pub fn new(value: LLVMValueRef) -> Self {
        IntValue { value }
    }
    pub fn as_llvm_ref(&self) -> LLVMValueRef {
        self.value
    }
}

