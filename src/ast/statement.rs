use std::sync::Arc;

use llvm_sys::{
    core::{LLVMBuildAlloca, LLVMBuildRet, LLVMBuildStore}, LLVMValue,
};

use crate::ast::{Expr, Identifier, TypeName, TyphoonContext};
use crate::llvm_wrapper::build::Build;

#[derive(Debug)]
pub enum Statement {
    Assign(Identifier, TypeName, Box<Expr>),
    Return(Box<Expr>),
}

impl Statement {
    pub fn codegen(&self, upper_context: Arc<TyphoonContext>) -> *mut LLVMValue {
        debug!("statement codegen: {:?}", &self);
        match self {
            Statement::Assign(identifier, _id_type, init) => {

                // let {identifier} : {_id_type} = {expr}
                let expr_type = init.get_type(upper_context.clone());

                let expr_value = init.codegen(upper_context.clone());

                let assigned_type = upper_context.get_type_from_name(_id_type.clone()).expect("cannot get type");


                if !assigned_type.equals(expr_type.clone()) {
                    panic!(" expr type {} is not equals to assigned type {}", &expr_type.name, &assigned_type.name)
                }

                let assigned_llvm_type = assigned_type.generate_type(upper_context.clone());

                let a = if assigned_type.is_primitive() {
                    Build::declare(identifier, assigned_llvm_type, expr_value, upper_context.builder)
                } else {
                    expr_value
                };

                upper_context.new_assign(identifier.clone(), a, expr_type.type_id);
                a
            }
            Statement::Return(expr) => {
                let x1 = expr.codegen(upper_context.clone());
                Build::ret(x1, upper_context.builder)
            }
        }
    }
}
