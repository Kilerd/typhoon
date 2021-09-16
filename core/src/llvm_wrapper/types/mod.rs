use crate::llvm_wrapper::types::array_type::ArrayType;
use crate::llvm_wrapper::types::function_type::FunctionType;
use crate::llvm_wrapper::types::pointer_type::PointerType;
use crate::llvm_wrapper::types::vector_type::VectorType;
use llvm_sys::core::{LLVMArrayType, LLVMFunctionType, LLVMPointerType, LLVMVectorType};
use llvm_sys::prelude::LLVMTypeRef;

pub mod array_type;
pub mod function_type;
pub mod pointer_type;
pub mod vector_type;
pub mod int_type;

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum AddressSpace {
    Generic = 0,
    Global  = 1,
    Shared  = 3,
    Const   = 4,
    Local   = 5,
}


pub struct BasicType {
    ty: LLVMTypeRef,
}

impl BasicType {
    pub fn new(ty: LLVMTypeRef) -> Self {
        Self { ty }
    }

    pub fn as_llvm_type_ref(&self) -> LLVMTypeRef {
        self.ty
    }

    pub fn ptr_type(self, address_space: AddressSpace) -> PointerType {
        unsafe { PointerType::new(LLVMPointerType(self.ty, address_space as u32)) }
    }

    pub fn vec_type(self, size: u32) -> VectorType {
        unsafe { VectorType::new(LLVMVectorType(self.ty, size)) }
    }

    pub fn array_type(self, size: u32) -> ArrayType {
        unsafe { ArrayType::new(LLVMArrayType(self.ty, size)) }
    }

    pub fn fn_type(self, params: &[BasicType], is_var_args: bool) -> FunctionType {
        let mut params_ref: Vec<LLVMTypeRef> =
            params.iter().map(|it| it.as_llvm_type_ref()).collect();
        unsafe {
            FunctionType::new(LLVMFunctionType(
                self.ty,
                params_ref.as_mut_ptr(),
                params_ref.len() as u32,
                is_var_args as i32,
            ))
        }
    }
}
