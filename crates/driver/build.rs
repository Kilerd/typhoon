//! Build script for `typhoon-driver`.
//!
//! The driver links Typhoon programs against `libtyphoon_runtime.a`, which is a
//! `staticlib` and therefore cannot be expressed as a normal Cargo dependency.
//! This script exists only to declare the rerun triggers for the environment
//! variables that `typhoon_driver::toolchain` reads, so that changing them
//! invalidates the build.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=TYPHOON_CLANG");
    println!("cargo:rerun-if-env-changed=TYPHOON_RUNTIME_LIB");
    println!("cargo:rerun-if-env-changed=TYPHOON_GC_LIB_DIR");
}
