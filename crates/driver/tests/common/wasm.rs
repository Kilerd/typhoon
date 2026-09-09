//! Running a compiled Typhoon program on the wasm backend, under `wasmtime`.
//!
//! This is the test-side twin of `crates/driver/assets/loader.js`: it provides
//! the same two host imports (`host_write`, `host_exit`), instantiates the
//! runtime module, then the program module with the runtime's exports, and
//! calls `_start`. If the two ever disagree, the golden suite and the browser
//! would disagree too — so they are deliberately spelled the same way.

use std::sync::OnceLock;

use wasmtime::{Caller, Engine, Extern, Linker, Module, Store};

/// What a program printed and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmOutput {
    /// Exit status: 0 normally, 101 for a panic, 134 for a trap.
    pub status: i32,
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
}

/// Exit code the loader reports when the module traps instead of exiting.
pub const TRAP_EXIT_CODE: i32 = 134;

/// The per-run host state.
struct Host {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit: Option<i32>,
    /// How far the linear memory may grow, when the run is budgeted.
    memory_limit: Option<usize>,
}

/// The memory budget of a run, so that a test can watch the wasm backend hit
/// the limit v1 has no collector to avoid.
impl wasmtime::ResourceLimiter for Host {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(self.memory_limit.is_none_or(|limit| desired <= limit))
    }

    fn table_growing(
        &mut self,
        _current: usize,
        _desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(true)
    }
}

/// The error `host_exit` returns to unwind out of wasm, the way the JS loader
/// throws a sentinel.
#[derive(Debug)]
struct ExitSignal(i32);

impl std::fmt::Display for ExitSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "typhoon: exit {}", self.0)
    }
}

impl std::error::Error for ExitSignal {}

/// The engine and the compiled runtime module, built once for the whole test
/// binary: compiling the runtime for every case would dominate the run time.
fn runtime() -> &'static (Engine, Module) {
    static RUNTIME: OnceLock<(Engine, Module)> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        let engine = Engine::default();
        let bytes = typhoon_driver::wasm::runtime_module()
            .unwrap_or_else(|err| panic!("no wasm runtime available: {err}"));
        let module = Module::new(&engine, bytes).expect("the runtime module is valid");
        (engine, module)
    })
}

/// Whether this build can run wasm programs at all.
pub fn is_available() -> bool {
    typhoon_driver::wasm::is_available()
}

/// Runs a compiled Typhoon wasm module and captures its output.
///
/// # Errors
///
/// A message when the module cannot be compiled or instantiated — a mismatched
/// import, an invalid module, an engine error.
pub fn run_wasm(program: &[u8]) -> Result<WasmOutput, String> {
    run_wasm_with_limit(program, None)
}

/// Runs a compiled Typhoon wasm module with a cap on how large its linear
/// memory may grow.
///
/// # Errors
///
/// As [`run_wasm`].
pub fn run_wasm_with_limit(
    program: &[u8],
    memory_limit: Option<usize>,
) -> Result<WasmOutput, String> {
    let (engine, runtime_module) = runtime();
    let program_module =
        Module::new(engine, program).map_err(|err| format!("invalid program module: {err:?}"))?;

    let mut store = Store::new(
        engine,
        Host {
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit: None,
            memory_limit,
        },
    );
    store.limiter(|host| host);

    let mut linker: Linker<Host> = Linker::new(engine);
    linker
        .func_wrap(
            "env",
            "host_write",
            |mut caller: Caller<'_, Host>, fd: i32, ptr: i32, len: i32| {
                if len <= 0 {
                    return Ok(());
                }
                let memory = match caller.get_export("memory") {
                    Some(Extern::Memory(memory)) => memory,
                    _ => return Err(wasmtime::Error::msg("the runtime exports no memory")),
                };
                let mut bytes = vec![0u8; len as usize];
                memory
                    .read(&mut caller, ptr as usize, &mut bytes)
                    .map_err(|err| wasmtime::Error::msg(format!("bad host_write: {err}")))?;
                if fd == 2 {
                    caller.data_mut().stderr.extend_from_slice(&bytes);
                } else {
                    caller.data_mut().stdout.extend_from_slice(&bytes);
                }
                Ok(())
            },
        )
        .map_err(|err| err.to_string())?;
    linker
        .func_wrap(
            "env",
            "host_exit",
            |mut caller: Caller<'_, Host>, code: i32| -> Result<(), wasmtime::Error> {
                caller.data_mut().exit = Some(code);
                Err(wasmtime::Error::new(ExitSignal(code)))
            },
        )
        .map_err(|err| err.to_string())?;

    let runtime_instance = linker
        .instantiate(&mut store, runtime_module)
        .map_err(|err| format!("cannot instantiate the runtime: {err:?}"))?;

    // Everything the runtime exports becomes an `env.*` import for the program,
    // exactly as the JS loader hands it `runtimeInstance.exports`.
    let exports: Vec<(String, Extern)> = runtime_instance
        .exports(&mut store)
        .map(|export| (export.name().to_string(), export.into_extern()))
        .collect();
    for (name, item) in exports {
        linker
            .define(&mut store, "env", &name, item)
            .map_err(|err| format!("cannot define env.{name}: {err}"))?;
    }

    let instance = linker
        .instantiate(&mut store, &program_module)
        .map_err(|err| format!("cannot instantiate the program: {err:?}"))?;
    let start = instance
        .get_typed_func::<(), ()>(&mut store, typhoon_codegen_wasm::START_EXPORT)
        .map_err(|err| format!("the program exports no _start: {err}"))?;

    let outcome = start.call(&mut store, ());
    let mut status = 0;
    if let Err(err) = outcome {
        match store.data().exit {
            Some(code) => status = code,
            None => {
                // A trap that is not our exit signal: report it the way the JS
                // loader does, with the message on stderr. The trap code alone
                // (`unreachable`, `out of bounds memory access`, …) is what a
                // browser shows too; the wasm backtrace under it would bury it.
                status = TRAP_EXIT_CODE;
                let message = match err.downcast_ref::<wasmtime::Trap>() {
                    Some(trap) => format!("trap: {trap}\n"),
                    None => format!("trap: {err}\n"),
                };
                store
                    .data_mut()
                    .stderr
                    .extend_from_slice(message.as_bytes());
            }
        }
    }

    let host = store.data();
    Ok(WasmOutput {
        status,
        stdout: String::from_utf8_lossy(&host.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&host.stderr).into_owned(),
    })
}

/// Compiles Typhoon source and runs it on the wasm backend.
///
/// # Errors
///
/// The rendered diagnostics if it does not compile, or the engine's message if
/// it cannot run.
pub fn compile_and_run(name: &str, source: &str) -> Result<WasmOutput, String> {
    let module = typhoon_codegen_wasm::compile_to_wasm(name, source)
        .map_err(|diagnostics| diagnostics.join("\n"))?;
    run_wasm(&module)
}
