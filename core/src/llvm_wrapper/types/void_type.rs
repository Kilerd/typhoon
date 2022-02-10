use llvm_sys::prelude::LLVMTypeRef;
use crate::llvm_wrapper::types::BasicType;
use crate::llvm_wrapper::values::void_value::VoidValue;

pub struct VoidType {
    ty: LLVMTypeRef
}

impl VoidType {
    pub fn new(ty: LLVMTypeRef) -> Self {
        VoidType { ty }
    }

    pub fn as_basic_type(&self) -> BasicType {
        BasicType {
            ty: self.ty,
        }
    }
    pub fn const_value(&self) -> VoidValue {
        VoidValue{}
}
}