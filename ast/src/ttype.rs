use uuid::Uuid;

// use crate::{Opcode, StructDetail};
// use llvm_sys::prelude::{LLVMBuilderRef, LLVMContextRef, LLVMModuleRef, LLVMTypeRef, LLVMValueRef};
// use llvm_wrapper::typ::{Typ};
// use std::{
//     collections::HashMap,
//     sync::{Arc, RwLock},
// };
// use uuid::Uuid;
//
pub type Identifier = String;
//
// pub type TypeName = String;
pub type TypeId = Uuid;
//
#[derive(Debug)]
pub struct Type {
    pub name: Identifier,
    pub type_id: TypeId,
}
//
impl Type {
    pub fn new(name: Identifier) -> Self {
        Self {
            name,
            type_id: Uuid::new_v4(),
        }
    }
}
//
//     pub fn new_struct(struct_detail: &StructDetail, llvm_type: LLVMTypeRef) -> Self {
//         Self {
//             name: struct_detail.name.clone(),
//             type_id: Uuid::new_v4(),
//             operands: Default::default(),
//             llvm_type_ref: Some((struct_detail.clone(), llvm_type)),
//         }
//     }
//
//     pub fn generate_type(&self, context: Arc<TyphoonContext>) -> LLVMTypeRef {
//         // todo support all primitive type
//         if self.name.eq("i8") {
//             Typ::int8(context.llvm_context)
//         } else if self.name.eq("i32") {
//             Typ::int32(context.llvm_context)
//         } else if let Some((_, b)) = self.llvm_type_ref.as_ref() {
//             b.clone()
//         } else {
//             Typ::int32(context.llvm_context)
//         }
//     }
//
//     pub fn is_primitive(&self) -> bool {
//         self.llvm_type_ref.is_none()
//     }
//
//     pub fn get_field_type(
//         &self,
//         context: Arc<TyphoonContext>,
//         field: &Identifier,
//     ) -> Option<Arc<Type>> {
//         self.llvm_type_ref
//             .as_ref()
//             .map(|t_ref| &t_ref.0)
//             .and_then(|struct_detail| {
//                 struct_detail
//                     .fields
//                     .iter()
//                     .filter(|(k, _v)| k.eq(&field))
//                     .map(|(_k, v)| v)
//                     .next()
//             })
//             .and_then(|v| context.get_type_from_name(v.clone()))
//     }
//
//     pub fn get_type_field_idx(&self, ident: &Identifier) -> Option<u32> {
//         self.llvm_type_ref
//             .as_ref()
//             .map(|t_ref| &t_ref.0)
//             .and_then(|struct_detail| {
//                 struct_detail
//                     .fields
//                     .iter()
//                     .enumerate()
//                     .filter(|(_idx, (k, _v))| k.eq(&ident))
//                     .map(|(idx, _)| idx as u32)
//                     .next()
//             })
//     }
//
//     pub fn get_operand_type(&self, opcode: Opcode, rhs: Arc<Type>) -> Option<TypeId> {
//         let key = (opcode, rhs.type_id);
//         self.operands.get(&key).map(|v| v.clone())
//     }
//
//     pub fn can_be_operand(&self, opcode: Opcode, rhs: Arc<Type>) -> bool {
//         let key = (opcode, rhs.type_id);
//         self.operands.contains_key(&key)
//     }
//
//     pub fn equals(&self, other: Arc<Type>) -> bool {
//         self.name.eq(&other.name)
//     }
// }
//
// #[derive(Debug)]
// pub struct TyphoonContext {
//     pub llvm_context: LLVMContextRef,
//     pub builder: LLVMBuilderRef,
//     pub module: LLVMModuleRef,
//     pub upper: Option<Arc<TyphoonContext>>,
//     pub variables: RwLock<HashMap<Identifier, (LLVMValueRef, TypeId)>>,
//     pub types: RwLock<HashMap<TypeName, Arc<Type>>>,
//     pub types_id: RwLock<HashMap<TypeId, Arc<Type>>>,
//     pub function: Option<LLVMValueRef>,
// }
//
// impl TyphoonContext {
//     pub fn new(
//         llvm_context: LLVMContextRef,
//         builder: LLVMBuilderRef,
//         module: LLVMModuleRef,
//     ) -> TyphoonContext {
//         // todo
//         let mut i8_type = Type::new("i8".to_string());
//         let mut i32_type = Type::new("i32".to_string());
//
//         i8_type
//             .operands
//             .insert((Opcode::Add, i8_type.type_id), i8_type.type_id);
//         i8_type
//             .operands
//             .insert((Opcode::Mul, i8_type.type_id), i8_type.type_id);
//         i8_type
//             .operands
//             .insert((Opcode::Div, i8_type.type_id), i8_type.type_id);
//         i8_type
//             .operands
//             .insert((Opcode::Sub, i8_type.type_id), i8_type.type_id);
//         i32_type
//             .operands
//             .insert((Opcode::Add, i32_type.type_id), i32_type.type_id);
//         i32_type
//             .operands
//             .insert((Opcode::Mul, i32_type.type_id), i32_type.type_id);
//         i32_type
//             .operands
//             .insert((Opcode::Div, i32_type.type_id), i32_type.type_id);
//         i32_type
//             .operands
//             .insert((Opcode::Sub, i32_type.type_id), i32_type.type_id);
//
//         let arc = Arc::new(i8_type);
//         let arc1 = Arc::new(i32_type);
//
//         let mut type_map = HashMap::new();
//         type_map.insert("i8".to_string(), arc.clone());
//         type_map.insert("i32".to_string(), arc1.clone());
//
//         let mut type_id_map = HashMap::new();
//         type_id_map.insert(arc.clone().type_id, arc.clone());
//         type_id_map.insert(arc1.clone().type_id, arc1.clone());
//
//         TyphoonContext {
//             llvm_context,
//             builder,
//             module,
//             upper: None,
//             variables: RwLock::new(HashMap::new()),
//             types: RwLock::new(type_map),
//             types_id: RwLock::new(type_id_map),
//             function: None,
//         }
//     }
//
//     pub fn new_with_upper(upper: Arc<TyphoonContext>, function: LLVMValueRef) -> TyphoonContext {
//         TyphoonContext {
//             llvm_context: upper.llvm_context,
//             builder: upper.builder,
//             module: upper.module,
//             upper: Some(upper.clone()),
//             variables: RwLock::new(HashMap::new()),
//             types: RwLock::new(HashMap::new()),
//             types_id: RwLock::new(HashMap::default()),
//             function: Some(function),
//         }
//     }
//
//     pub fn new_assign(&self, name: Identifier, value: LLVMValueRef, type_id: TypeId) {
//         debug!("add assign to variable table {} {}", &name, &type_id);
//         let mut guard = self.variables.write().unwrap();
//         guard.insert(name, (value, type_id));
//     }
//
//     pub fn define_struct(&self, struct_detail: &StructDetail, llvm_type: LLVMTypeRef) {
//         // todo define struct duplicated check
//         let named_struct = Arc::new(Type::new_struct(struct_detail, llvm_type));
//         {
//             let mut guard = self.types.write().unwrap();
//             guard.insert(struct_detail.name.clone(), named_struct.clone());
//         }
//
//         {
//             let mut guard = self.types_id.write().unwrap();
//             guard.insert(named_struct.type_id, named_struct.clone());
//         }
//     }
//
//     pub fn get_variable_type(&self, name: Identifier) -> Option<Arc<Type>> {
//         debug!("get variable type {}", &name);
//         let guard = self.variables.read().unwrap();
//         guard
//             .get(&name)
//             .map(|d| d.1)
//             .and_then(|t| self.get_type_from_id(t))
//             .or_else(|| self.upper.as_ref().and_then(|f| f.get_variable_type(name)))
//     }
//
//     #[inline]
//     pub fn get_type_from_name(&self, name: Identifier) -> Option<Arc<Type>> {
//         debug!("get type from name {}", &name);
//         self.types
//             .read()
//             .expect("cannot get lock")
//             .get(&name)
//             .map(|d| d.clone())
//             .or_else(|| self.upper.as_ref().and_then(|f| f.get_type_from_name(name)))
//     }
//
//     pub fn get_type_from_id(&self, type_id: TypeId) -> Option<Arc<Type>> {
//         debug!("get type from id {}", &type_id);
//         self.types_id
//             .read()
//             .expect("cannot get lock")
//             .get(&type_id)
//             .map(|d| d.clone())
//             .or_else(|| {
//                 self.upper
//                     .as_ref()
//                     .and_then(|f| f.get_type_from_id(type_id))
//             })
//     }
// }
//
// pub trait Storable {
//     fn store();
//
//     fn load();
// }
