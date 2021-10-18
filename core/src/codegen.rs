use crate::llvm_wrapper::builder::TyphoonBuilder;
use crate::llvm_wrapper::context::TyphoonContext;
use crate::llvm_wrapper::module::TyphoonModule;
use crate::llvm_wrapper::types::BasicType;
use ast::{Expr, FunctionDeclare, Module, ModuleItem, Statement, StructDeclare, Type};
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
    );
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
        "" => context.void_type(),
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
        // let x = self.stats.expr_codegen(context, module);
        builder.position_at_end(&block);
        let value = context.i32_type().const_int(10, false);
        unsafe { LLVMBuildRet(builder.as_llvm_ref(), value.as_llvm_ref()) };
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
            Statement::Return(_) => {}
        }
    }
}

impl ExprCodegen for Expr {
    fn expr_codegen(
        self,
        context: &TyphoonContext,
        builder: &TyphoonBuilder,
        module: &TyphoonModule,
    ) {
        match self {
            Expr::Identifier(_) => {}
            Expr::Field(_, _) => {}
            Expr::Number(_) => {}
            Expr::BinOperation(_, _, _) => {}
            Expr::If { .. } => {}
            Expr::Call(_, _) => {}
            Expr::Block(stats, ret) => {}
            Expr::Group(_) => {}
            Expr::Negative(_) => {}
            Expr::String(_) => {}
        }
    }
}
