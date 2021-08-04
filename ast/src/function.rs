use crate::{statement::Statement, Identifier, Expr, Type};

use std::collections::HashMap;
use std::sync::Arc;

// stmt
#[derive(Debug)]
pub struct FunctionDeclare {
    pub name: Identifier,
    pub args: Vec<(Identifier, Type)>,
    pub return_type: Type,
    pub stats: Box<Expr>,
    // pub context: FunctionContext,
}

// pub type FunctionContext = HashMap<Identifier, LLVMValueRef>;

impl FunctionDeclare {
    pub fn new(
        name: Identifier,
        args: Vec<(Identifier, Type)>,
        return_type: Type,
        stats: Box<Expr>,
    ) -> Self {
        Self {
            name,
            args,
            return_type,
            stats,
            // context: HashMap::new(),
        }
    }
}
//
// impl FunctionDeclare {
//     pub fn codegen(self, upper_context: Arc<TyphoonContext>) {
//         debug!("function codegen: {}", &self.name);
//
//         let return_type = upper_context
//             .get_type_from_name(self.return_type.clone())
//             .expect("cannot get type");
//         let llvm_return_type = return_type.generate_type(upper_context.clone());
//         let function_type = Typ::func(&mut vec![], llvm_return_type);
//         let function =
//             Build::add_func_to_module(upper_context.module, self.name.as_str(), function_type);
//         let block = Build::append_block(upper_context.llvm_context, function, "entry");
//         Build::position_at_end(upper_context.builder, block);
//
//         let context = Arc::new(TyphoonContext::new_with_upper(
//             upper_context.clone(),
//             function,
//         ));
//         let variable_type = self.stats.codegen(context);
//         Build::ret(variable_type.unwrap(), upper_context.builder);
//     }
// }
