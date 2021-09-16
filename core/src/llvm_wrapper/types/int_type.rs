use crate::llvm_wrapper::types::BasicType;
use llvm_sys::prelude::LLVMTypeRef;

pub struct IntType {
    inner_type: BasicType,
}

impl IntType {
    pub fn new(llvm_type: LLVMTypeRef) -> Self {
        IntType {
            inner_type: BasicType::new(llvm_type),
        }
    }
    
    pub fn as_basic_type(&self) -> BasicType {
        BasicType {
            ty: self.inner_type.ty
        }
    }
}
