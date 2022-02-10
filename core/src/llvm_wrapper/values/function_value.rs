use llvm_sys::prelude::LLVMValueRef;

pub struct FunctionValue {
    v: LLVMValueRef
}

impl FunctionValue {
    pub fn new(v: LLVMValueRef) -> Self {
        FunctionValue { v }
    }

    pub fn as_llvm_value_ref(&self) -> LLVMValueRef {
        self.v
    }
}