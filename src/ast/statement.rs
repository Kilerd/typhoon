use std::sync::Arc;

use llvm_sys::{
    core::{LLVMBuildAlloca, LLVMBuildRet, LLVMBuildStore}, LLVMValue,
};

use crate::ast::{Expr, Identifier, TypeName, TyphoonContext};

#[derive(Debug)]
pub enum Statement {
    Assign(Identifier, TypeName, Box<Expr>),
    Return(Box<Expr>),
}

impl Statement {
    pub unsafe fn codegen(&self, upper_context: Arc<TyphoonContext>) -> *mut LLVMValue {
        debug!("statement codegen: {:?}", &self);
        match self {
            Statement::Assign(identifier, _id_type, expr) => {

                // let {identifier} : {_id_type} = {expr}
                let expr_type = expr.get_type(upper_context.clone());

                let expr_value = expr.codegen(upper_context.clone());

                let assigned_type = upper_context.get_type_from_name(_id_type.clone()).expect("cannot get type");

                if !assigned_type.equals(expr_type.clone()) {
                    panic!(" expr type {} is not equals to assigned type {}", &expr_type.name, &assigned_type.name)
                }

                let assigned_llvm_type = assigned_type.generate_type(upper_context.clone());
                let alloca = LLVMBuildAlloca(upper_context.builder, assigned_llvm_type, c_str!("assign_type"));
                let store = LLVMBuildStore(upper_context.builder, expr_value, alloca);
                let x = alloca.clone();

                upper_context.new_assign(identifier.clone(), x, expr_type.type_id);
                store
            }
            Statement::Return(expr) => {
                let x1 = expr.codegen(upper_context.clone());
                LLVMBuildRet(upper_context.builder, x1)
            }
        }
    }
}
