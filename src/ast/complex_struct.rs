use crate::ast::{Function, Identifier, TyphoonContext};
use std::collections::{BTreeMap};
use std::sync::Arc;
use llvm_sys::core::{LLVMStructCreateNamed, LLVMStructSetBody};
use std::ffi::{CString};
use llvm_sys::prelude::LLVMTypeRef;

#[derive(Debug)]
pub enum ModuleItem {
    Function(Function),
    StructDefine(StructDetail),
}


impl ModuleItem {
    pub unsafe fn codegen(&self, upper_context: Arc<TyphoonContext>) {
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
                let name = CString::new(defined_struct.name.as_str()).unwrap();
                let named_struct = LLVMStructCreateNamed(upper_context.llvm_context, name.as_ptr());
                LLVMStructSetBody(named_struct, fields_llvm_types.as_mut_ptr(), fields_llvm_types.len() as u32, 0);

                upper_context.define_struct(defined_struct, named_struct);
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