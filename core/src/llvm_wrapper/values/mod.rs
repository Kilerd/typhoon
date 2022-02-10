use llvm_sys::prelude::LLVMValueRef;

pub mod function_value;
pub mod int_value;
pub mod void_value;

#[derive(Debug)]
enum ValueOrVoid {
    Void,
    Value(LLVMValueRef),
}

#[derive(Debug)]
pub struct BasicValue {
    v: ValueOrVoid,
}

impl BasicValue {
    pub fn new(v: impl Into<Option<LLVMValueRef>>) -> Self {
        let v = match v.into() {
            None => ValueOrVoid::Void,
            Some(value) => ValueOrVoid::Value(value),
        };

        BasicValue { v }
    }

    pub fn as_llvm_ref(&self) -> Option<LLVMValueRef> {
        match self.v {
            ValueOrVoid::Void => None,
            ValueOrVoid::Value(lvr) => Some(lvr),
        }
    }
}
