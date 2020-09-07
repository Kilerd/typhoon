use crate::literal::Literal;
use llvm_sys::{
    core::{
        LLVMAddFunction, LLVMAddIncoming, LLVMAppendBasicBlockInContext, LLVMBuildAlloca,
        LLVMBuildBr, LLVMBuildCondBr, LLVMBuildGEP, LLVMBuildICmp, LLVMBuildLoad, LLVMBuildPhi,
        LLVMBuildRet, LLVMBuildStore, LLVMModuleCreateWithName, LLVMPositionBuilderAtEnd,
    },
    prelude::{
        LLVMBasicBlockRef, LLVMBuilderRef, LLVMContextRef, LLVMModuleRef, LLVMTypeRef, LLVMValueRef,
    },
    LLVMIntPredicate,
};
use std::ffi::CString;

pub struct Build;

impl Build {
    pub fn position_at_end(builder: LLVMBuilderRef, block: LLVMBasicBlockRef) {
        unsafe { LLVMPositionBuilderAtEnd(builder, block) }
    }

    pub fn append_block(
        context: LLVMContextRef,
        func: LLVMValueRef,
        name: &str,
    ) -> LLVMBasicBlockRef {
        let name = CString::new(name).unwrap();
        unsafe { LLVMAppendBasicBlockInContext(context, func, name.as_ptr()) }
    }

    pub fn add_func_to_module(
        module: LLVMModuleRef,
        func_name: &str,
        func_ty: LLVMTypeRef,
    ) -> LLVMValueRef {
        let name = CString::new(func_name).unwrap();
        unsafe { LLVMAddFunction(module, name.as_ptr(), func_ty) }
    }

    pub fn goto(builder: LLVMBuilderRef, to: LLVMBasicBlockRef) -> LLVMValueRef {
        unsafe { LLVMBuildBr(builder, to) }
    }

    pub fn cond_br(
        builder: LLVMBuilderRef,
        if_: LLVMValueRef,
        then_block: LLVMBasicBlockRef,
        else_block: LLVMBasicBlockRef,
    ) -> LLVMValueRef {
        unsafe { LLVMBuildCondBr(builder, if_, then_block, else_block) }
    }

    pub fn cmp(
        op: LLVMIntPredicate,
        lhs: LLVMValueRef,
        rhs: LLVMValueRef,
        name: &str,
        builder: LLVMBuilderRef,
    ) -> LLVMValueRef {
        let name = CString::new(name).unwrap();
        unsafe { LLVMBuildICmp(builder, op, lhs, rhs, name.as_ptr()) }
    }

    pub fn phi(
        builder: LLVMBuilderRef,
        ty: LLVMTypeRef,
        incoming: Vec<(LLVMValueRef, LLVMBasicBlockRef)>,
    ) -> LLVMValueRef {
        let (mut values, mut blocks): (Vec<LLVMValueRef>, Vec<LLVMBasicBlockRef>) =
            incoming.into_iter().unzip();
        unsafe {
            let phi = LLVMBuildPhi(builder, ty, c_str!("phi_"));
            LLVMAddIncoming(
                phi,
                values.as_mut_ptr(),
                blocks.as_mut_ptr(),
                values.len() as u32,
            );
            phi
        }
    }

    pub fn module(name: &str) -> LLVMModuleRef {
        let name = CString::new(name).unwrap();
        unsafe { LLVMModuleCreateWithName(name.as_ptr()) }
    }

    pub fn declare_struct(
        name: &str,
        typ: LLVMTypeRef,
        fields: &mut Vec<(u32, LLVMValueRef)>,
        builder: LLVMBuilderRef,
        context: LLVMContextRef,
    ) -> LLVMValueRef {
        let name = CString::new(name.to_lowercase()).unwrap();
        unsafe {
            let struct_al = LLVMBuildAlloca(builder, typ, name.as_ptr());

            for (idx, field_value) in fields.into_iter() {
                let slice_idx = Literal::int32(0, context);
                let field_dix_t = Literal::int32(*idx as i32, context);
                let mut vec1 = vec![slice_idx, field_dix_t];

                let gep = LLVMBuildGEP(
                    builder,
                    struct_al,
                    vec1.as_mut_ptr(),
                    vec1.len() as u32,
                    c_str!("dsp_"),
                );
                LLVMBuildStore(builder, *field_value, gep);
            }

            struct_al
        }
    }

    pub fn declare(
        name: &str,
        typ: LLVMTypeRef,
        value: LLVMValueRef,
        builder: LLVMBuilderRef,
    ) -> LLVMValueRef {
        let name = CString::new(name).unwrap();
        unsafe {
            let variable = LLVMBuildAlloca(builder, typ, name.as_ptr());
            LLVMBuildStore(builder, value, variable);
            variable
        }
    }

    pub fn store(
        variable: LLVMValueRef,
        value: LLVMValueRef,
        builder: LLVMBuilderRef,
    ) -> LLVMValueRef {
        unsafe { LLVMBuildStore(builder, value, variable) }
    }

    pub fn load(variable: LLVMValueRef, builder: LLVMBuilderRef) -> LLVMValueRef {
        unsafe { LLVMBuildLoad(builder, variable, c_str!("load_")) }
    }

    pub fn ret(variable: LLVMValueRef, builder: LLVMBuilderRef) -> LLVMValueRef {
        unsafe { LLVMBuildRet(builder, variable) }
    }

    pub fn gep(
        ty: LLVMValueRef,
        idx: u32,
        builder: LLVMBuilderRef,
        context: LLVMContextRef,
    ) -> LLVMValueRef {
        unsafe {
            let mut idx_vec = vec![Literal::uint32(0, context), Literal::uint32(idx, context)];
            LLVMBuildGEP(
                builder,
                ty,
                idx_vec.as_mut_ptr(),
                idx_vec.len() as u32,
                c_str!("gep_"),
            )
        }
    }

    pub fn add() -> LLVMValueRef {
        todo!()
    }

    pub fn sub() -> LLVMValueRef {
        todo!()
    }

    pub fn mul() -> LLVMValueRef {
        todo!()
    }

    pub fn div() -> LLVMValueRef {
        todo!()
    }
}
