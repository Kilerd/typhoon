//! `typhoon-driver` — the compile → link → run pipeline behind the `typhoon` CLI.
//!
//! See `docs/DESIGN.md` §6.2 and §6.3. The driver owns the two steps that sit
//! after `typhoon-codegen`:
//!
//! ```text
//! source (.ty) --codegen--> text LLVM IR --clang -O2--> native binary
//! ```
//!
//! It is a library so that the golden test harness can drive exactly the same
//! pipeline the CLI does.
//!
//! # Environment overrides
//!
//! See [`toolchain`] for `TYPHOON_CLANG`, `TYPHOON_RUNTIME_LIB` and
//! `TYPHOON_GC_LIB_DIR`.
//!
//! # Example
//!
//! ```no_run
//! use typhoon_driver::{BuildOptions, compile_source, run_binary};
//!
//! let opts = BuildOptions::default();
//! let artifact = compile_source("hello.ty", "fn main():\n    print(\"hi\")\n", &opts)?;
//! let output = run_binary(artifact.binary(), &[])?;
//! assert_eq!(output.stdout, "hi\n");
//! # Ok::<(), typhoon_driver::DriverError>(())
//! ```

pub mod toolchain;

pub(crate) mod temp;

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use temp::TempDir;

/// How to build a Typhoon program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildOptions {
    /// `clang` optimisation level, 0 to 3. Defaults to 2 (DESIGN §6.2).
    pub opt_level: u8,
    /// Keep the generated LLVM IR in [`Artifact::ir`].
    pub emit_ir: bool,
    /// Leave the intermediate `.ll` file and scratch directory on disk.
    pub keep_temps: bool,
    /// Where to write the executable. `None` means a scratch directory owned by
    /// the returned [`Artifact`].
    pub output: Option<PathBuf>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            opt_level: 2,
            emit_ir: false,
            keep_temps: false,
            output: None,
        }
    }
}

/// A successfully built program.
#[derive(Debug)]
pub struct Artifact {
    binary: PathBuf,
    ir: Option<String>,
    /// Kept alive so that a scratch-directory binary outlives this value, and
    /// is deleted with it.
    _scratch: Option<TempDir>,
}

impl Artifact {
    /// Path of the linked executable.
    pub fn binary(&self) -> &Path {
        &self.binary
    }

    /// The textual LLVM IR, present when [`BuildOptions::emit_ir`] was set.
    pub fn ir(&self) -> Option<&str> {
        self.ir.as_deref()
    }
}

/// The result of running a compiled program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    /// Exit status; `128 + signal` when the process was killed by a signal.
    pub status: i32,
    /// Captured standard output, lossily decoded as UTF-8.
    pub stdout: String,
    /// Captured standard error, lossily decoded as UTF-8.
    pub stderr: String,
}

/// Everything that can go wrong between a `.ty` file and a running binary.
#[derive(Debug)]
pub enum DriverError {
    /// The frontend rejected the program; carries the rendered diagnostics.
    Compile(Vec<String>),
    /// `clang` failed; carries its combined output.
    Link(String),
    /// A required external tool or library could not be found.
    Toolchain(String),
    /// `opt_level` was outside `0..=3`.
    InvalidOptLevel(u8),
    /// A filesystem or process-spawn failure.
    Io(std::io::Error),
}

impl DriverError {
    /// The rendered diagnostics of a [`DriverError::Compile`], if any.
    pub fn diagnostics(&self) -> &[String] {
        match self {
            DriverError::Compile(diags) => diags,
            _ => &[],
        }
    }
}

impl fmt::Display for DriverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DriverError::Compile(diags) => {
                for (i, d) in diags.iter().enumerate() {
                    if i > 0 {
                        writeln!(f)?;
                    }
                    write!(f, "{d}")?;
                }
                Ok(())
            }
            DriverError::Link(message) => write!(f, "linking failed:\n{message}"),
            DriverError::Toolchain(message) => write!(f, "{message}"),
            DriverError::InvalidOptLevel(level) => {
                write!(
                    f,
                    "invalid optimisation level `{level}`: expected 0, 1, 2 or 3"
                )
            }
            DriverError::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for DriverError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DriverError::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for DriverError {
    fn from(err: std::io::Error) -> Self {
        DriverError::Io(err)
    }
}

/// Compiles Typhoon source to LLVM IR.
///
/// This is a thin wrapper over [`typhoon_codegen::compile_to_llvm_ir`] that
/// turns the diagnostics into a [`DriverError`].
///
/// # Errors
///
/// [`DriverError::Compile`] with the rendered diagnostics.
pub fn compile_to_ir(name: &str, src: &str) -> Result<String, DriverError> {
    typhoon_codegen::compile_to_llvm_ir(name, src).map_err(DriverError::Compile)
}

/// Compiles Typhoon source all the way to a native executable.
///
/// With `opts.output` unset the binary is placed in a scratch directory owned
/// by the returned [`Artifact`] and deleted when it is dropped.
///
/// # Errors
///
/// [`DriverError::Compile`] if the frontend rejects the program,
/// [`DriverError::Link`] if `clang` fails, [`DriverError::Toolchain`] if part
/// of the toolchain is missing.
pub fn compile_source(name: &str, src: &str, opts: &BuildOptions) -> Result<Artifact, DriverError> {
    let ir = compile_to_ir(name, src)?;

    let (binary, scratch) = match &opts.output {
        Some(path) => (path.clone(), None),
        None => {
            let mut scratch = TempDir::new("typhoon-build")?;
            if opts.keep_temps {
                scratch.keep();
            }
            let stem = Path::new(name)
                .file_stem()
                .map_or_else(|| "a.out".to_string(), |s| s.to_string_lossy().into_owned());
            (scratch.path().join(stem), Some(scratch))
        }
    };

    link_ll(&ir, &binary, opts)?;

    Ok(Artifact {
        binary,
        ir: opts.emit_ir.then_some(ir),
        _scratch: scratch,
    })
}

/// Optimises and links textual LLVM IR into the executable at `output`.
///
/// Runs
///
/// ```sh
/// clang -O<n> -Wno-override-module module.ll libtyphoon_runtime.a \
///     -L<gcdir> -lgc <native libs> -o <output>
/// ```
///
/// # Errors
///
/// [`DriverError::Link`] carrying `clang`'s own diagnostics when it fails,
/// [`DriverError::Toolchain`] when `clang`, the runtime archive or Boehm GC
/// cannot be found, [`DriverError::InvalidOptLevel`] for `opt_level > 3`.
pub fn link_ll(ll_text: &str, output: &Path, opts: &BuildOptions) -> Result<(), DriverError> {
    if opts.opt_level > 3 {
        return Err(DriverError::InvalidOptLevel(opts.opt_level));
    }

    let runtime = toolchain::runtime_lib()?;
    let gc_dir = toolchain::gc_lib_dir()?;
    let clang = toolchain::clang();

    let mut scratch = TempDir::new("typhoon-link")?;
    if opts.keep_temps {
        scratch.keep();
    }
    let stem = output
        .file_stem()
        .map_or_else(|| "module".into(), std::ffi::OsString::from);
    let ll_path = if opts.keep_temps {
        output.with_extension("ll")
    } else {
        scratch.path().join(stem).with_extension("ll")
    };
    if let Some(parent) = ll_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&ll_path, ll_text)?;

    if let Some(parent) = output.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    let mut cmd = Command::new(&clang);
    cmd.arg(format!("-O{}", opts.opt_level))
        // The generated module carries no target triple, so clang supplies the
        // host's; that is intended and must not warn.
        .arg("-Wno-override-module")
        .arg(&ll_path)
        .arg(&runtime)
        .arg(format!("-L{}", gc_dir.display()))
        .arg("-lgc")
        .args(toolchain::native_static_libs())
        .arg("-o")
        .arg(output);

    let out = cmd.output().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            clang_not_found(&clang)
        } else {
            DriverError::Io(err)
        }
    })?;

    if !out.status.success() {
        let mut message = String::new();
        message.push_str(&String::from_utf8_lossy(&out.stderr));
        message.push_str(&String::from_utf8_lossy(&out.stdout));
        if message.trim().is_empty() {
            message = format!("`{}` exited with {}", clang.display(), out.status);
        }
        return Err(DriverError::Link(message));
    }

    Ok(())
}

/// The actionable error reported when the C compiler cannot be spawned.
fn clang_not_found(clang: &Path) -> DriverError {
    DriverError::Toolchain(format!(
        "cannot run the C compiler `{}`\n\
         help: install clang (`brew install llvm` / `apt-get install clang`), \
         or set TYPHOON_CLANG to its path",
        clang.display()
    ))
}

/// Runs a compiled program, capturing its output.
///
/// # Errors
///
/// [`DriverError::Io`] when the process cannot be spawned.
pub fn run_binary(path: &Path, args: &[String]) -> Result<RunOutput, DriverError> {
    let out = Command::new(path).args(args).output()?;
    Ok(RunOutput {
        status: exit_status_code(&out.status),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// Normalises an exit status to an integer, mapping signals to `128 + signal`
/// the way a POSIX shell does.
fn exit_status_code(status: &std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    -1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_options_are_o2() {
        let opts = BuildOptions::default();
        assert_eq!(opts.opt_level, 2);
        assert!(!opts.emit_ir);
        assert!(!opts.keep_temps);
        assert_eq!(opts.output, None);
    }

    #[test]
    fn compile_to_ir_produces_a_module() {
        let ir = compile_to_ir("x.ty", "fn main():\n    print(1)\n").unwrap();
        assert!(ir.contains("define i32 @main()"), "{ir}");
    }

    #[test]
    fn compile_error_carries_frontend_diagnostics() {
        let err = compile_to_ir("x.ty", "fn main():\n    print(nope)\n").unwrap_err();
        assert!(matches!(err, DriverError::Compile(_)));
        assert_eq!(err.diagnostics().len(), 1);
        assert!(err.to_string().contains("cannot find value `nope`"));
    }

    #[test]
    fn invalid_opt_level_is_rejected_before_spawning_clang() {
        let opts = BuildOptions {
            opt_level: 4,
            ..BuildOptions::default()
        };
        let err = link_ll("", Path::new("/nonexistent/out"), &opts).unwrap_err();
        assert!(matches!(err, DriverError::InvalidOptLevel(4)));
        assert!(err.to_string().contains("invalid optimisation level"));
    }

    #[test]
    fn missing_clang_is_reported_with_a_fix() {
        let err = clang_not_found(Path::new("/no/such/clang"));
        let msg = err.to_string();
        assert!(matches!(err, DriverError::Toolchain(_)));
        assert!(msg.contains("/no/such/clang"), "{msg}");
        assert!(msg.contains("TYPHOON_CLANG"), "{msg}");
        assert!(msg.contains("brew install llvm"), "{msg}");
    }

    #[test]
    fn link_error_displays_clang_output() {
        let err = DriverError::Link("error: expected type".into());
        assert!(err.to_string().contains("linking failed"));
        assert!(err.to_string().contains("expected type"));
    }

    #[test]
    fn temp_dir_is_removed_on_drop() {
        let path = {
            let dir = TempDir::new("typhoon-test").unwrap();
            assert!(dir.path().is_dir());
            dir.path().to_path_buf()
        };
        assert!(!path.exists());
    }

    #[test]
    fn temp_dir_can_be_kept() {
        let path = {
            let mut dir = TempDir::new("typhoon-test-keep").unwrap();
            dir.keep();
            dir.path().to_path_buf()
        };
        assert!(path.is_dir());
        std::fs::remove_dir_all(&path).unwrap();
    }

    #[test]
    fn native_libs_are_known_for_this_platform() {
        let libs = toolchain::native_static_libs();
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            assert!(libs.contains(&"-lc"), "{libs:?}");
            assert!(libs.contains(&"-lm"), "{libs:?}");
        }
    }
}
