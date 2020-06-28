use crate::ast::{statement::Statement, Identifier, TyphoonContext, Type};
use llvm_sys::{
    core,
    prelude::{LLVMValueRef},
};
use std::{
    collections::HashMap,
    ffi::CString,
    ptr,
    sync::{Arc},
};
use llvm_sys::prelude::LLVMTypeRef;

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
    pub unsafe fn codegen(&self, upper_context: Arc<TyphoonContext>) {
        println!("function codegen");

        let return_type = upper_context.get_type_from_name(self.return_type.clone());
        let llvm_return_type = return_type.generate_type(upper_context.clone());

        let function_type = core::LLVMFunctionType(llvm_return_type, ptr::null_mut(), 0, 0);
        let function_name = CString::new(self.name.as_str()).unwrap();
        let function =
            core::LLVMAddFunction(upper_context.module, function_name.as_ptr(), function_type);

        let context = Arc::new(TyphoonContext::new_with_upper(
            upper_context.clone(),
            function,
        ));

        let bb = core::LLVMAppendBasicBlockInContext(
            upper_context.llvm_context,
            function,
            c_str!("entry"),
        );
        core::LLVMPositionBuilderAtEnd(upper_context.builder, bb);

        for x in &self.stats {
            match x.as_ref() {
                Statement::Return(expr) => {
                    let x1 = expr.get_type(upper_context.clone());
                    if !x1.equals(return_type.clone()) {
                        panic!(format!("return stats type {} is not adjusted to function return type {}", x1.name, return_type.name));
                    }
                    expr.codegen(context.clone());
                }
                _ => {
                    let _x1 = x.codegen(context.clone());
                }
            }
        }
    }
}
