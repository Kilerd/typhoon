use crate::llvm_wrapper::types::BasicType;
use crate::llvm_wrapper::values::int_value::IntValue;
use llvm_sys::core::LLVMConstInt;
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
            ty: self.inner_type.ty,
        }
    }

    pub fn const_int(&self, value: u64, sign_extend: bool) -> IntValue {
        IntValue::new(unsafe { LLVMConstInt(self.inner_type.ty, value, sign_extend as i32) })
    }
}
