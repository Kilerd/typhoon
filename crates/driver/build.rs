//! Build script for `typhoon-driver`.
//!
//! It compiles `crates/runtime` for `wasm32-unknown-unknown` and drops the
//! resulting module in `OUT_DIR`, where `src/wasm.rs` embeds it with
//! `include_bytes!`. That module is one half of every Typhoon wasm program
//! (DESIGN §6.2.1): the user module imports its memory and its `ty_*` exports.
//!
//! The nested build runs
//!
//! ```sh
//! cargo rustc -p typhoon-runtime --target wasm32-unknown-unknown \
//!     --profile wasm-release --crate-type cdylib --target-dir $OUT_DIR/wasm-rt
//! ```
//!
//! `cargo rustc --crate-type cdylib` rather than a second `crate-type` entry in
//! the runtime's manifest: a `cdylib` is what produces a wasm module with
//! exports, but adding it unconditionally would also make every *native* build
//! link a shared library against Boehm GC, which is not what the runtime is
//! for. The `--target-dir` of its own keeps the nested build from waiting on
//! the outer build's lock.
//!
//! # Environment
//!
//! | variable | effect |
//! |---|---|
//! | `TYPHOON_RUNTIME_WASM` | use this `.wasm` instead of building one |
//! | `TYPHOON_SKIP_WASM_RUNTIME` | skip the nested build entirely |
//!
//! If the wasm target is not installed the build still succeeds — the compiler
//! is perfectly usable without it — and `typhoon_driver::wasm::runtime_module`
//! reports an actionable error at run time instead.
//!
//! The script also declares the rerun triggers for the environment variables
//! `typhoon_driver::toolchain` reads, so that changing one invalidates the
//! build.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/loader.js");
    println!("cargo:rerun-if-changed=../runtime/src/lib.rs");
    println!("cargo:rerun-if-changed=../runtime/build.rs");
    println!("cargo:rerun-if-changed=../runtime/Cargo.toml");
    println!("cargo:rerun-if-env-changed=TYPHOON_RUNTIME_WASM");
    println!("cargo:rerun-if-env-changed=TYPHOON_SKIP_WASM_RUNTIME");
    println!("cargo:rerun-if-env-changed=TYPHOON_CLANG");
    println!("cargo:rerun-if-env-changed=TYPHOON_RUNTIME_LIB");
    println!("cargo:rerun-if-env-changed=TYPHOON_GC_LIB_DIR");
    // The `has_wasm_runtime` cfg is set below when the module was produced.
    println!("cargo::rustc-check-cfg=cfg(has_wasm_runtime)");

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let destination = out_dir.join("typhoon_rt.wasm");

    if std::env::var_os("TYPHOON_SKIP_WASM_RUNTIME").is_some() {
        println!("cargo:warning=TYPHOON_SKIP_WASM_RUNTIME is set: `--target wasm` will not work");
        return;
    }

    if let Some(explicit) = std::env::var_os("TYPHOON_RUNTIME_WASM") {
        let path = PathBuf::from(explicit);
        match std::fs::copy(&path, &destination) {
            Ok(_) => println!("cargo:rustc-cfg=has_wasm_runtime"),
            Err(err) => println!(
                "cargo:warning=TYPHOON_RUNTIME_WASM points at `{}`, which cannot be read: {err}",
                path.display()
            ),
        }
        return;
    }

    match build_runtime(&out_dir) {
        Ok(built) => match std::fs::copy(&built, &destination) {
            Ok(_) => println!("cargo:rustc-cfg=has_wasm_runtime"),
            Err(err) => println!(
                "cargo:warning=cannot copy the wasm runtime from `{}`: {err}",
                built.display()
            ),
        },
        Err(message) => {
            // Not fatal: everything except `--target wasm` works without it.
            println!("cargo:warning=the wasm runtime was not built ({message})");
        }
    }
}

/// Compiles `typhoon-runtime` to a wasm module, returning its path.
fn build_runtime(out_dir: &Path) -> Result<PathBuf, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let manifest = workspace_manifest();
    let target_dir = out_dir.join("wasm-rt");

    let mut command = Command::new(cargo);
    command
        .arg("rustc")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("-p")
        .arg("typhoon-runtime")
        .arg("--target")
        .arg("wasm32-unknown-unknown")
        .arg("--profile")
        .arg("wasm-release")
        .arg("--target-dir")
        .arg(&target_dir)
        .arg("--crate-type")
        .arg("cdylib");
    // A build script runs inside cargo, and several of the variables it
    // inherits would make the nested invocation build the wrong thing.
    for leaked in [
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS",
        "CARGO_BUILD_TARGET",
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_TARGET_DIR",
        "RUSTC_WORKSPACE_WRAPPER",
    ] {
        command.env_remove(leaked);
    }

    let output = command
        .output()
        .map_err(|err| format!("cannot run cargo: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let hint = if stderr.contains("wasm32-unknown-unknown") {
            "; run `rustup target add wasm32-unknown-unknown`"
        } else {
            ""
        };
        let last: Vec<&str> = stderr.lines().rev().take(5).collect();
        let mut summary: Vec<&str> = last;
        summary.reverse();
        return Err(format!("{}{hint}", summary.join(" / ")));
    }

    let built = target_dir
        .join("wasm32-unknown-unknown")
        .join("wasm-release")
        .join("typhoon_runtime.wasm");
    if !built.is_file() {
        return Err(format!("`{}` was not produced", built.display()));
    }
    Ok(built)
}

/// The workspace root manifest (`crates/driver/../../Cargo.toml`).
fn workspace_manifest() -> PathBuf {
    let manifest_dir = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"),
    );
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("crates/driver always has two ancestors")
        .join("Cargo.toml")
}
