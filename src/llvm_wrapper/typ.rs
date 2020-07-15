use llvm_sys::prelude::{LLVMContextRef, LLVMValueRef, LLVMTypeRef};
use llvm_sys::core::*;
use std::ffi::CString;

pub struct Typ;

impl Typ {
    pub fn void(context: LLVMContextRef) -> LLVMTypeRef {
        unsafe { LLVMVoidTypeInContext(context) }
    }
    pub fn bool(context: LLVMContextRef) -> LLVMTypeRef {
        unsafe { LLVMInt1TypeInContext(context) }
    }
    pub fn int8(context: LLVMContextRef) -> LLVMTypeRef {
        unsafe { LLVMInt8TypeInContext(context) }
    }
    pub fn int16(context: LLVMContextRef) -> LLVMTypeRef {
        unsafe { LLVMInt16TypeInContext(context) }
    }
    pub fn int32(context: LLVMContextRef) -> LLVMTypeRef {
        unsafe { LLVMInt32TypeInContext(context) }
    }
    pub fn char(context: LLVMContextRef) -> LLVMTypeRef {
        unsafe { Typ::int8(context) }
    }
    pub fn struct_(name: &str, fields: &mut Vec<LLVMTypeRef>, context: LLVMContextRef) -> LLVMTypeRef {
        let name = CString::new(name).unwrap();
        unsafe {
            let named_struct = LLVMStructCreateNamed(context, name.as_ptr());
            LLVMStructSetBody(named_struct, fields.as_mut_ptr(), fields.len() as u32, 0);
            named_struct
        }
    }
    pub fn ptr(typ: LLVMTypeRef) -> LLVMTypeRef {
        unsafe { LLVMPointerType(typ, 0) }
    }

    pub fn func(params: &mut Vec<LLVMTypeRef>, ret: LLVMTypeRef) -> LLVMTypeRef {
        unsafe { LLVMFunctionType(ret, params.as_mut_ptr(), params.len() as u32, 0) }
    }
}
