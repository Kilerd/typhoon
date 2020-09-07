use crate::error::TyphoonError;
use ast::Module;
use llvm_sys::{
    core,
    core::LLVMPrintModuleToString,
    target::{
        LLVM_InitializeAllAsmParsers, LLVM_InitializeAllAsmPrinters, LLVM_InitializeAllTargetInfos,
        LLVM_InitializeAllTargetMCs, LLVM_InitializeAllTargets,
    },
    target_machine::{
        LLVMCodeGenFileType, LLVMCodeGenOptLevel, LLVMCodeModel, LLVMCreateTargetMachine,
        LLVMGetDefaultTargetTriple, LLVMGetTargetFromTriple, LLVMGetTargetName, LLVMRelocMode,
        LLVMTargetMachineEmitToFile, LLVMTargetRef,
    },
};
use llvm_wrapper::c_str;
use parser::parser::ModuleParser;
use std::mem::MaybeUninit;
use std::{
    ffi::{CStr, CString},
    path::Path,
    process::ExitStatus,
    ptr,
};

pub struct Program {
    pub token_tree: Box<Module>,
}

impl Program {
    pub fn new(path: impl AsRef<Path>) -> Result<Self, TyphoonError> {
        let path = path.as_ref();
        let file_content = std::fs::read_to_string(path)
            .map_err(|e| TyphoonError::FileError(path.to_str().unwrap().to_string(), e))?;
        Program::new_with_string(file_content)
    }

    pub fn new_with_string(source: String) -> Result<Self, TyphoonError> {
        let token_tree: Box<Module> = ModuleParser::new()
            .parse(&source)
            .map_err(|e| TyphoonError::ParserError(e.to_string()))?;

        Ok(Self { token_tree })
    }

    pub fn as_llir(&mut self) -> String {
        unsafe {
            let context = core::LLVMContextCreate();
            let builder = core::LLVMCreateBuilderInContext(context);
            let module = self.token_tree.codegen(context, builder);

            let string = LLVMPrintModuleToString(module);

            core::LLVMDisposeBuilder(builder);
            core::LLVMDisposeModule(module);
            core::LLVMContextDispose(context);

            let x = CStr::from_ptr(string).to_str().unwrap();
            x.to_string()
        }
    }

    pub fn as_binary_output(
        &mut self,
        output_name: &str,
    ) -> Result<(ExitStatus, String, String), TyphoonError> {
        unsafe {
            let context = core::LLVMContextCreate();
            let builder = core::LLVMCreateBuilderInContext(context);
            let module = self.token_tree.codegen(context, builder);

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
                module,
                o_file.as_ptr() as *mut i8,
                file_type,
                &mut error_str,
            );
            debug!("clean up llvm module");
            core::LLVMDisposeBuilder(builder);
            core::LLVMDisposeModule(module);
            core::LLVMContextDispose(context);

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

                Ok((output.status, stdout, stderr))
            } else {
                println!("cannot emit executing file");
                let stdout = String::from_utf8(output.stdout).unwrap();
                let stderr = String::from_utf8(output.stderr).unwrap();
                Err(TyphoonError::LinkError(output.status, stdout, stderr))
            }
        }
    }
}
