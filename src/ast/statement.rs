use crate::ast::expresion::{Identifier, Type, Expr};
use llvm_sys::{LLVMContext, LLVMBuilder, LLVMValue};
use crate::ast::function::FunctionContext;
use llvm_sys::prelude::LLVMValueRef;
use llvm_sys::core::{LLVMInt32TypeInContext, LLVMBuildAlloca, LLVMBuildStore, LLVMBuildRet};

#[derive(Debug)]
pub enum Statement {
    Assign(Identifier, Type, Box<Expr>),
    Return(Box<Expr>),
}


impl Statement {
    pub unsafe fn codegen(&self, context: *mut LLVMContext, builder: *mut LLVMBuilder, func_context: &mut FunctionContext, function: LLVMValueRef) -> *mut LLVMValue {
        match self {
            Statement::Assign(identifier, id_type, expr) => {
                let expr_value = expr.codegen(context, builder, func_context, function);
                let ttype = LLVMInt32TypeInContext(context);
                let alloca = LLVMBuildAlloca(builder, ttype, c_str!("assign_type"));
                let store = LLVMBuildStore(builder, expr_value, alloca);
                let x = alloca.clone();
                func_context.insert(identifier.clone(), x);
                store
            }
            Statement::Return(expr) => {
                let x1 = expr.codegen(context, builder, func_context, function);
                LLVMBuildRet(builder, x1)
                // unimplemented!()
            }
        }
    }
}