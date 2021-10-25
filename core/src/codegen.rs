use crate::llvm_wrapper::builder::TyphoonBuilder;
use crate::llvm_wrapper::context::TyphoonContext;
use crate::llvm_wrapper::module::TyphoonModule;
use crate::llvm_wrapper::types::void_type::VoidType;
use crate::llvm_wrapper::types::BasicType;
use crate::llvm_wrapper::values::BasicValue;
use ast::{Expr, FunctionDeclare, Module, ModuleItem, Number, Statement, StructDeclare, Type};
use llvm_sys::core::{LLVMBuildRet, LLVMBuildRetVoid};

pub trait Codegen {
    fn codegen(self, context: &TyphoonContext) -> (TyphoonModule, TyphoonBuilder);
}

pub trait ModuleCodegen {
    fn module_codegen(
        self,
        context: &TyphoonContext,
        builder: &TyphoonBuilder,
        module: &TyphoonModule,
    );
}

pub trait ExprCodegen {
    fn expr_codegen(
        self,
        context: &TyphoonContext,
        builder: &TyphoonBuilder,
        module: &TyphoonModule,
    ) -> BasicValue;
}

impl Codegen for Module {
    fn codegen(self, context: &TyphoonContext) -> (TyphoonModule, TyphoonBuilder) {
        debug!("module codegen");
        let module = context.create_module("typhoon");
        let builder = context.create_builder();
        for item in self.items {
            item.module_codegen(&context, &builder, &module);
        }
        (module, builder)
    }
}

impl ModuleCodegen for ModuleItem {
    fn module_codegen(
        self,
        context: &TyphoonContext,
        builder: &TyphoonBuilder,
        module: &TyphoonModule,
    ) {
        match self {
            ModuleItem::FunctionDeclare(func_decl) => {
                func_decl.module_codegen(context, builder, module);
            }
            ModuleItem::StructDeclare(struct_decl) => {
                struct_decl.module_codegen(context, builder, module);
            }
        }
    }
}

impl ModuleCodegen for StructDeclare {
    fn module_codegen(
        self,
        context: &TyphoonContext,
        builder: &TyphoonBuilder,
        module: &TyphoonModule,
    ) {
        // let struct_type = context.opaque_struct_type(&self.name);
        for it in self.fields {}
        // struct_type.set_body()
    }
}

fn to_basic_type(ty: &Type, context: &TyphoonContext) -> BasicType {
    match ty.name.as_str() {
        "i8" => context.i8_type().as_basic_type(),
        "i16" => context.i16_type().as_basic_type(),
        "i32" => context.i32_type().as_basic_type(),
        "i64" => context.i64_type().as_basic_type(),
        "" => context.void_type().as_basic_type(),
        _ => {
            unimplemented!()
        }
    }
}

impl ModuleCodegen for FunctionDeclare {
    fn module_codegen(
        self,
        context: &TyphoonContext,
        builder: &TyphoonBuilder,
        module: &TyphoonModule,
    ) {
        debug!("function {} codegen", &self.name);

        let return_type = to_basic_type(&self.return_type, &context);
        let args: Vec<BasicType> = self
            .args
            .into_iter()
            .map(|(name, ty)| to_basic_type(&ty, &context))
            .collect();
        let function_type = return_type.fn_type(&args, false);
        let function_value = module.add_function(&self.name, function_type);
        let block = context.append_basic_block(function_value, "entry");
        builder.position_at_end(&block);
        let x = self.stats.expr_codegen(context, builder, module);
        dbg!(x);
    }
}

impl ModuleCodegen for Statement {
    fn module_codegen(
        self,
        context: &TyphoonContext,
        builder: &TyphoonBuilder,
        module: &TyphoonModule,
    ) {
        match self {
            Statement::Declare(_, _, _) => {}
            Statement::Assignment(_, _) => {}
            Statement::Expr(_) => {}
            Statement::Return(expr) => {
                trace!("build return");
                let value = expr.expr_codegen(&context, &builder, &module);
                dbg!(&value);
                builder.build_return(value);
            }
        }
    }
}

impl ExprCodegen for Expr {
    fn expr_codegen(
        self,
        context: &TyphoonContext,
        builder: &TyphoonBuilder,
        module: &TyphoonModule,
    ) -> BasicValue {
        match self {
            Expr::Identifier(_) => {
                unimplemented!()
            }
            Expr::Field(_, _) => {
                unimplemented!()
            }
            Expr::Number(n) => {
                trace!("build number");
                let number_int_value = match n {
                    Number::Integer8(inner) => context.i8_type().const_int(inner as u64, true),
                    Number::Integer16(inner) => context.i16_type().const_int(inner as u64, true),
                    Number::Integer32(inner) => context.i32_type().const_int(inner as u64, true),
                    Number::UnSignInteger8(inner) => {
                        unimplemented!()
                    }
                    Number::UnSignInteger16(inner) => {
                        unimplemented!()
                    }
                    Number::UnSignInteger32(inner) => {
                        unimplemented!()
                    }
                };
                number_int_value.into_basic_value()
            }
            Expr::BinOperation(_, _, _) => {
                unimplemented!()
            }
            Expr::If { .. } => {
                unimplemented!()
            }
            Expr::Call(_, _) => {
                unimplemented!()
            }
            Expr::Block(stats, ret) => {
                for statement in stats {
                    statement.module_codegen(&context, &builder, &module);
                }

                if let Some(ret_expr) = ret {
                    ret_expr.expr_codegen(&context, &builder, &module)
                } else {
                    let value = context.void_type().const_value();
                    value.into_basic_value()
                }
            }
            Expr::Group(_) => {
                unimplemented!()
            }
            Expr::Negative(_) => {
                unimplemented!()
            }
            Expr::String(_) => {
                unimplemented!()
            }
        }
    }
}
