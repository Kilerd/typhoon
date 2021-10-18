use llvm_sys::prelude::LLVMBasicBlockRef;
use std::ops::Deref;

pub struct BasicBlock {
    bb: LLVMBasicBlockRef,
}

impl BasicBlock {
    pub fn new(bb: LLVMBasicBlockRef) -> Self {
        BasicBlock { bb }
    }

    pub fn as_llvm_ref(&self) -> LLVMBasicBlockRef {
        self.bb
    }
}

impl Deref for BasicBlock {
    type Target = LLVMBasicBlockRef;

    fn deref(&self) -> &Self::Target {
        &self.bb
    }
}