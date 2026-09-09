//! `typhoon` — the compiler's command line interface (DESIGN section 6.1).
//!
//! ```text
//! typhoon build <file> [-o OUT] [-O 0..3] [--emit-ir] [--keep-temps]
//! typhoon run   <file> [-- ARGS...]
//! typhoon emit-ir <file> [-o OUT]
//! typhoon check <file>
//! ```
//!
//! Exit codes: `0` success, `1` compile or link error (diagnostics on stderr),
//! `2` usage error (bad flags, or an input file that cannot be read).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use clap::{Parser, Subcommand};
use typhoon_driver::{BuildOptions, DriverError, compile_source, compile_to_ir};

/// Exit code for a compile or link failure.
const EXIT_ERROR: u8 = 1;
/// Exit code for a usage error, matching clap's own convention.
const EXIT_USAGE: u8 = 2;

#[derive(Debug, Parser)]
#[command(
    name = "typhoon",
    version,
    about = "The Typhoon compiler: Python-flavoured syntax, static types, native binaries",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Compile a Typhoon source file to a native executable.
    Build {
        /// The `.ty` file to compile.
        file: PathBuf,
        /// Where to write the executable (defaults to the source file's stem).
        #[arg(short = 'o', long = "output", value_name = "OUT")]
        output: Option<PathBuf>,
        /// Optimisation level passed to clang.
        #[arg(
            short = 'O',
            value_name = "LEVEL",
            default_value_t = 2,
            value_parser = clap::value_parser!(u8).range(0..=3),
        )]
        opt_level: u8,
        /// Also write the generated LLVM IR next to the executable.
        #[arg(long)]
        emit_ir: bool,
        /// Keep the intermediate `.ll` file.
        #[arg(long)]
        keep_temps: bool,
    },

    /// Compile and immediately run a Typhoon source file.
    Run {
        /// The `.ty` file to compile and run.
        file: PathBuf,
        /// Optimisation level passed to clang.
        #[arg(
            short = 'O',
            value_name = "LEVEL",
            default_value_t = 2,
            value_parser = clap::value_parser!(u8).range(0..=3),
        )]
        opt_level: u8,
        /// Arguments forwarded to the compiled program, after `--`.
        #[arg(last = true, value_name = "ARGS")]
        args: Vec<String>,
    },

    /// Print the generated LLVM IR and stop (DESIGN section 6.3).
    EmitIr {
        /// The `.ty` file to compile.
        file: PathBuf,
        /// Where to write the `.ll` text (defaults to stdout).
        #[arg(short = 'o', long = "output", value_name = "OUT")]
        output: Option<PathBuf>,
    },

    /// Type-check a Typhoon source file without producing a binary.
    Check {
        /// The `.ty` file to check.
        file: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.command) {
        Ok(code) => code,
        Err(Error::Usage(message)) => {
            eprintln!("error: {message}");
            ExitCode::from(EXIT_USAGE)
        }
        Err(Error::Driver(err)) => {
            eprintln!("{err}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}

/// A CLI-level failure, carrying the exit code it should map to.
enum Error {
    /// Bad input that is the user's fault: exit 2.
    Usage(String),
    /// The compiler or linker rejected the program: exit 1.
    Driver(DriverError),
}

impl From<DriverError> for Error {
    fn from(err: DriverError) -> Self {
        Error::Driver(err)
    }
}

/// Reads a source file, reporting an unreadable path as a usage error.
fn read_source(file: &Path) -> Result<String, Error> {
    std::fs::read_to_string(file)
        .map_err(|err| Error::Usage(format!("cannot read `{}`: {err}", file.display())))
}

/// The name used in diagnostics for a source file.
fn display_name(file: &Path) -> String {
    file.to_string_lossy().into_owned()
}

/// Default output path for `build`: the source file's stem in the current directory.
fn default_output(file: &Path) -> PathBuf {
    PathBuf::from(file.file_stem().unwrap_or(file.as_os_str()))
}

fn run(cmd: Cmd) -> Result<ExitCode, Error> {
    match cmd {
        Cmd::Build {
            file,
            output,
            opt_level,
            emit_ir,
            keep_temps,
        } => {
            let source = read_source(&file)?;
            let output = output.unwrap_or_else(|| default_output(&file));
            let opts = BuildOptions {
                opt_level,
                emit_ir,
                keep_temps,
                output: Some(output.clone()),
            };
            let artifact = compile_source(&display_name(&file), &source, &opts)?;
            if let Some(ir) = artifact.ir() {
                let ir_path = output.with_extension("ll");
                std::fs::write(&ir_path, ir).map_err(DriverError::Io)?;
            }
            Ok(ExitCode::SUCCESS)
        }

        Cmd::Run {
            file,
            opt_level,
            args,
        } => {
            let source = read_source(&file)?;
            let opts = BuildOptions {
                opt_level,
                ..BuildOptions::default()
            };
            let artifact = compile_source(&display_name(&file), &source, &opts)?;
            // Inherit stdio so the program streams straight to the terminal.
            let status = Command::new(artifact.binary())
                .args(&args)
                .status()
                .map_err(DriverError::Io)?;
            // Propagate the program's own exit code; a signal death becomes 1.
            let code = status.code().unwrap_or(i32::from(EXIT_ERROR));
            Ok(ExitCode::from(u8::try_from(code).unwrap_or(EXIT_ERROR)))
        }

        Cmd::EmitIr { file, output } => {
            let source = read_source(&file)?;
            let ir = compile_to_ir(&display_name(&file), &source)?;
            match output {
                Some(path) => std::fs::write(&path, ir).map_err(DriverError::Io)?,
                None => {
                    let mut stdout = std::io::stdout().lock();
                    stdout.write_all(ir.as_bytes()).map_err(DriverError::Io)?;
                    stdout.flush().map_err(DriverError::Io)?;
                }
            }
            Ok(ExitCode::SUCCESS)
        }

        Cmd::Check { file } => {
            let source = read_source(&file)?;
            compile_to_ir(&display_name(&file), &source)?;
            Ok(ExitCode::SUCCESS)
        }
    }
}
