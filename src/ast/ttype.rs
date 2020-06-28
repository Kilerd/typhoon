use crate::ast::Opcode;
use llvm_sys::prelude::{LLVMBuilderRef, LLVMContextRef, LLVMModuleRef, LLVMValueRef};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, RwLock},
};

pub type Identifier = String;
pub type TypeName = String;

pub struct Type {
    pub name: String,
    pub operands: HashMap<(Opcode, Box<Type>), Box<Type>>,
}

pub struct TyphoonContext {
    pub llvm_context: LLVMContextRef,
    pub builder: LLVMBuilderRef,
    pub module: LLVMModuleRef,
    pub upper: Option<Arc<TyphoonContext>>,
    pub variables: RwLock<HashMap<Identifier, (LLVMValueRef, Type)>>,
    pub types: HashMap<TypeName, Type>,
    pub function: Option<LLVMValueRef>,
}

impl TyphoonContext {
    pub fn new(
        llvm_context: LLVMContextRef,
        builder: LLVMBuilderRef,
        module: LLVMModuleRef,
    ) -> TyphoonContext {
        TyphoonContext {
            llvm_context,
            builder,
            module,
            upper: None,
            variables: RwLock::new(HashMap::new()),
            types: HashMap::new(),
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
}

pub trait Storable {
    fn store();

    fn load();
}
