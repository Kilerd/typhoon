use llvm_sys::prelude::LLVMTypeRef;

pub struct VectorType {
    ty: LLVMTypeRef,
}

impl VectorType {
    pub fn new(ty: LLVMTypeRef) -> Self {
        Self { ty }
    }
}
