use crate::ast::expresion::Identifier;
use crate::ast::statement::Statement;
use std::collections::HashMap;
use llvm_sys::prelude::{LLVMValueRef, LLVMModuleRef, LLVMContextRef, LLVMBuilderRef};
use llvm_sys::core;
use std::ptr;
use std::ffi::CString;

// stmt
#[derive(Debug)]
pub struct Function {
    pub name: Identifier,
    pub return_type: Identifier,
    pub stats: Vec<Box<Statement>>,
    pub context: FunctionContext,
}

pub type FunctionContext = HashMap<Identifier, LLVMValueRef>;

impl Function {
    pub fn new(name: Identifier, return_type: Identifier, stats: Vec<Box<Statement>>) -> Self {
        Self {
            name,
            return_type,
            stats,
            context: HashMap::new(),
        }
    }
}


impl Function {
    pub unsafe fn codegen(&mut self, module: LLVMModuleRef, context: LLVMContextRef, builder: LLVMBuilderRef) {
        let int_type = core::LLVMInt32TypeInContext(context);
        let function_type = core::LLVMFunctionType(int_type, ptr::null_mut(), 0, 0);
        let function_name = CString::new(self.name.as_str()).unwrap();
        let function = core::LLVMAddFunction(module, function_name.as_ptr(), function_type);
        let bb = core::LLVMAppendBasicBlockInContext(context, function, c_str!("entry"));
        core::LLVMPositionBuilderAtEnd(builder, bb);
        for x in &self.stats {
            let x1 = x.codegen(context, builder, &mut self.context, function);
        }
    }
}