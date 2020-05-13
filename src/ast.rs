use llvm_sys::core::{LLVMInt32TypeInContext, LLVMConstInt, LLVMBuildOr, LLVMBuildXor, LLVMBuildAnd, LLVMBuildBinOp, LLVMBuildLShr, LLVMBuildShl, LLVMBuildAShr, LLVMBuildMul, LLVMBuildUDiv, LLVMBuildAdd, LLVMBuildSub, LLVMBuildSDiv, LLVMBuildStore, LLVMBuildLoad, LLVMBuildAlloca, LLVMBuildRet};
use llvm_sys::prelude::{LLVMBuilderRef, LLVMContextRef, LLVMValueRef};
use llvm_sys::LLVMOpcode::LLVMOr;
use llvm_sys::{LLVMBuilder, LLVMContext, LLVMValue};
use std::collections::HashMap;
use std::sync::Arc;


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


pub type Identifier = String;
pub type Type = String;

// mathematical
#[derive(Debug)]
pub enum Expr {
    Identifier(Identifier),
    Number(i32),
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
}


impl Function {
    pub unsafe fn codegen(&mut self, context: LLVMContextRef, builder: LLVMBuilderRef)  {
        for x in &self.stats {
            let x1 = x.codegen(context, builder, &mut self.context);
        }
    }
}

impl Statement {
    pub unsafe fn codegen(&self, context: *mut LLVMContext, builder: *mut LLVMBuilder, func_context: &mut FunctionContext) -> *mut LLVMValue {
        match self {
            Statement::Assign(identifier, id_type, expr) => {
                let expr_value = expr.codegen(context, builder, func_context);
                let ttype = LLVMInt32TypeInContext(context);
                let alloca = LLVMBuildAlloca(builder, ttype, c_str!("assign_type"));
                let store = LLVMBuildStore(builder, expr_value, alloca);
                let x = alloca.clone();
                func_context.insert(identifier.clone(), x);
                store
            }
            Statement::Return(expr) => {
                let x1 = expr.codegen(context, builder, func_context);
                LLVMBuildRet(builder, x1)
                // unimplemented!()
            }
        }
    }
}


impl Expr {
    pub unsafe fn codegen(&self, context: LLVMContextRef, builder: LLVMBuilderRef, func_context: &mut FunctionContext) -> LLVMValueRef {
        match self {
            Expr::Number(n) => {
                let int_type = LLVMInt32TypeInContext(context);
                let int = LLVMConstInt(int_type, *n as u64, 0);
                int
            }
            Expr::Identifier(identifier) => {
                let x = func_context.get(identifier).expect(&format!("variable '{}' is undefined", identifier));
                LLVMBuildLoad(builder, *x, c_str!("loadi"))
            }
            Expr::Or(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder, func_context);
                let rhs_value = rhs.codegen(context, builder, func_context);
                LLVMBuildOr(builder, lhs_value, rhs_value, c_str!("ortmp"))
            }
            Expr::Xor(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder, func_context);
                let rhs_value = rhs.codegen(context, builder, func_context);
                LLVMBuildXor(builder, lhs_value, rhs_value, c_str!("xortmp"))
            }
            Expr::And(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder, func_context);
                let rhs_value = rhs.codegen(context, builder, func_context);
                LLVMBuildAnd(builder, lhs_value, rhs_value, c_str!("andtmp"))
            }
            Expr::LShift(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder, func_context);
                let rhs_value = rhs.codegen(context, builder, func_context);
                LLVMBuildShl(builder, lhs_value, rhs_value, c_str!("lshifttmp"))
            }
            Expr::RShift(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder, func_context);
                let rhs_value = rhs.codegen(context, builder, func_context);
                LLVMBuildAShr(builder, lhs_value, rhs_value, c_str!("rshifttmp"))
            }
            Expr::Mod(lhs, rhs) => {
                let lhs_value_1 = lhs.codegen(context, builder, func_context);

                let lhs_value = lhs.codegen(context, builder, func_context);
                let rhs_value = rhs.codegen(context, builder, func_context);

                let div = LLVMBuildSDiv(builder, lhs_value, rhs_value, c_str!("modinnerdivtmp"));
                let mul = LLVMBuildMul(builder, div, rhs_value, c_str!("modinnermultmp"));
                LLVMBuildSub(builder, lhs_value_1, mul, c_str!("modtmp"))
            }
            Expr::Mul(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder, func_context);
                let rhs_value = rhs.codegen(context, builder, func_context);
                LLVMBuildMul(builder, lhs_value, rhs_value, c_str!("multmp"))
            }
            Expr::Div(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder, func_context);
                let rhs_value = rhs.codegen(context, builder, func_context);
                LLVMBuildSDiv(builder, lhs_value, rhs_value, c_str!("udivtmp"))
            }
            Expr::Add(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder, func_context);
                let rhs_value = rhs.codegen(context, builder, func_context);
                LLVMBuildAdd(builder, lhs_value, rhs_value, c_str!("addtmp"))
            }
            Expr::Sub(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder, func_context);
                let rhs_value = rhs.codegen(context, builder, func_context);
                LLVMBuildSub(builder, lhs_value, rhs_value, c_str!("subtmp"))
            }
            Expr::Pow(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder, func_context);
                let rhs_value = rhs.codegen(context, builder, func_context);
                lhs_value
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
