use crate::{statement::Statement, Identifier, TyphoonContext};

use llvm_sys::prelude::LLVMValueRef;
use llvm_wrapper::build::Build;
use llvm_wrapper::typ::Typ;
use std::{collections::HashMap, sync::Arc};

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
    pub fn codegen(&self, upper_context: Arc<TyphoonContext>) {
        debug!("function codegen: {}", &self.name);

        let return_type = upper_context
            .get_type_from_name(self.return_type.clone())
            .expect("cannot get type");
        let llvm_return_type = return_type.generate_type(upper_context.clone());
        let function_type = Typ::func(&mut vec![], llvm_return_type);
        let function =
            Build::add_func_to_module(upper_context.module, self.name.as_str(), function_type);
        let block = Build::append_block(upper_context.llvm_context, function, "entry");
        Build::position_at_end(upper_context.builder, block);

        let context = Arc::new(TyphoonContext::new_with_upper(
            upper_context.clone(),
            function,
        ));
        for x in &self.stats {
            match x.as_ref() {
                Statement::Return(expr) => {
                    let x1 = expr.get_type(context.clone());
                    if !x1.equals(return_type.clone()) {
                        panic!(format!(
                            "return stats type {} is not adjusted to function return type {}",
                            x1.name, return_type.name
                        ));
                    }
                    x.codegen(context.clone());
                }
                _ => {
                    let _x1 = x.codegen(context.clone());
                }
            }
        }
    }
}
