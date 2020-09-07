use crate::typ::Typ;
use llvm_sys::{
    core::LLVMConstInt,
    prelude::{LLVMContextRef, LLVMValueRef},
};

pub struct Literal;

impl Literal {
    pub fn str() -> LLVMValueRef {
        todo!()
    }

    pub fn bool(b: bool, context: LLVMContextRef) -> LLVMValueRef {
        unsafe { LLVMConstInt(Typ::bool(context), b as u64, 0) }
    }

    pub fn int8(n: i8, context: LLVMContextRef) -> LLVMValueRef {
        unsafe { LLVMConstInt(Typ::int8(context), n as u64, 1) }
    }

    pub fn int16(n: i16, context: LLVMContextRef) -> LLVMValueRef {
        unsafe { LLVMConstInt(Typ::int16(context), n as u64, 1) }
    }

    pub fn int32(n: i32, context: LLVMContextRef) -> LLVMValueRef {
        unsafe { LLVMConstInt(Typ::int32(context), n as u64, 1) }
    }

    pub fn uint8(n: u8, context: LLVMContextRef) -> LLVMValueRef {
        unsafe { LLVMConstInt(Typ::int8(context), n as u64, 0) }
    }

    pub fn uint16(n: u16, context: LLVMContextRef) -> LLVMValueRef {
        unsafe { LLVMConstInt(Typ::int16(context), n as u64, 0) }
    }

    pub fn uint32(n: u32, context: LLVMContextRef) -> LLVMValueRef {
        unsafe { LLVMConstInt(Typ::int32(context), n as u64, 0) }
    }
}
