use crate::ast::{Expr, Identifier, Type, TypeName, TyphoonContext};
use llvm_sys::{
    core::{LLVMBuildAlloca, LLVMBuildRet, LLVMBuildStore, LLVMInt32TypeInContext}, LLVMValue,
};
use std::sync::{Arc};

#[derive(Debug)]
pub enum Statement {
    Assign(Identifier, TypeName, Box<Expr>),
    Return(Box<Expr>),
}

impl Statement {
    pub unsafe fn codegen(&self, upper_context: Arc<TyphoonContext>) -> *mut LLVMValue {
        println!("statement codegen");
        match self {
            Statement::Assign(identifier, _id_type, expr) => {
                let expr_value = expr.codegen(upper_context.clone());
                let ttype = LLVMInt32TypeInContext(upper_context.llvm_context);
                let alloca = LLVMBuildAlloca(upper_context.builder, ttype, c_str!("assign_type"));
                let store = LLVMBuildStore(upper_context.builder, expr_value, alloca);
                let x = alloca.clone();

                let mut guard = upper_context.variables.write().unwrap();
                guard.insert(
                    identifier.clone(),
                    (
                        x,
                        Type {
                            name: "".to_string(),
                            operands: Default::default(),
                        },
                    ),
                );
                store
            }
            Statement::Return(expr) => {
                let x1 = expr.codegen(upper_context.clone());
                LLVMBuildRet(upper_context.builder, x1)
                // unimplemented!()
            }
        }
    }
}
