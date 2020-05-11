use lalrpop_util::lalrpop_mod;
use llvm_sys::core;
use llvm_sys::core::LLVMPrintValueToString;
use llvm_sys::target::{
    LLVM_InitializeAllAsmParsers, LLVM_InitializeAllAsmPrinters, LLVM_InitializeAllTargetInfos,
    LLVM_InitializeAllTargetMCs, LLVM_InitializeAllTargets, LLVM_InitializeNativeTarget,
};
use llvm_sys::target_machine::{
    LLVMCodeGenFileType, LLVMCodeGenOptLevel, LLVMCodeModel, LLVMCreateTargetMachine,
    LLVMGetDefaultTargetTriple, LLVMGetFirstTarget, LLVMGetTargetFromName, LLVMGetTargetFromTriple,
    LLVMGetTargetName, LLVMRelocMode, LLVMTarget, LLVMTargetMachineEmitToFile, LLVMTargetRef,
};
use std::ffi::{CStr, CString};
use std::ptr;

pub mod ast;
lalrpop_mod!(pub parser);

macro_rules! c_str {
    ($s:expr) => {
        concat!($s, "\0").as_ptr() as *const i8
    };
}

fn main() {
    dbg!(parser::ExprParser::new().parse("((22+2)*3%2)|1"));
    dbg!(parser::ExprParser::new().parse("22"));
    dbg!(parser::ExprParser::new().parse("a"));
    dbg!(parser::ExprParser::new().parse("_a+2"));
    dbg!(parser::StatementParser::new().parse("let b : i32 = a+2"));
    dbg!(parser::StatementParser::new().parse("let a :i32 = ((22+dDS)*3%c)|1"));

    unsafe {
        let context = core::LLVMContextCreate();
        let module = core::LLVMModuleCreateWithName(c_str!("test"));
        let builder = core::LLVMCreateBuilderInContext(context);

        let int_type = core::LLVMInt32TypeInContext(context);
        let function_type = core::LLVMFunctionType(int_type, ptr::null_mut(), 0, 0);
        let function = core::LLVMAddFunction(module, c_str!("main"), function_type);

        let bb = core::LLVMAppendBasicBlockInContext(context, function, c_str!("entry"));
        core::LLVMPositionBuilderAtEnd(builder, bb);

        let int_value = 100;

        let int = core::LLVMConstInt(int_type, int_value, 0);
        core::LLVMBuildRet(builder, int);

        // compile to object
        let triple = LLVMGetDefaultTargetTriple();
        LLVM_InitializeAllTargetInfos();
        LLVM_InitializeAllTargets();
        LLVM_InitializeAllTargetMCs();
        LLVM_InitializeAllAsmParsers();
        LLVM_InitializeAllAsmPrinters();
        // let target = LLVMGetFirstTarget();
        // LLVMGetTargetFromTriple(triple, target, ptr::null_mut());
        let mut target: LLVMTargetRef = std::mem::uninitialized();
        LLVMGetTargetFromTriple(triple, &mut target, ptr::null_mut());
        let opt_level = LLVMCodeGenOptLevel::LLVMCodeGenLevelLess;
        let reloc_mode = LLVMRelocMode::LLVMRelocDefault;
        let code_model = LLVMCodeModel::LLVMCodeModelDefault;

        dbg!(&opt_level);
        dbg!(&reloc_mode);
        dbg!(&code_model);
        dbg!(&triple);
        let name = LLVMGetTargetName(target);
        let x = CStr::from_ptr(name as *mut i8);
        println!("target name: {:?}", x);
        let target_machine = LLVMCreateTargetMachine(
            target,
            triple,
            c_str!("x86-64"),
            c_str!(""),
            opt_level,
            reloc_mode,
            code_model,
        );
        let file_type = LLVMCodeGenFileType::LLVMObjectFile;

        let mut error_str = ptr::null_mut();
        let output_file_name = CString::new("out.o").unwrap();
        let ret = LLVMTargetMachineEmitToFile(
            target_machine,
            module,
            output_file_name.as_ptr() as *mut i8,
            file_type,
            &mut error_str,
        );
        if ret == 1 {
            let x = CStr::from_ptr(error_str);
            panic!("compile failed: {:?}", x);
        }
        let output = std::process::Command::new("cc")
            .arg("out.o")
            .arg("-o")
            .arg("out")
            .output()
            .expect("error on executing linker cc");
        // core::LLVMPrintModuleToFile(module, c_str!("out.ll"), ptr::null_mut());

        core::LLVMDisposeBuilder(builder);
        core::LLVMDisposeModule(module);
        core::LLVMContextDispose(context);
    }
}
