use llvm_sys::prelude::LLVMBasicBlockRef;

pub struct BasicBlock {
    bb: LLVMBasicBlockRef,
}

impl BasicBlock {
    pub fn new(bb: LLVMBasicBlockRef) -> Self {
        BasicBlock { bb }
    }
}