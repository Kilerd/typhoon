use llvm_sys::core::{LLVMInt32TypeInContext, LLVMConstInt, LLVMBuildOr, LLVMBuildXor, LLVMBuildAnd, LLVMBuildBinOp, LLVMBuildLShr, LLVMBuildShl, LLVMBuildAShr, LLVMBuildMul, LLVMBuildUDiv, LLVMBuildAdd, LLVMBuildSub, LLVMBuildSDiv, LLVMBuildStore, LLVMBuildLoad, LLVMBuildAlloca, LLVMBuildRet, LLVMInt8Type, LLVMInt8TypeInContext, LLVMInt16TypeInContext, LLVMInt64TypeInContext, LLVMBuildICmp, LLVMAppendBasicBlockInContext, LLVMBuildCondBr, LLVMPositionBuilderAtEnd, LLVMBuildBr, LLVMBuildPhi, LLVMAddIncoming};
use llvm_sys::prelude::{LLVMBuilderRef, LLVMContextRef, LLVMValueRef, LLVMModuleRef};
use llvm_sys::LLVMOpcode::LLVMOr;
use llvm_sys::{LLVMBuilder, LLVMContext, LLVMValue, LLVMIntPredicate};
use std::collections::HashMap;
use std::sync::Arc;
use std::ptr;
use llvm_sys::core;
use std::ffi::CString;

mod module;
mod function;
mod statement;
mod expresion;

pub use module::Module;
pub use function::Function;
pub use statement::Statement;
pub use expresion::{Identifier, Type, Expr, Opcode, Number};