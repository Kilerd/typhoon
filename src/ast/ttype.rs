use crate::ast::Opcode;
use llvm_sys::prelude::{LLVMBuilderRef, LLVMContextRef, LLVMModuleRef, LLVMValueRef, LLVMTypeRef};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, RwLock},
};

pub type Identifier = String;

pub type TypeName = String;

pub struct Type {
    pub name: Identifier,
    pub operands: HashMap<(Opcode, Arc<Type>), Arc<Type>>,
}

impl Type {
    pub unsafe fn generate_type(&self, context: Arc<TyphoonContext>) -> LLVMTypeRef {
        // todo
        llvm_sys::core::LLVMInt32TypeInContext(context.llvm_context)
    }

    pub fn get_operand_type(&self, opcode: Opcode, rhs: Arc<Type>) -> Option<Arc<Type>> {
        let key = (opcode, rhs);
        self.operands.get(&key).map(|v| v.clone())
    }

    pub fn can_be_operand(&self, opcode: Opcode, rhs: Arc<Type>) -> bool {
        let key = (opcode, rhs);
        self.operands.contains_key(&key)
    }

    pub fn equals(&self, other: Arc<Type>) -> bool {
        self.name.eq(&other.name)
    }
}

pub struct TyphoonContext {
    pub llvm_context: LLVMContextRef,
    pub builder: LLVMBuilderRef,
    pub module: LLVMModuleRef,
    pub upper: Option<Arc<TyphoonContext>>,
    pub variables: RwLock<HashMap<Identifier, (LLVMValueRef, Type)>>,
    pub types: HashMap<TypeName, Arc<Type>>,
    pub function: Option<LLVMValueRef>,
}

impl TyphoonContext {
    pub fn new(
        llvm_context: LLVMContextRef,
        builder: LLVMBuilderRef,
        module: LLVMModuleRef,
    ) -> TyphoonContext {

        // todo
        let i8_type = Arc::new(Type { name: "i8".to_string(), operands: Default::default() });
        let i32_type = Arc::new(Type { name: "i32".to_string(), operands: Default::default() });

        let mut map = HashMap::new();
        map.insert("i8".to_string(), i8_type);
        map.insert("i32".to_string(), i32_type);

        TyphoonContext {
            llvm_context,
            builder,
            module,
            upper: None,
            variables: RwLock::new(HashMap::new()),
            types: map,
            function: None,
        }
    }

    pub fn new_with_upper(upper: Arc<TyphoonContext>, function: LLVMValueRef) -> TyphoonContext {
        TyphoonContext {
            llvm_context: upper.llvm_context,
            builder: upper.builder,
            module: upper.module,
            upper: Some(upper.clone()),
            variables: RwLock::new(HashMap::new()),
            types: HashMap::new(),
            function: Some(function),
        }
    }

    #[inline]
    pub fn get_type_from_name(&self, name: Identifier) -> Arc<Type> {
        self.types.get(&name).expect("cannot find type").clone()
    }
}

pub trait Storable {
    fn store();

    fn load();
}
