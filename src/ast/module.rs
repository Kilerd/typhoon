use crate::ast::{TyphoonContext, ModuleItem};
use llvm_sys::{
    core::LLVMModuleCreateWithName,
    prelude::{LLVMBuilderRef, LLVMContextRef, LLVMModuleRef},
};
use std::sync::{Arc};

// stmt
#[derive(Debug)]
pub struct Module {
    pub items: Vec<Box<ModuleItem>>,
}

impl Module {
    pub fn new(items: Vec<Box<ModuleItem>>) -> Self {
        Self { items }
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

        for item in self.items.iter_mut() {
            item.codegen(typhoon_context.clone())
        }
        return module;
    }
}
