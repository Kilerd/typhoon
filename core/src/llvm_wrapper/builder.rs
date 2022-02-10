use crate::llvm_wrapper::basic_block::BasicBlock;
use crate::llvm_wrapper::values::BasicValue;
use llvm_sys::core::{
    LLVMBuildRet, LLVMBuildRetVoid, LLVMDisposeBuilder, LLVMPositionBuilderAtEnd,
};
use llvm_sys::prelude::LLVMBuilderRef;
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
        unsafe { LLVMPositionBuilderAtEnd(self.b, block.as_llvm_ref()) }
    }

    pub fn build_return(&self, value: impl Into<Option<BasicValue>>) {
        match dbg!(value.into().and_then(|v| v.as_llvm_ref())) {
            None => unsafe { LLVMBuildRetVoid(self.b) },
            Some(lvr) => unsafe { LLVMBuildRet(self.b, lvr) },
        };
    }
}

impl Drop for TyphoonBuilder {
    fn drop(&mut self) {
        trace!("dispose builder");
        unsafe { LLVMDisposeBuilder(self.b) }
    }
}
