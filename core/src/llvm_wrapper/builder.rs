use llvm_sys::core::LLVMDisposeBuilder;
use llvm_sys::prelude::LLVMBuilderRef;

pub struct TyphoonBuilder {
    b: LLVMBuilderRef,
}

impl TyphoonBuilder {
    pub fn new(b: LLVMBuilderRef) -> Self {
        TyphoonBuilder { b }
    }

    pub fn as_llvm_ref(&self) -> LLVMBuilderRef {
        self.b
    }
}

impl Drop for TyphoonBuilder {
    fn drop(&mut self) {
        unsafe { LLVMDisposeBuilder(self.b) }
    }
}
