use crate::ast::{
    ttype::{Identifier, Type},
    TyphoonContext,
};
use llvm_sys::{
    core::{
        LLVMAddIncoming, LLVMAppendBasicBlockInContext, LLVMBuildAShr, LLVMBuildAdd, LLVMBuildAnd,
        LLVMBuildBr, LLVMBuildCondBr, LLVMBuildICmp, LLVMBuildLoad, LLVMBuildMul, LLVMBuildOr,
        LLVMBuildPhi, LLVMBuildSDiv, LLVMBuildShl, LLVMBuildSub, LLVMBuildXor, LLVMConstInt,
        LLVMInt16TypeInContext, LLVMInt32TypeInContext, LLVMInt64TypeInContext,
        LLVMInt8TypeInContext, LLVMPositionBuilderAtEnd,
    },
    prelude::{LLVMBuilderRef, LLVMContextRef, LLVMValueRef},
    LLVMBuilder, LLVMContext, LLVMIntPredicate, LLVMValue,
};
use std::sync::{Arc};

#[derive(Debug)]
pub enum Opcode {
    Mul,
    Div,
    Add,
    Sub,
}

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

    If {
        condition: Box<Expr>,
        then_body: Box<Expr>,
        else_body: Box<Expr>,
    },
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
    pub unsafe fn codegen(&self, upper_context: Arc<TyphoonContext>) -> *mut LLVMValue {
        match self {
            Number::Integer8(n) => {
                let int_type = LLVMInt8TypeInContext(upper_context.llvm_context);
                LLVMConstInt(int_type, *n as u64, 1)
            }
            Number::Integer16(n) => {
                let int_type = LLVMInt16TypeInContext(upper_context.llvm_context);
                LLVMConstInt(int_type, *n as u64, 1)
            }
            Number::Integer32(n) => {
                let int_type = LLVMInt32TypeInContext(upper_context.llvm_context);
                LLVMConstInt(int_type, *n as u64, 1)
            }
            Number::Integer64(n) => {
                let int_type = LLVMInt64TypeInContext(upper_context.llvm_context);
                LLVMConstInt(int_type, *n as u64, 1)
            }
            Number::UnSignInteger8(n) => {
                let int_type = LLVMInt8TypeInContext(upper_context.llvm_context);
                LLVMConstInt(int_type, *n as u64, 0)
            }
            Number::UnSignInteger16(n) => {
                let int_type = LLVMInt16TypeInContext(upper_context.llvm_context);
                LLVMConstInt(int_type, *n as u64, 0)
            }
            Number::UnSignInteger32(n) => {
                let int_type = LLVMInt32TypeInContext(upper_context.llvm_context);
                LLVMConstInt(int_type, *n as u64, 0)
            }
            Number::UnSignInteger64(n) => {
                let int_type = LLVMInt8TypeInContext(upper_context.llvm_context);
                LLVMConstInt(int_type, *n as u64, 0)
            }
        }
    }
}

impl Expr {
    pub fn get_type(&self) -> Type {
        todo!()
    }

    pub unsafe fn codegen(&self, upper_context: Arc<TyphoonContext>) -> LLVMValueRef {
        println!("expr codegen");

        match self {
            Expr::Number(n) => n.codegen(upper_context.clone()),
            Expr::Identifier(identifier) => {
                let guard = upper_context.variables.read().unwrap();
                let x = guard
                    .get(identifier)
                    .expect(&format!("variable '{}' is undefined", identifier));
                let x = x.0;
                LLVMBuildLoad(upper_context.builder, x, c_str!("loadi"))
            }
            Expr::Or(lhs, rhs) => {
                let lhs_value = lhs.codegen(upper_context.clone());
                let rhs_value = rhs.codegen(upper_context.clone());
                LLVMBuildOr(upper_context.builder, lhs_value, rhs_value, c_str!("ortmp"))
            }
            Expr::Xor(lhs, rhs) => {
                let lhs_value = lhs.codegen(upper_context.clone());
                let rhs_value = rhs.codegen(upper_context.clone());
                LLVMBuildXor(
                    upper_context.builder,
                    lhs_value,
                    rhs_value,
                    c_str!("xortmp"),
                )
            }
            Expr::And(lhs, rhs) => {
                let lhs_value = lhs.codegen(upper_context.clone());
                let rhs_value = rhs.codegen(upper_context.clone());
                LLVMBuildAnd(
                    upper_context.builder,
                    lhs_value,
                    rhs_value,
                    c_str!("andtmp"),
                )
            }
            Expr::LShift(lhs, rhs) => {
                let lhs_value = lhs.codegen(upper_context.clone());
                let rhs_value = rhs.codegen(upper_context.clone());
                LLVMBuildShl(
                    upper_context.builder,
                    lhs_value,
                    rhs_value,
                    c_str!("lshifttmp"),
                )
            }
            Expr::RShift(lhs, rhs) => {
                let lhs_value = lhs.codegen(upper_context.clone());
                let rhs_value = rhs.codegen(upper_context.clone());
                LLVMBuildAShr(
                    upper_context.builder,
                    lhs_value,
                    rhs_value,
                    c_str!("rshifttmp"),
                )
            }
            Expr::Mod(lhs, rhs) => {
                let lhs_value_1 = lhs.codegen(upper_context.clone());

                let lhs_value = lhs.codegen(upper_context.clone());
                let rhs_value = rhs.codegen(upper_context.clone());

                let div = LLVMBuildSDiv(
                    upper_context.builder,
                    lhs_value,
                    rhs_value,
                    c_str!("modinnerdivtmp"),
                );
                let mul = LLVMBuildMul(
                    upper_context.builder,
                    div,
                    rhs_value,
                    c_str!("modinnermultmp"),
                );
                LLVMBuildSub(upper_context.builder, lhs_value_1, mul, c_str!("modtmp"))
            }
            Expr::Mul(lhs, rhs) => {
                let lhs_value = lhs.codegen(upper_context.clone());
                let rhs_value = rhs.codegen(upper_context.clone());
                LLVMBuildMul(
                    upper_context.builder,
                    lhs_value,
                    rhs_value,
                    c_str!("multmp"),
                )
            }
            Expr::Div(lhs, rhs) => {
                let lhs_value = lhs.codegen(upper_context.clone());
                let rhs_value = rhs.codegen(upper_context.clone());
                LLVMBuildSDiv(
                    upper_context.builder,
                    lhs_value,
                    rhs_value,
                    c_str!("udivtmp"),
                )
            }
            Expr::Add(lhs, rhs) => {
                let lhs_value = lhs.codegen(upper_context.clone());
                let rhs_value = rhs.codegen(upper_context.clone());
                LLVMBuildAdd(
                    upper_context.builder,
                    lhs_value,
                    rhs_value,
                    c_str!("addtmp"),
                )
            }
            Expr::Sub(lhs, rhs) => {
                let lhs_value = lhs.codegen(upper_context.clone());
                let rhs_value = rhs.codegen(upper_context.clone());
                LLVMBuildSub(
                    upper_context.builder,
                    lhs_value,
                    rhs_value,
                    c_str!("subtmp"),
                )
            }
            Expr::Pow(lhs, rhs) => {
                let lhs_value = lhs.codegen(upper_context.clone());
                let _rhs_value = rhs.codegen(upper_context.clone());
                lhs_value
            }
            Expr::If {
                condition,
                then_body,
                else_body,
            } => {
                let condition_value = condition.codegen(upper_context.clone());
                let int_type = LLVMInt32TypeInContext(upper_context.llvm_context);
                let zero = LLVMConstInt(int_type, 0, 0);
                let is_not_zero = LLVMBuildICmp(
                    upper_context.builder,
                    LLVMIntPredicate::LLVMIntNE,
                    condition_value,
                    zero,
                    c_str!("is_not_zero"),
                );

                let then_block = LLVMAppendBasicBlockInContext(
                    upper_context.llvm_context,
                    upper_context.function.unwrap(),
                    c_str!("entry"),
                );
                let else_block = LLVMAppendBasicBlockInContext(
                    upper_context.llvm_context,
                    upper_context.function.unwrap(),
                    c_str!("entry"),
                );
                let merge_block = LLVMAppendBasicBlockInContext(
                    upper_context.llvm_context,
                    upper_context.function.unwrap(),
                    c_str!("entry"),
                );
                LLVMBuildCondBr(upper_context.builder, is_not_zero, then_block, else_block);

                LLVMPositionBuilderAtEnd(upper_context.builder, then_block);
                let then_return = then_body.codegen(upper_context.clone());
                LLVMBuildBr(upper_context.builder, merge_block);

                LLVMPositionBuilderAtEnd(upper_context.builder, else_block);
                let else_return = else_body.codegen(upper_context.clone());
                LLVMBuildBr(upper_context.builder, merge_block);

                LLVMPositionBuilderAtEnd(upper_context.builder, merge_block);
                let phi = LLVMBuildPhi(upper_context.builder, int_type, c_str!("iftmp"));
                let mut values = vec![then_return, else_return];
                let mut blocks = vec![then_block, else_block];
                LLVMAddIncoming(phi, values.as_mut_ptr(), blocks.as_mut_ptr(), 2);
                phi
            }
        }
    }
}
