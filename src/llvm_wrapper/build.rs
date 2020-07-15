use llvm_sys::prelude::{LLVMValueRef, LLVMBuilderRef, LLVMTypeRef, LLVMContextRef};
use llvm_sys::core::{LLVMBuildStore, LLVMBuildLoad, LLVMBuildRet, LLVMBuildAlloca, LLVMBuildGEP};
use std::ffi::CString;
use crate::llvm_wrapper::literal::Literal;

pub struct Build;


impl Build {
    pub fn declare_struct(name: &str, typ: LLVMTypeRef, fields: &mut Vec<LLVMValueRef>, builder: LLVMBuilderRef, context: LLVMContextRef) -> LLVMValueRef {
        let name = CString::new(name).unwrap();
        unsafe {
            let struct_al = LLVMBuildAlloca(builder, typ, name.as_ptr());

            for (idx, field_value) in fields.into_iter().enumerate() {
                let slice_idx = Literal::int32(0, context);
                let field_dix_t = Literal::int32(idx as i32, context);
                let mut vec1 = vec![slice_idx, field_dix_t];

                let gep = LLVMBuildGEP(builder, struct_al, vec1.as_mut_ptr(), vec1.len() as u32, c_str!("dsp_"));
                LLVMBuildStore(builder, *field_value, gep);
            }

            struct_al
        }
    }

    pub fn declare(name: &str, typ: LLVMTypeRef, value: LLVMValueRef, builder: LLVMBuilderRef) -> LLVMValueRef {
        let name = CString::new(name).unwrap();
        unsafe {
            let variable = LLVMBuildAlloca(builder, typ, name.as_ptr());
            LLVMBuildStore(builder, value, variable);
            variable
        }
    }

    pub fn store(variable: LLVMValueRef, value: LLVMValueRef, builder: LLVMBuilderRef) -> LLVMValueRef {
        unsafe { LLVMBuildStore(builder, value, variable) }
    }

    pub fn load(variable: LLVMValueRef, builder: LLVMBuilderRef) -> LLVMValueRef {
        unsafe { LLVMBuildLoad(builder, variable, c_str!("load_")) }
    }
    pub fn ret(variable: LLVMValueRef, builder: LLVMBuilderRef) -> LLVMValueRef {
        unsafe { LLVMBuildRet(builder, variable) }
    }
    pub fn add() -> LLVMValueRef { todo!() }
    pub fn sub() -> LLVMValueRef { todo!() }
    pub fn mul() -> LLVMValueRef { todo!() }
    pub fn div() -> LLVMValueRef { todo!() }
    pub fn gep() -> LLVMValueRef { todo!() }
}