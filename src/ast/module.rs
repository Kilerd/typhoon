use crate::ast::function::Function;
use llvm_sys::prelude::{LLVMContextRef, LLVMBuilderRef, LLVMModuleRef};
use llvm_sys::core::LLVMModuleCreateWithName;

// stmt
#[derive(Debug)]
pub struct Module {
    pub func: Vec<Box<Function>>,
}

impl Module {
    pub fn new(func: Vec<Box<Function>>) -> Self {
        Self {
            func
        }
    }
}

impl Module {
    pub unsafe fn codegen(&mut self, context: LLVMContextRef, builder: LLVMBuilderRef) -> LLVMModuleRef {
        let module = LLVMModuleCreateWithName(c_str!("typhoon"));
        for x in self.func.iter_mut() {
            x.codegen(module, context, builder)
        }
        return module;
    }
}