use llvm_sys::core::{LLVMInt32TypeInContext, LLVMConstInt, LLVMBuildOr, LLVMBuildXor, LLVMBuildAnd, LLVMBuildBinOp, LLVMBuildLShr, LLVMBuildShl, LLVMBuildAShr, LLVMBuildMul, LLVMBuildUDiv, LLVMBuildAdd, LLVMBuildSub, LLVMBuildSDiv};
use llvm_sys::prelude::{LLVMBuilderRef, LLVMContextRef, LLVMValueRef};
use llvm_sys::LLVMOpcode::LLVMOr;

#[derive(Debug)]

// stmt

pub enum Statement {
    Assign(Identifier, Type, Box<Expr>),
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

impl Expr {
    pub unsafe fn codegen(&self, context: LLVMContextRef, builder: LLVMBuilderRef) -> LLVMValueRef {
        match self {
            Expr::Number(n) => {
                let int_type = LLVMInt32TypeInContext(context);
                let int = LLVMConstInt(int_type, *n as u64, 0);
                int
            },
            Expr::Identifier(_) => {
                unimplemented!()
            }
            Expr::Or(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder);
                let rhs_value = rhs.codegen(context, builder);
                LLVMBuildOr(builder, lhs_value, rhs_value, c_str!("ortmp"))
            }
            Expr::Xor(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder);
                let rhs_value = rhs.codegen(context, builder);
                LLVMBuildXor(builder, lhs_value, rhs_value, c_str!("xortmp"))
            }
            Expr::And(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder);
                let rhs_value = rhs.codegen(context, builder);
                LLVMBuildAnd(builder, lhs_value, rhs_value, c_str!("andtmp"))
            }
            Expr::LShift(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder);
                let rhs_value = rhs.codegen(context, builder);
                LLVMBuildShl(builder, lhs_value, rhs_value, c_str!("lshifttmp"))
            }
            Expr::RShift(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder);
                let rhs_value = rhs.codegen(context, builder);
                LLVMBuildAShr(builder, lhs_value, rhs_value, c_str!("rshifttmp"))
            }
            Expr::Mod(lhs, rhs) => {

                let lhs_value_1 = lhs.codegen(context, builder);

                let lhs_value = lhs.codegen(context, builder);
                let rhs_value = rhs.codegen(context, builder);

                let div = LLVMBuildSDiv(builder, lhs_value, rhs_value, c_str!("modinnerdivtmp"));
                let mul = LLVMBuildMul(builder, div, rhs_value, c_str!("modinnermultmp"));
                LLVMBuildSub(builder, lhs_value_1, mul, c_str!("modtmp"))
            }
            Expr::Mul(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder);
                let rhs_value = rhs.codegen(context, builder);
                LLVMBuildMul(builder, lhs_value, rhs_value, c_str!("multmp"))
            }
            Expr::Div(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder);
                let rhs_value = rhs.codegen(context, builder);
                LLVMBuildSDiv(builder, lhs_value, rhs_value, c_str!("udivtmp"))
            }
            Expr::Add(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder);
                let rhs_value = rhs.codegen(context, builder);
                LLVMBuildAdd(builder, lhs_value, rhs_value, c_str!("addtmp"))
            }
            Expr::Sub(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder);
                let rhs_value = rhs.codegen(context, builder);
                LLVMBuildSub(builder, lhs_value, rhs_value, c_str!("subtmp"))
            }
            Expr::Pow(lhs, rhs) => {
                let lhs_value = lhs.codegen(context, builder);
                let rhs_value = rhs.codegen(context, builder);
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
