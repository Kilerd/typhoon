//! Build script for `typhoon-runtime`.
//!
//! The runtime is shipped as a `staticlib`: Boehm GC is *not* linked into the
//! archive, it is linked by `clang` at the final link of a Typhoon binary (see
//! `typhoon-driver`). This script therefore only does two things:
//!
//! 1. it records `-lgc` (plus a search path) as a native static library, so
//!    that Rust's own test binary for this crate — which does a real link and
//!    must resolve `GC_init` / `GC_malloc` — builds and runs;
//! 2. it makes the emitted `--print native-static-libs` list self-describing.
//!
//! `TYPHOON_GC_LIB_DIR` overrides the search path for Boehm GC.
//!
//! The wasm build (`--target wasm32-unknown-unknown`, DESIGN §6.2.1) has no
//! GC at all — it allocates from Rust's global allocator and never frees — so
//! none of this applies there and `-lgc` must *not* be recorded: a native
//! archive is not a wasm object and `rust-lld` would complain about every
//! member of it.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-env-changed=TYPHOON_GC_LIB_DIR");

    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        return;
    }

    if let Some(dir) = gc_lib_dir() {
        println!("cargo:rustc-link-search=native={dir}");
    }
    println!("cargo:rustc-link-lib=gc");
}

/// Best-effort location of the directory holding `libgc`.
fn gc_lib_dir() -> Option<String> {
    if let Ok(dir) = std::env::var("TYPHOON_GC_LIB_DIR")
        && !dir.is_empty()
    {
        return Some(dir);
    }
    if let Ok(out) = Command::new("brew").args(["--prefix", "bdw-gc"]).output()
        && out.status.success()
    {
        let prefix = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !prefix.is_empty() {
            let lib = format!("{prefix}/lib");
            if std::path::Path::new(&lib).is_dir() {
                return Some(lib);
            }
        }
    }
    for candidate in ["/usr/local/lib", "/usr/lib/x86_64-linux-gnu", "/usr/lib"] {
        if std::path::Path::new(candidate).join("libgc.a").exists()
            || std::path::Path::new(candidate).join("libgc.so").exists()
        {
            return Some(candidate.to_string());
        }
    }
    None
}
