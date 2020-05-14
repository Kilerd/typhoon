use llvm_sys::core::{LLVMInt32TypeInContext, LLVMConstInt, LLVMBuildOr, LLVMBuildXor, LLVMBuildAnd, LLVMBuildBinOp, LLVMBuildLShr, LLVMBuildShl, LLVMBuildAShr, LLVMBuildMul, LLVMBuildUDiv, LLVMBuildAdd, LLVMBuildSub, LLVMBuildSDiv, LLVMBuildStore, LLVMBuildLoad, LLVMBuildAlloca, LLVMBuildRet, LLVMInt8Type, LLVMInt8TypeInContext, LLVMInt16TypeInContext, LLVMInt64TypeInContext, LLVMBuildICmp, LLVMAppendBasicBlockInContext, LLVMBuildCondBr, LLVMPositionBuilderAtEnd, LLVMBuildBr, LLVMBuildPhi, LLVMAddIncoming};
use llvm_sys::prelude::{LLVMBuilderRef, LLVMContextRef, LLVMValueRef, LLVMModuleRef};
use llvm_sys::LLVMOpcode::LLVMOr;
use llvm_sys::{LLVMBuilder, LLVMContext, LLVMValue, LLVMIntPredicate};
use std::collections::HashMap;
use std::sync::Arc;
use std::ptr;
use llvm_sys::core;


// stmt
#[derive(Debug)]
pub struct Function {
    pub stats: Vec<Box<Statement>>,
    pub context: FunctionContext,
}

pub type FunctionContext = HashMap<Identifier, LLVMValueRef>;

impl Function {
    pub fn new(stats: Vec<Box<Statement>>) -> Self {
        Self {
            stats,
            context: HashMap::new(),
        }
    }
}


#[derive(Debug)]
pub enum Statement {
    Assign(Identifier, Type, Box<Expr>),
    Return(Box<Expr>),
}

#[derive(Debug)]
pub enum Number {
    Integer8(i8),
    Integer16(i16),
    Integer32(i32),
    Integer64(i64),
    UnSignInteger8(u8),
    UnSignInteger16(u16),
    UnSignInteger32(u32),
    UnSignInteger64(u64),
}

impl Number {
    pub unsafe fn codegen(&self, context: *mut LLVMContext, builder: *mut LLVMBuilder, func_context: &mut FunctionContext) -> *mut LLVMValue {
        match self {
            Number::Integer8(n) => {
                let int_type = LLVMInt8TypeInContext(context);
                LLVMConstInt(int_type, *n as u64, 1)
            }
            Number::Integer16(n) => {
                let int_type = LLVMInt16TypeInContext(context);
                LLVMConstInt(int_type, *n as u64, 1)
            }
            Number::Integer32(n) => {
                let int_type = LLVMInt32TypeInContext(context);
                LLVMConstInt(int_type, *n as u64, 1)
            }
            Number::Integer64(n) => {
                let int_type = LLVMInt64TypeInContext(context);
                LLVMConstInt(int_type, *n as u64, 1)
            }
            Number::UnSignInteger8(n) => {
                let int_type = LLVMInt8TypeInContext(context);
                LLVMConstInt(int_type, *n as u64, 0)
            }
            Number::UnSignInteger16(n) => {
                let int_type = LLVMInt16TypeInContext(context);
                LLVMConstInt(int_type, *n as u64, 0)
            }
            Number::UnSignInteger32(n) => {
                let int_type = LLVMInt32TypeInContext(context);
                LLVMConstInt(int_type, *n as u64, 0)
            }
            Number::UnSignInteger64(n) => {
                let int_type = LLVMInt8TypeInContext(context);
                LLVMConstInt(int_type, *n as u64, 0)
            }
        }

    }
}


pub type Identifier = String;
pub type Type = String;

// mathematical
#[derive(Debug)]
pub enum Expr {
    Identifier(Identifier),
    Number(Number),
    Or(Box<Expr>, Box<Expr>),
    Xor(Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    LShift(Box<Expr>, Box<Expr>),
    RShift(Box<Expr>, Box<Expr>),
    Mod(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Div(Box<Expr>, Box<Expr>),
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Pow(Box<Expr>, Box<Expr>),

    If{condition:Box<Expr>, then_body: Box<Expr>, else_body:Box<Expr>},
}


impl Function {
    pub unsafe fn codegen(&mut self, module:LLVMModuleRef  ,context: LLVMContextRef, builder: LLVMBuilderRef)  {
        let int_type = core::LLVMInt32TypeInContext(context);
        let function_type = core::LLVMFunctionType(int_type, ptr::null_mut(), 0, 0);
        let function = core::LLVMAddFunction(module, c_str!("main"), function_type);
        let bb = core::LLVMAppendBasicBlockInContext(context, function, c_str!("entry"));
        core::LLVMPositionBuilderAtEnd(builder, bb);
        for x in &self.stats {
            let x1 = x.codegen(context, builder, &mut self.context, function);
        }
    }
}

impl Statement {
    pub unsafe fn codegen(&self, context: *mut LLVMContext, builder: *mut LLVMBuilder, func_context: &mut FunctionContext, function: LLVMValueRef) -> *mut LLVMValue {
        match self {
            Statement::Assign(identifier, id_type, expr) => {
                let expr_value = expr.codegen(context, builder, func_context, function);
                let ttype = LLVMInt32TypeInContext(context);
                let alloca = LLVMBuildAlloca(builder, ttype, c_str!("assign_type"));
                let store = LLVMBuildStore(builder, expr_value, alloca);
                let x = alloca.clone();
                func_context.insert(identifier.clone(), x);
                store
            }
            Statement::Return(expr) => {
                let x1 = expr.codegen(context, builder, func_context, function);
                LLVMBuildRet(builder, x1)
                // unimplemented!()
            }
        }
    }
}


impl Expr {
    pub unsafe fn codegen(&self, context: LLVMContextRef, builder: LLVMBuilderRef, func_context: &mut FunctionContext, function:LLVMValueRef) -> LLVMValueRef {
        match self {
            Expr::Number(n) => {
                n.codegen(context, builder,func_context)
            }
            Expr::Identifier(identifier) => {
                let x = func_context.get(identifier).expect(&format!("variable '{}' is undefined", identifier));
                LLVMBuildLoad(builder, *x, c_str!("loadi"))
            }
            Expr::Or(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder, func_context, function);
                let rhs_value = rhs.codegen(context, builder, func_context, function);
                LLVMBuildOr(builder, lhs_value, rhs_value, c_str!("ortmp"))
            }
            Expr::Xor(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder, func_context, function);
                let rhs_value = rhs.codegen(context, builder, func_context, function);
                LLVMBuildXor(builder, lhs_value, rhs_value, c_str!("xortmp"))
            }
            Expr::And(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder, func_context, function);
                let rhs_value = rhs.codegen(context, builder, func_context, function);
                LLVMBuildAnd(builder, lhs_value, rhs_value, c_str!("andtmp"))
            }
            Expr::LShift(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder, func_context, function);
                let rhs_value = rhs.codegen(context, builder, func_context, function);
                LLVMBuildShl(builder, lhs_value, rhs_value, c_str!("lshifttmp"))
            }
            Expr::RShift(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder, func_context, function);
                let rhs_value = rhs.codegen(context, builder, func_context, function);
                LLVMBuildAShr(builder, lhs_value, rhs_value, c_str!("rshifttmp"))
            }
            Expr::Mod(lhs, rhs) => {
                let lhs_value_1 = lhs.codegen(context, builder, func_context, function);

                let lhs_value = lhs.codegen(context, builder, func_context, function);
                let rhs_value = rhs.codegen(context, builder, func_context, function);

                let div = LLVMBuildSDiv(builder, lhs_value, rhs_value, c_str!("modinnerdivtmp"));
                let mul = LLVMBuildMul(builder, div, rhs_value, c_str!("modinnermultmp"));
                LLVMBuildSub(builder, lhs_value_1, mul, c_str!("modtmp"))
            }
            Expr::Mul(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder, func_context, function);
                let rhs_value = rhs.codegen(context, builder, func_context, function);
                LLVMBuildMul(builder, lhs_value, rhs_value, c_str!("multmp"))
            }
            Expr::Div(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder, func_context, function);
                let rhs_value = rhs.codegen(context, builder, func_context, function);
                LLVMBuildSDiv(builder, lhs_value, rhs_value, c_str!("udivtmp"))
            }
            Expr::Add(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder, func_context, function);
                let rhs_value = rhs.codegen(context, builder, func_context, function);
                LLVMBuildAdd(builder, lhs_value, rhs_value, c_str!("addtmp"))
            }
            Expr::Sub(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder, func_context, function);
                let rhs_value = rhs.codegen(context, builder, func_context, function);
                LLVMBuildSub(builder, lhs_value, rhs_value, c_str!("subtmp"))
            }
            Expr::Pow(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder, func_context, function);
                let rhs_value = rhs.codegen(context, builder, func_context, function);
                lhs_value
            }
            Expr::If { condition, then_body, else_body } => {
                let condition_value = condition.codegen(context, builder, func_context, function);
                let int_type = LLVMInt32TypeInContext(context);
                let zero = LLVMConstInt(int_type, 0, 0);
                let is_not_zero = LLVMBuildICmp(builder, LLVMIntPredicate::LLVMIntNE, condition_value, zero, c_str!("is_not_zero"));

                let then_block = LLVMAppendBasicBlockInContext(context, function, c_str!("entry"));
                let else_block = LLVMAppendBasicBlockInContext(context, function, c_str!("entry"));
                let merge_block = LLVMAppendBasicBlockInContext(context, function, c_str!("entry"));
                LLVMBuildCondBr(builder, is_not_zero, then_block, else_block);

                LLVMPositionBuilderAtEnd(builder, then_block);
                let then_return = then_body.codegen(context, builder, func_context, function);
                LLVMBuildBr(builder, merge_block);


                LLVMPositionBuilderAtEnd(builder, else_block);
                let else_return = else_body.codegen(context, builder, func_context, function);
                LLVMBuildBr(builder, merge_block);

                LLVMPositionBuilderAtEnd(builder, merge_block);
                let phi = LLVMBuildPhi(builder, int_type, c_str!("iftmp"));
                let mut values = vec![then_return, else_return];
                let mut blocks = vec![then_block, else_block];
                LLVMAddIncoming(phi, values.as_mut_ptr(), blocks.as_mut_ptr(), 2);
                phi

            }
        }
    }
}

#[derive(Debug)]
pub enum Opcode {
    Mul,
    Div,
    Add,
    Sub,
}
