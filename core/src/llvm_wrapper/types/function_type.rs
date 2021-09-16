use llvm_sys::prelude::LLVMTypeRef;

pub struct FunctionType {
    ty: LLVMTypeRef,
}

impl FunctionType {
    pub fn new(ty: LLVMTypeRef) -> Self {
        Self { ty }
    }
    pub fn as_llvm_type_ref(&self) -> LLVMTypeRef {
        self.ty
    }
}
