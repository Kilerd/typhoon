use crate::ast::{Function, Identifier, TyphoonContext};
use std::collections::{BTreeMap};
use std::sync::Arc;
use llvm_sys::core::{LLVMStructCreateNamed, LLVMStructSetBody};
use std::ffi::{CString};
use llvm_sys::prelude::LLVMTypeRef;
use crate::llvm_wrapper::build::Build;
use crate::llvm_wrapper::typ::Typ;

#[derive(Debug)]
pub enum ModuleItem {
    Function(Function),
    StructDefine(StructDetail),
}


impl ModuleItem {
    pub  fn codegen(&self, upper_context: Arc<TyphoonContext>) {
        match self {
            ModuleItem::Function(func) => {
                func.codegen(upper_context)
            }
            ModuleItem::StructDefine(defined_struct) => {
                debug!("struct codegen {:?}", defined_struct);

                let mut fields_llvm_types: Vec<LLVMTypeRef> = defined_struct
                    .fields
                    .iter()
                    .map(|(_field_key, field_value)| {
                        let arc = upper_context.get_type_from_name(field_value.clone()).expect("cannot found type");
                        arc.generate_type(upper_context.clone())
                    })
                    .collect();

                let struct_ty = Typ::struct_(&defined_struct.name.clone(), &mut fields_llvm_types, upper_context.llvm_context);

                upper_context.define_struct(defined_struct, struct_ty);
            }
        }
    }
}


#[derive(Debug, Clone)]
pub struct StructDetail {
    pub name: Identifier,
    pub fields: BTreeMap<Identifier, Identifier>,
}

impl StructDetail {
    pub fn new(name: String, items: Vec<(Identifier, Identifier)>) -> Self {
        Self {
            name,
            fields: items.into_iter().collect(),
        }
    }
}