use llvm_sys::core::{LLVMDisposeBuilder, LLVMPositionBuilderAtEnd};
use llvm_sys::prelude::LLVMBuilderRef;
use crate::llvm_wrapper::basic_block::BasicBlock;
use std::ops::Deref;

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

    pub fn position_at_end(&self, block: &BasicBlock) {
        unsafe {
            LLVMPositionBuilderAtEnd(self.b, block.as_llvm_ref())
        }
    }
}

impl Drop for TyphoonBuilder {
    fn drop(&mut self) {
        trace!("dispose builder");
        unsafe { LLVMDisposeBuilder(self.b) }
    }
}
