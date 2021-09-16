use crate::llvm_wrapper::types::function_type::FunctionType;
use crate::llvm_wrapper::values::function_value::FunctionValue;
use llvm_sys::core::{LLVMAddFunction, LLVMDisposeModule};
use llvm_sys::prelude::LLVMModuleRef;
use std::ffi::CString;

pub struct TyphoonModule {
    module: LLVMModuleRef,
}

impl TyphoonModule {
    pub fn new(module: LLVMModuleRef) -> Self {
        TyphoonModule { module }
    }
    pub fn add_function(&self, name: &str, func: FunctionType) -> FunctionValue {
        let name = CString::new(name).unwrap();
        let llvm_value_ref =
            unsafe { LLVMAddFunction(self.module, name.as_ptr(), func.as_llvm_type_ref()) };
        FunctionValue::new(llvm_value_ref)
    }

    pub fn to_llvm_module_ref(&self) -> LLVMModuleRef {
        self.module
    }
}

impl Drop for TyphoonModule {
    fn drop(&mut self) {
        unsafe {
            LLVMDisposeModule(self.module);
        }
    }
}
