use crate::{Expr, FunctionDeclare, Identifier, Type};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug)]
pub enum ModuleItem {
    FunctionDeclare(FunctionDeclare),
    StructDeclare(StructDeclare),
}
//
// impl ModuleItem {
//     pub fn codegen(self, upper_context: Arc<TyphoonContext>) {
//         debug!("module item codegen {:?}", self);
//         match self {
//             ModuleItem::FunctionDeclare(func) => func.codegen(upper_context),
//             ModuleItem::StructDeclare(defined_struct) => {
//                 let mut fields_llvm_types: Vec<LLVMTypeRef> = defined_struct
//                     .fields
//                     .iter()
//                     .map(|(_field_key, field_value)| {
//                         let arc = upper_context
//                             .get_type_from_name(field_value.clone())
//                             .expect("cannot found type");
//                         arc.generate_type(upper_context.clone())
//                     })
//                     .collect();
//
//                 let struct_ty = Typ::struct_(
//                     &defined_struct.name.clone(),
//                     &mut fields_llvm_types,
//                     upper_context.llvm_context,
//                 );
//
//                 upper_context.define_struct(&defined_struct, struct_ty);
//             }
//         }
//     }
// }

#[derive(Debug)]
pub struct StructDeclare {
    pub name: Identifier,
    pub fields: BTreeMap<Identifier, Type>,
}

impl StructDeclare {
    pub fn new(name: String, items: Vec<(Identifier, Type)>) -> Self {
        Self {
            name,
            fields: items.into_iter().collect(),
        }
    }
}
