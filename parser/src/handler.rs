use ast::{ModuleItem, StructDetail};

pub fn struct_declare_handler(name: String, params: Vec<(String, String)>) -> Box<ModuleItem> {
    Box::new(ModuleItem::StructDeclare(StructDetail::new(name, params)))
}
