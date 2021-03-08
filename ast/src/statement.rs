use crate::{Expr, Identifier, TypeName};
use std::fmt::Debug;

#[derive(Debug)]
pub enum Statement {
    Declare(Identifier, TypeName, Box<Expr>),
    Assignment(Identifier, Box<Expr>),
    Expr(Box<Expr>),
    Return(Box<Expr>),
}
// impl Display for Statement {
//     fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
//         match self {
//             Statement::Declare(ident, type_name, expr) => {
//                 write!(f, "let {}: {} = {}", ident, type_name, expr)
//             }
//             Statement::Return(expr) => write!(f, "return {}", expr),
//             Statement::Assignment(ident, expr) => {
//                 write!(f, "{} = {}", ident, expr)
//             }
//         }
//     }
// }
//
// impl Statement {
//     pub fn codegen(&self, upper_context: Arc<TyphoonContext>) -> *mut LLVMValue {
//         debug!("statement codegen: {}", &self);
//         match self {
//             Statement::Declare(identifier, _id_type, init) => {
//                 // let {identifier} : {_id_type} = {expr}
//                 let expr_type = init.get_type(upper_context.clone());
//
//                 let expr_value = init.codegen(upper_context.clone());
//
//                 let assigned_type = upper_context
//                     .get_type_from_name(_id_type.clone())
//                     .expect("cannot get type");
//
//                 if !assigned_type.equals(expr_type.clone()) {
//                     panic!(
//                         " expr type {} is not equals to assigned type {}",
//                         &expr_type.name, &assigned_type.name
//                     )
//                 }
//
//                 let assigned_llvm_type = assigned_type.generate_type(upper_context.clone());
//
//                 let a = if assigned_type.is_primitive() {
//                     Build::declare(
//                         identifier,
//                         assigned_llvm_type,
//                         expr_value.get_value(upper_context.builder),
//                         upper_context.builder,
//                     )
//                 } else {
//                     expr_value.unwrap()
//                 };
//
//                 upper_context.new_assign(identifier.clone(), a, expr_type.type_id);
//                 a
//             }
//             Statement::Return(expr) => {
//                 let x1 = expr
//                     .codegen(upper_context.clone())
//                     .get_value(upper_context.builder);
//                 Build::ret(x1, upper_context.builder)
//             }
//             Statement::Assignment(ident, expr) => {
//                 let guard = upper_context.variables.read().unwrap();
//                 let x = guard.get(ident).expect("cannot get variable");
//                 let variable_type = expr.codegen(upper_context.clone());
//                 let x2 = variable_type.get_value(upper_context.builder);
//                 Build::store(x.0, x2, upper_context.builder)
//             }
//         }
//     }
// }
