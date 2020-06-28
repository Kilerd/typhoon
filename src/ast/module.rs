use crate::ast::{function::Function, TyphoonContext};
use llvm_sys::{
    core::LLVMModuleCreateWithName,
    prelude::{LLVMBuilderRef, LLVMContextRef, LLVMModuleRef},
};
use std::sync::{Arc};

// stmt
#[derive(Debug)]
pub struct Module {
    pub func: Vec<Box<Function>>,
}

impl Module {
    pub fn new(func: Vec<Box<Function>>) -> Self {
        Self { func }
    }
}

impl Module {
    pub unsafe fn codegen(
        &mut self,
        context: LLVMContextRef,
        builder: LLVMBuilderRef,
    ) -> LLVMModuleRef {
        println!("module codegen");
        let module = LLVMModuleCreateWithName(c_str!("typhoon"));
        let typhoon_context = Arc::new(TyphoonContext::new(context, builder, module));

        for func in self.func.iter_mut() {
            func.codegen(typhoon_context.clone())
        }
        return module;
    }
}
