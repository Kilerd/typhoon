use crate::{ttype::Identifier, Statement, Type};
use std::fmt::{Display, Formatter};
use std::sync::Arc;

#[derive(Debug, PartialOrd, PartialEq, Eq, Hash, Clone, Copy)]
pub enum Opcode {
    Add,
    Sub,
    Mul,
    Div,

    Mod,
    Pow,

    Or,
    And,
    Xor,

    LShift,
    RShift,
}

impl Display for Opcode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Opcode::Add => write!(f, "+"),
            Opcode::Sub => write!(f, "-"),
            Opcode::Mul => write!(f, "*"),
            Opcode::Div => write!(f, "/"),
            Opcode::Mod => write!(f, "%"),
            Opcode::Pow => write!(f, "**"),
            Opcode::Or => write!(f, "|"),
            Opcode::And => write!(f, "&"),
            Opcode::Xor => write!(f, "^"),
            Opcode::LShift => write!(f, "<<"),
            Opcode::RShift => write!(f, ">>"),
        }
    }
}
//
// impl Opcode {
//     pub fn calculate_codegen(
//         &self,
//         lhs: LLVMValueRef,
//         rhs: LLVMValueRef,
//         upper_context: Arc<TyphoonContext>,
//     ) -> LLVMValueRef {
//         unsafe {
//             match self {
//                 Opcode::Add => LLVMBuildAdd(upper_context.builder, lhs, rhs, c_str!("add_tmp")),
//                 Opcode::Sub => LLVMBuildSub(upper_context.builder, lhs, rhs, c_str!("sub_tmp")),
//                 Opcode::Mul => LLVMBuildMul(upper_context.builder, lhs, rhs, c_str!("mul_tmp")),
//                 Opcode::Div => LLVMBuildSDiv(upper_context.builder, lhs, rhs, c_str!("u_div_tmp")),
//                 Opcode::Mod => {
//                     let div =
//                         LLVMBuildSDiv(upper_context.builder, lhs, rhs, c_str!("mod_inner_div_tmp"));
//                     let mul =
//                         LLVMBuildMul(upper_context.builder, div, rhs, c_str!("mod_inner_mul_tmp"));
//                     LLVMBuildSub(upper_context.builder, lhs.clone(), mul, c_str!("mod_tmp"))
//                 }
//                 Opcode::Pow => todo!(),
//                 Opcode::Or => LLVMBuildOr(upper_context.builder, lhs, rhs, c_str!("ort_mp")),
//                 Opcode::And => LLVMBuildAnd(upper_context.builder, lhs, rhs, c_str!("and_tmp")),
//                 Opcode::Xor => LLVMBuildXor(upper_context.builder, lhs, rhs, c_str!("xor_tmp")),
//                 Opcode::LShift => {
//                     LLVMBuildShl(upper_context.builder, lhs, rhs, c_str!("lshift_tmp"))
//                 }
//                 Opcode::RShift => {
//                     LLVMBuildAShr(upper_context.builder, lhs, rhs, c_str!("rshift_tmp"))
//                 }
//             }
//         }
//     }
// }
// mathematical
#[derive(Debug)]
pub enum Expr {
    // StructAssign(Identifier, Vec<(Box<Expr>, Box<Expr>)>),
    Identifier(Identifier),
    Field(Box<Expr>, Box<Expr>),
    Number(Number),
    BinOperation(Opcode, Box<Expr>, Box<Expr>),
    If {
        condition: Box<Expr>,
        then_body: Box<Expr>,
        else_body: Box<Expr>,
    },

    Call(Box<Expr>, Vec<Box<Expr>>),
    Block(Vec<Box<Statement>>, Option<Box<Expr>>),
    Group(Box<Expr>),
    Negative(Box<Number>),
    String(String),
}

impl Display for Expr {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // match self {
        //     Expr::StructAssign(ident, _fields) => write!(f, "struct {} {{}}", ident),
        //     Expr::Identifier(ident) => write!(f, "{}", ident),
        //     Expr::IdentifierWithAccess(ident, field) => write!(f, "{}.{}", ident, field),
        //     Expr::Number(num) => write!(f, "{}", num),
        //     Expr::BinOperation(opcode, lhs, rhs) => write!(f, "{} {} {}", lhs, opcode, rhs),
        //     Expr::If {
        //         condition,
        //         then_body,
        //         else_body,
        //     } => write!(
        //         f,
        //         "if {} {{ {} }} else {{ {} }}",
        //         condition, then_body, else_body
        //     ),
        // }

        todo!()
    }
}

#[derive(Debug)]
pub enum Number {
    Integer8(i8),
    Integer16(i16),
    Integer32(i32),
    UnSignInteger8(u8),
    UnSignInteger16(u16),
    UnSignInteger32(u32),
}
// impl Display for Number {
//     fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
//         match self {
//             Number::Integer8(n) => write!(f, "{}i8", n),
//             Number::Integer16(n) => write!(f, "{}i16", n),
//             Number::Integer32(n) => write!(f, "{}i32", n),
//             Number::UnSignInteger8(n) => write!(f, "{}u8", n),
//             Number::UnSignInteger16(n) => write!(f, "{}u16", n),
//             Number::UnSignInteger32(n) => write!(f, "{}u32", n),
//         }
//     }
// }
//
// impl Number {
//     pub fn negative(&self) -> Number {
//         match self {
//             Number::Integer8(n) => Number::Integer8(n *-1),
//             Number::Integer16(n) => Number::Integer16(n *-1),
//             Number::Integer32(n) => Number::Integer32(n *-1),
//             Number::UnSignInteger8(n) => unreachable!(),
//             Number::UnSignInteger16(n) => unreachable!(),
//             Number::UnSignInteger32(n) => unreachable!(),
//         }
//     }
//
//
//     pub fn codegen(&self, context: Arc<TyphoonContext>) -> *mut LLVMValue {
//         match self {
//             Number::Integer8(n) => Literal::int8(*n, context.llvm_context),
//             Number::Integer16(n) => Literal::int16(*n, context.llvm_context),
//             Number::Integer32(n) => Literal::int32(*n, context.llvm_context),
//             Number::UnSignInteger8(n) => Literal::uint8(*n, context.llvm_context),
//             Number::UnSignInteger16(n) => Literal::uint16(*n, context.llvm_context),
//             Number::UnSignInteger32(n) => Literal::uint32(*n, context.llvm_context),
//         }
//     }
//
//     pub fn get_type(&self, context: Arc<TyphoonContext>) -> Arc<Type> {
//         let number_name = match self {
//             Number::Integer8(_) => "i8",
//             Number::Integer16(_) => "i16",
//             Number::Integer32(_) => "i32",
//             Number::UnSignInteger8(_) => "u8",
//             Number::UnSignInteger16(_) => "u16",
//             Number::UnSignInteger32(_) => "u32",
//         };
//         let number_type_name = String::from(number_name);
//         context
//             .get_type_from_name(number_type_name)
//             .expect("cannot find type")
//     }
// }
// //
// // pub enum VariableType {
// //     Literal(LLVMValueRef),
// //     Ptr(LLVMValueRef),
// // }
//
// impl VariableType {
//     pub fn get_value(self, builder: LLVMBuilderRef) -> LLVMValueRef {
//         match self {
//             VariableType::Literal(s) => s,
//             VariableType::Ptr(ptr) => Build::load(ptr, builder),
//         }
//     }
//
//     pub fn unwrap(self) -> LLVMValueRef {
//         match self {
//             VariableType::Literal(s) => s,
//             VariableType::Ptr(ptr) => ptr,
//         }
//     }
// }
// impl Expr {
//     pub fn get_type(&self, upper_context: Arc<TyphoonContext>) -> Arc<Type> {
//         todo!()
//         // match self {
//         //     Expr::Number(number) => number.get_type(upper_context.clone()),
//         //     Expr::Identifier(identifier) => upper_context
//         //         .get_variable_type(identifier.clone())
//         //         .expect("cannot find type"),
//         //     Expr::BinOperation(opcode, lhs, rhs) => {
//         //         let lhs_type = lhs.get_type(upper_context.clone());
//         //         let rhs_type = rhs.get_type(upper_context.clone());
//         //         let option = lhs_type.get_operand_type(opcode.clone(), rhs_type);
//         //         // todo unwrap -> Option
//         //         upper_context
//         //             .get_type_from_id(option.expect("cannot get operand type"))
//         //             .expect("cannot find type")
//         //     }
//         //
//         //     Expr::If {
//         //         condition: _,
//         //         then_body,
//         //         else_body,
//         //     } => {
//         //         let then_ret_type = then_body.get_type(upper_context.clone());
//         //         let else_ret_type = else_body.get_type(upper_context.clone());
//         //         if !then_ret_type.equals(else_ret_type) {
//         //             panic!("the return type of then body is not equal to its of else body");
//         //         }
//         //         then_ret_type
//         //     }
//         //
//         //     Expr::StructAssign(ident, _fields) => upper_context
//         //         .get_type_from_name(ident.clone())
//         //         .expect(format!("cannot get type {}", ident).as_str()),
//         //
//         //     Expr::IdentifierWithAccess(ident, item) => {
//         //         let arc = ident.get_type(upper_context.clone());
//         //         arc.get_field_type(upper_context.clone(), item)
//         //             .expect("cannot found type")
//         //     }
//         // }
//     }
//
//     pub fn codegen(self, upper_context: Arc<TyphoonContext>) -> VariableType {
//         debug!("expr codegen: {}", &self);
//
//         match self {
//             Expr::Number(num) => VariableType::Literal(num.codegen(upper_context.clone())),
//
//             Expr::Identifier(identifier) => {
//                 let guard = upper_context.variables.read().unwrap();
//                 let x = guard
//                     .get(identifier.as_str())
//                     .expect(&format!("variable '{}' is undefined", identifier));
//                 let x = x.0;
//                 VariableType::Ptr(x)
//             }
//
//             Expr::BinOperation(opcode, lhs, rhs) => {
//                 let lhs_value = lhs
//                     .codegen(upper_context.clone())
//                     .get_value(upper_context.builder);
//                 let rhs_value = rhs
//                     .codegen(upper_context.clone())
//                     .get_value(upper_context.builder);
//
//                 VariableType::Literal(opcode.calculate_codegen(
//                     lhs_value,
//                     rhs_value,
//                     upper_context.clone(),
//                 ))
//             }
//
//             Expr::If {
//                 condition,
//                 then_body,
//                 else_body,
//             } => {
//                 let condition_value = condition
//                     .codegen(upper_context.clone())
//                     .get_value(upper_context.builder);
//                 let zero = Literal::int32(0, upper_context.llvm_context);
//
//                 let is_not_zero = Build::cmp(
//                     LLVMIntPredicate::LLVMIntNE,
//                     condition_value,
//                     zero,
//                     "is_not_zero",
//                     upper_context.builder,
//                 );
//
//                 let then_block = Build::append_block(
//                     upper_context.llvm_context,
//                     upper_context.function.unwrap(),
//                     "then_entry",
//                 );
//                 let else_block = Build::append_block(
//                     upper_context.llvm_context,
//                     upper_context.function.unwrap(),
//                     "else_entry",
//                 );
//                 let merge_block = Build::append_block(
//                     upper_context.llvm_context,
//                     upper_context.function.unwrap(),
//                     "merge_entry",
//                 );
//
//                 Build::cond_br(upper_context.builder, is_not_zero, then_block, else_block);
//
//                 Build::position_at_end(upper_context.builder, then_block);
//                 let then_return = then_body
//                     .codegen(upper_context.clone())
//                     .get_value(upper_context.builder);
//                 Build::goto(upper_context.builder, merge_block);
//
//                 Build::position_at_end(upper_context.builder, else_block);
//                 let else_return = else_body
//                     .codegen(upper_context.clone())
//                     .get_value(upper_context.builder);
//                 Build::goto(upper_context.builder, merge_block);
//
//                 Build::position_at_end(upper_context.builder, merge_block);
//
//                 let incoming = vec![(then_return, then_block), (else_return, else_block)];
//                 VariableType::Literal(Build::phi(
//                     upper_context.builder,
//                     Typ::int32(upper_context.llvm_context),
//                     incoming,
//                 ))
//             }
//             // Expr::StructAssign(ident, fields) => {
//             //     let struct_ty = upper_context
//             //         .get_type_from_name(ident.clone())
//             //         .expect("cannot find type");
//             //     let struct_llvm_ty = struct_ty.generate_type(upper_context.clone());
//             //
//             //     // store fields
//             //     // todo check un initial field
//             //     // todo check field type is equals to expr type
//             //     let mut fields_idx_value = fields
//             //         .into_iter()
//             //         .map(|(field_ident, expr)| {
//             //             let field_idx = struct_ty
//             //                 .get_type_field_idx(field_ident)
//             //                 .expect("field is not in struct define");
//             //             let expr_llvm_value = expr
//             //                 .codegen(upper_context.clone())
//             //                 .get_value(upper_context.builder);
//             //             (field_idx, expr_llvm_value)
//             //         })
//             //         .collect();
//             //
//             //     VariableType::Literal(Build::declare_struct(
//             //         ident,
//             //         struct_llvm_ty,
//             //         &mut fields_idx_value,
//             //         upper_context.builder,
//             //         upper_context.llvm_context,
//             //     ))
//             // }
//             // Expr::IdentifierWithAccess(ident, field) => {
//
//             // }
//             Expr::Field(ident, field) => {
//                 todo!()
//                 // // {ident}.{field}
//                 // let ident_type = ident.get_type(upper_context.clone());
//                 // let ident_codegen = ident.codegen(upper_context.clone()).unwrap();
//                 // let x1 = field.codegen(upper_context.clone()).unwrap();
//                 // let field_idx = ident_type
//                 //     .get_type_field_idx(field)
//                 //     .expect("struct has not item");
//                 // let gep = Build::gep(
//                 //     ident_codegen,
//                 //     field_idx,
//                 //     upper_context.builder,
//                 //     upper_context.llvm_context,
//                 // );
//                 // // Build::load(gep, upper_context.builder)
//                 // VariableType::Ptr(gep)
//             }
//             Expr::Call(_, _) => { todo!()}
//             Expr::Block(stats, expr) => {
//                 let context = TyphoonContext::new(upper_context.llvm_context, upper_context.builder, upper_context.module);
//
//                 for a in stats {
//                     a.codegen(upper_context.clone());
//                 }
//                 if let Some(inner ) = expr {
//                     inner.codegen(upper_context.clone())
//                 }else {
//                     todo!();
//                 }
//
//             }
//             Expr::Group(e) => {
//                 e.codegen(upper_context.clone())
//             }
//             Expr::Negative(n) => {
//                 VariableType::Literal(n.negative().codegen(upper_context.clone()))
//             }
//         }
//     }
// }
