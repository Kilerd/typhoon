use llvm_sys::prelude::LLVMTypeRef;

pub struct PointerType {
    ty: LLVMTypeRef,
}

impl PointerType {
    pub fn new(ty: LLVMTypeRef) -> Self {
        Self { ty }
    }
}
