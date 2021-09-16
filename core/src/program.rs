use crate::codegen::Codegen;
use crate::error::TyphoonError;
use crate::llvm_wrapper::context::TyphoonContext;
use ast::Module;
use llvm_sys::target::{
    LLVM_InitializeAllAsmParsers, LLVM_InitializeAllAsmPrinters, LLVM_InitializeAllTargetInfos,
    LLVM_InitializeAllTargetMCs, LLVM_InitializeAllTargets,
};
use llvm_sys::target_machine::{LLVMCodeGenFileType, LLVMCreateTargetMachine, LLVMGetDefaultTargetTriple, LLVMGetTargetFromTriple, LLVMGetTargetName, LLVMTargetMachineEmitToFile, LLVMTargetRef, LLVMCodeGenOptLevel, LLVMRelocMode, LLVMCodeModel};
use parser::parser::parse_module;
use std::{
    ffi::{CStr, CString},
    mem::MaybeUninit,
    path::Path,
    ptr,
};

pub struct Program {
    pub token_tree: Box<Module>,
}

impl Program {
    pub fn new(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let file_content = std::fs::read_to_string(path).unwrap();
        Program::new_with_string(&file_content)
    }

    pub fn new_with_string(content: &str) -> Program {
        let module = parse_module(content).unwrap();
        Program {
            token_tree: Box::new(module),
        }
    }

    pub fn as_llir(self) -> String {
        todo!()
    }

    pub fn as_binary_output(
        self,
        output_name: &str,
    ) -> Result<(i32, String, String), TyphoonError> {
        let context = TyphoonContext::new();
        let module = self.token_tree.codegen(context);
        unsafe {
            // unsafe {
            //     let context = core::LLVMContextCreate();
            //     let builder = core::LLVMCreateBuilderInContext(context);
            //     let module = self.token_tree.codegen(context, builder);
            //
            debug!("init target message");

            let triple = LLVMGetDefaultTargetTriple();
            LLVM_InitializeAllTargetInfos();
            LLVM_InitializeAllTargets();
            LLVM_InitializeAllTargetMCs();
            LLVM_InitializeAllAsmParsers();
            LLVM_InitializeAllAsmPrinters();
            let mut target: MaybeUninit<LLVMTargetRef> = std::mem::MaybeUninit::uninit();
            LLVMGetTargetFromTriple(triple, target.as_mut_ptr(), ptr::null_mut());
            let target = target.assume_init();
            let opt_level = LLVMCodeGenOptLevel::LLVMCodeGenLevelNone;
            let reloc_mode = LLVMRelocMode::LLVMRelocDefault;
            let code_model = LLVMCodeModel::LLVMCodeModelDefault;

            let name = LLVMGetTargetName(target);
            let _x = CStr::from_ptr(name as *mut i8);
            debug!("creating target machine");
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

            let o_file_ = format!("{}.o", output_name);
            let o_file = CString::new(o_file_).unwrap();
            let mut error_str = ptr::null_mut();

            debug!("output object file {:?}", &o_file);
            let ret = LLVMTargetMachineEmitToFile(
                target_machine,
                module.to_llvm_module_ref(),
                o_file.as_ptr() as *mut i8,
                file_type,
                &mut error_str,
            );
            debug!("clean up llvm module");

            if ret == 1 {
                let x = CStr::from_ptr(error_str);

                return Err(TyphoonError::CompileError(x.to_str().unwrap().to_string()));
            }

            debug!("link object file as binary {}", &output_name);
            let output = std::process::Command::new("cc")
                .arg(format!("{}.o", output_name))
                .arg("-o")
                .arg(output_name)
                .output()
                .expect("error on executing linker cc");

            if output.status.success() {
                debug!("running binary file {}", &output_name);
                let output = std::process::Command::new(format!("./{}", output_name))
                    .output()
                    .expect("error on executing output file");

                let stdout = String::from_utf8(output.stdout).unwrap();
                let stderr = String::from_utf8(output.stderr).unwrap();

                Ok((output.status.code().unwrap(), stdout, stderr))
            } else {
                println!("cannot emit executing file");
                let stdout = String::from_utf8(output.stdout).unwrap();
                let stderr = String::from_utf8(output.stderr).unwrap();
                Err(TyphoonError::LinkError(output.status, stdout, stderr))
            }
        }
    }
}
