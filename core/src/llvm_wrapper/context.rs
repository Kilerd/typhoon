use crate::llvm_wrapper::basic_block::BasicBlock;
use crate::llvm_wrapper::builder::TyphoonBuilder;
use crate::llvm_wrapper::module::TyphoonModule;
use crate::llvm_wrapper::types::int_type::IntType;
use crate::llvm_wrapper::values::function_value::FunctionValue;
use llvm_sys::core::{LLVMAppendBasicBlockInContext, LLVMContextCreate, LLVMCreateBuilderInContext, LLVMInt16TypeInContext, LLVMInt8Type, LLVMInt8TypeInContext, LLVMModuleCreateWithNameInContext, LLVMContextDispose};
use llvm_sys::prelude::LLVMContextRef;
use std::ffi::CString;

pub struct TyphoonContext {
    ctx: LLVMContextRef,
}

impl TyphoonContext {
    pub(crate) fn create_builder(&self) -> TyphoonBuilder {
        let builder = unsafe { LLVMCreateBuilderInContext(self.ctx) };
        TyphoonBuilder::new(builder)
    }
}

impl TyphoonContext {}

impl TyphoonContext {
    pub fn new() -> Self {
        TyphoonContext {
            ctx: unsafe { LLVMContextCreate() },
        }
    }

    pub(crate) fn create_module(&self, name: &str) -> TyphoonModule {
        let name = CString::new(name).unwrap();
        let module = unsafe { LLVMModuleCreateWithNameInContext(name.as_ptr(), self.ctx) };
        TyphoonModule::new(module)
    }

    pub(crate) fn append_basic_block(&self, func_value: FunctionValue, name: &str) -> BasicBlock {
        let name = CString::new(name).unwrap();
        let block = unsafe {
            LLVMAppendBasicBlockInContext(self.ctx, func_value.as_llvm_value_ref(), name.as_ptr())
        };
        BasicBlock::new(block)
    }

    pub fn i8_type(&self) -> IntType {
        IntType::new(unsafe { LLVMInt8TypeInContext(self.ctx) })
    }
    pub fn i16_type(&self) -> IntType {
        IntType::new(unsafe { LLVMInt16TypeInContext(self.ctx) })
    }
}


impl Drop for TyphoonContext {
    fn drop(&mut self) {
        unsafe { LLVMContextDispose(self.ctx) }
    }
}
