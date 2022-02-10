
use llvm_sys::prelude::LLVMTypeRef;

pub struct ArrayType {
    ty: LLVMTypeRef,
}

impl ArrayType {
    pub fn new(ty: LLVMTypeRef) -> Self {
        Self { ty }
    }
}
