use crate::codegen::Codegen;
use crate::error::TyphoonError;
use crate::llvm_wrapper::context::TyphoonContext;
use ast::Module;
use llvm_sys::core::LLVMPrintModuleToString;
use llvm_sys::target::{
    LLVM_InitializeAllAsmParsers, LLVM_InitializeAllAsmPrinters, LLVM_InitializeAllTargetInfos,
    LLVM_InitializeAllTargetMCs, LLVM_InitializeAllTargets,
};
use llvm_sys::target_machine::{
    LLVMCodeGenFileType, LLVMCodeGenOptLevel, LLVMCodeModel, LLVMCreateTargetMachine,
    LLVMGetDefaultTargetTriple, LLVMGetTargetFromTriple, LLVMGetTargetName, LLVMRelocMode,
    LLVMTargetMachineEmitToFile, LLVMTargetRef,
};
use parser::parser::parse_module;
use std::path::PathBuf;
use std::time::SystemTime;
use std::{
    ffi::{CStr, CString},
    mem::MaybeUninit,
    path::Path,
    ptr,
};

pub struct Program {
    pub timestamp: u64,
    pub filename: String,
    pub build_folder: PathBuf,
    pub token_tree: Box<Module>,
}

impl Program {
    pub fn new(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let file_content = std::fs::read_to_string(path).unwrap();
        Program::new_with_string(path.to_path_buf(), &file_content)
    }

    pub fn new_with_string(filename: PathBuf, content: &str) -> Program {
        let module = parse_module(content).unwrap();

        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let build_folder = PathBuf::from("typhoon_build");

        let filename = filename.file_stem().unwrap().to_str().unwrap().to_string();
        let target_build_folder = build_folder.join(format!(
            "{}_{}",
            timestamp,
            &filename
        ));
        std::fs::create_dir_all(&target_build_folder).expect("Cannot create build folder");
        std::fs::write(target_build_folder.join("source.ty"), &content).expect("cannot output source code");
        Program {
            timestamp,
            filename,
            build_folder: target_build_folder,
            token_tree: Box::new(module),
        }
    }

    pub fn as_llir(self) -> String {
        todo!()
    }

    pub fn as_binary_output(
        self,
        debug: bool,
    ) -> Result<(i32, String, String), TyphoonError> {
        if debug {
            debug!("output ast file");
            std::fs::write(
                self.build_folder.join("ast"),
                format!("{:#?}", &self.token_tree),
            )
            .expect("cannot output ast file");
        }

        let context = TyphoonContext::new();
        let (module, builder) = self.token_tree.codegen(&context);
        unsafe {
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
            let name = CStr::from_ptr(name as *mut i8);
            debug!("name is {}", name.to_str().unwrap());

            // llir
            if debug {
                debug!("output llir file");
                let string = LLVMPrintModuleToString(module.to_llvm_module_ref());

                let x = CStr::from_ptr(string).to_str().unwrap();
                let llir = x.to_string();
                std::fs::write(self.build_folder.join("llir.ll"), format!("{}", llir))
                    .expect("cannot output llir file");
            }

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

            let o_file_ = self.build_folder
                .join(format!("{}.o", &self.filename))
                .to_str()
                .unwrap()
                .to_string();
            let o_file = CString::new(o_file_.as_str()).unwrap();
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

            debug!("link object file as binary {}", &self.filename);
            let execute_file_path = self.build_folder.join(&self.filename);
            let output = std::process::Command::new("cc")
                .arg(o_file_)
                .arg("-o")
                .arg(execute_file_path.to_str().unwrap())
                .output()
                .expect("error on executing linker cc");

            if output.status.success() {
                debug!("running binary file {}", &self.filename);
                let output = std::process::Command::new(execute_file_path)
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
