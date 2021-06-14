use crate::{ModuleItem, TyphoonContext};
use llvm_sys::prelude::{LLVMBuilderRef, LLVMContextRef, LLVMModuleRef};
use llvm_wrapper::build::Build;
use std::sync::Arc;

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
    pub fn codegen(self, context: LLVMContextRef, builder: LLVMBuilderRef) -> LLVMModuleRef {
        println!("module codegen");
        let module = Build::module("typhoon");
        let typhoon_context = Arc::new(TyphoonContext::new(context, builder, module));
        for item in self.items {
            item.codegen(typhoon_context.clone())
        }
        module
    }
}
