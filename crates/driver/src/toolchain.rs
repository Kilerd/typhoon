//! Locating the external pieces of the toolchain.
//!
//! Every lookup can be overridden with an environment variable, which is what
//! CI and packagers use:
//!
//! | variable | overrides |
//! |---|---|
//! | `TYPHOON_CLANG` | the `clang` used to optimise and link `.ll` files |
//! | `TYPHOON_RUNTIME_LIB` | the path of `libtyphoon_runtime.a` |
//! | `TYPHOON_GC_LIB_DIR` | the directory holding Boehm GC (`libgc`) |

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use crate::DriverError;

/// File name of the Typhoon runtime archive produced by `cargo build`.
pub const RUNTIME_LIB_NAME: &str = "libtyphoon_runtime.a";

/// Reads an environment variable, treating an empty value as unset.
fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// The `clang` executable used for optimisation and linking.
///
/// `TYPHOON_CLANG` wins; otherwise `clang` is resolved through `PATH` by the
/// operating system when the process is spawned.
pub fn clang() -> PathBuf {
    env_var("TYPHOON_CLANG").map_or_else(|| PathBuf::from("clang"), PathBuf::from)
}

/// Locates `libtyphoon_runtime.a`.
///
/// Search order:
///
/// 1. `TYPHOON_RUNTIME_LIB` (must exist, otherwise an error is reported);
/// 2. next to the running executable — `target/debug/typhoon`;
/// 3. its parent directory — this is what makes integration-test binaries in
///    `target/debug/deps/` find the archive in `target/debug/`.
pub fn runtime_lib() -> Result<PathBuf, DriverError> {
    resolve_runtime_lib(
        env_var("TYPHOON_RUNTIME_LIB").as_deref(),
        std::env::current_exe().ok(),
    )
}

/// The pure core of [`runtime_lib`], parameterised over the environment so it
/// can be tested without mutating process-global state.
pub(crate) fn resolve_runtime_lib(
    explicit: Option<&str>,
    current_exe: Option<PathBuf>,
) -> Result<PathBuf, DriverError> {
    if let Some(explicit) = explicit {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Ok(path);
        }
        return Err(DriverError::Toolchain(format!(
            "TYPHOON_RUNTIME_LIB points at `{}`, which does not exist",
            path.display()
        )));
    }

    let mut searched = Vec::new();
    if let Some(exe) = current_exe {
        let dir = exe.parent().map(Path::to_path_buf);
        let parent = dir.as_deref().and_then(Path::parent).map(Path::to_path_buf);
        for candidate in [dir, parent].into_iter().flatten() {
            let lib = candidate.join(RUNTIME_LIB_NAME);
            if lib.is_file() {
                return Ok(lib);
            }
            searched.push(lib);
        }
    }

    Err(DriverError::Toolchain(format!(
        "cannot find the Typhoon runtime `{RUNTIME_LIB_NAME}`\n\
         searched: {}\n\
         help: run `cargo build -p typhoon-runtime`, or set TYPHOON_RUNTIME_LIB to the archive",
        searched
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

/// Directories that are searched for Boehm GC when `TYPHOON_GC_LIB_DIR` is unset.
const GC_FALLBACK_DIRS: &[&str] = &[
    "/opt/homebrew/lib",
    "/usr/local/lib",
    "/usr/lib/x86_64-linux-gnu",
    "/usr/lib/aarch64-linux-gnu",
    "/usr/lib64",
    "/usr/lib",
];

fn has_libgc(dir: &Path) -> bool {
    ["libgc.a", "libgc.so", "libgc.dylib"]
        .iter()
        .any(|name| dir.join(name).exists())
}

/// Cached result of `brew --prefix bdw-gc`; `brew` is only ever run once.
fn brew_gc_lib_dir() -> Option<&'static Path> {
    static CACHE: OnceLock<Option<PathBuf>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let out = Command::new("brew")
                .args(["--prefix", "bdw-gc"])
                .output()
                .ok()?;
            if !out.status.success() {
                return None;
            }
            let prefix = String::from_utf8(out.stdout).ok()?;
            let dir = PathBuf::from(prefix.trim()).join("lib");
            has_libgc(&dir).then_some(dir)
        })
        .as_deref()
}

/// Locates the directory containing Boehm GC (`libgc`).
///
/// `TYPHOON_GC_LIB_DIR`, then `brew --prefix bdw-gc`, then the usual system
/// library directories.
pub fn gc_lib_dir() -> Result<PathBuf, DriverError> {
    resolve_gc_lib_dir(env_var("TYPHOON_GC_LIB_DIR").as_deref(), brew_gc_lib_dir())
}

/// The pure core of [`gc_lib_dir`], parameterised over the environment so it
/// can be tested without mutating process-global state.
pub(crate) fn resolve_gc_lib_dir(
    explicit: Option<&str>,
    brew: Option<&Path>,
) -> Result<PathBuf, DriverError> {
    if let Some(explicit) = explicit {
        let dir = PathBuf::from(explicit);
        if dir.is_dir() {
            return Ok(dir);
        }
        return Err(DriverError::Toolchain(format!(
            "TYPHOON_GC_LIB_DIR points at `{}`, which is not a directory",
            dir.display()
        )));
    }

    if let Some(dir) = brew {
        return Ok(dir.to_path_buf());
    }

    for candidate in GC_FALLBACK_DIRS {
        let dir = Path::new(candidate);
        if has_libgc(dir) {
            return Ok(dir.to_path_buf());
        }
    }

    Err(DriverError::Toolchain(format!(
        "cannot find the Boehm GC library `libgc`\n\
         searched: {}\n\
         help: install it (`brew install bdw-gc` / `apt-get install libgc-dev`), \
         or set TYPHOON_GC_LIB_DIR to the directory containing libgc",
        GC_FALLBACK_DIRS.join(", ")
    )))
}

/// The platform libraries that `libtyphoon_runtime.a` needs at the final link.
///
/// Obtained from `cargo rustc -p typhoon-runtime --lib -- --print
/// native-static-libs`; `-lgc` is reported there too but is passed separately
/// by [`crate::link_ll`], together with its search path.
pub fn native_static_libs() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        // native-static-libs: -lSystem -lc -lm -lgc
        &["-lSystem", "-lc", "-lm"]
    } else if cfg!(target_os = "linux") {
        &[
            "-lgcc_s",
            "-lutil",
            "-lrt",
            "-lpthread",
            "-lm",
            "-ldl",
            "-lc",
        ]
    } else {
        &[]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_runtime_lib_must_exist() {
        let err = resolve_runtime_lib(Some("/definitely/not/here.a"), None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("/definitely/not/here.a"), "{msg}");
        assert!(msg.contains("TYPHOON_RUNTIME_LIB"), "{msg}");
    }

    #[test]
    fn explicit_runtime_lib_is_used_verbatim() {
        let dir = crate::temp::TempDir::new("typhoon-toolchain").unwrap();
        let lib = dir.path().join(RUNTIME_LIB_NAME);
        std::fs::write(&lib, b"!<arch>\n").unwrap();
        let found = resolve_runtime_lib(Some(lib.to_str().unwrap()), None).unwrap();
        assert_eq!(found, lib);
    }

    #[test]
    fn runtime_lib_is_found_next_to_the_executable() {
        let dir = crate::temp::TempDir::new("typhoon-toolchain").unwrap();
        let lib = dir.path().join(RUNTIME_LIB_NAME);
        std::fs::write(&lib, b"!<arch>\n").unwrap();
        let fake_exe = dir.path().join("typhoon");
        assert_eq!(resolve_runtime_lib(None, Some(fake_exe)).unwrap(), lib);
    }

    #[test]
    fn runtime_lib_is_found_one_directory_up() {
        // This is the `target/debug/deps/<test-binary>` layout.
        let dir = crate::temp::TempDir::new("typhoon-toolchain").unwrap();
        let lib = dir.path().join(RUNTIME_LIB_NAME);
        std::fs::write(&lib, b"!<arch>\n").unwrap();
        let deps = dir.path().join("deps");
        std::fs::create_dir_all(&deps).unwrap();
        assert_eq!(
            resolve_runtime_lib(None, Some(deps.join("golden-abc123"))).unwrap(),
            lib
        );
    }

    #[test]
    fn missing_runtime_lib_message_is_actionable() {
        let dir = crate::temp::TempDir::new("typhoon-toolchain").unwrap();
        let err = resolve_runtime_lib(None, Some(dir.path().join("deps/t"))).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("cargo build -p typhoon-runtime"), "{msg}");
        assert!(msg.contains("TYPHOON_RUNTIME_LIB"), "{msg}");
    }

    #[test]
    fn explicit_gc_dir_must_be_a_directory() {
        let err = resolve_gc_lib_dir(Some("/definitely/not/a/dir"), None).unwrap_err();
        assert!(err.to_string().contains("TYPHOON_GC_LIB_DIR"), "{err}");
    }

    #[test]
    fn explicit_gc_dir_wins_over_brew() {
        let dir = crate::temp::TempDir::new("typhoon-toolchain").unwrap();
        let brew = Path::new("/opt/homebrew/opt/bdw-gc/lib");
        let found = resolve_gc_lib_dir(Some(dir.path().to_str().unwrap()), Some(brew)).unwrap();
        assert_eq!(found, dir.path());
    }

    #[test]
    fn brew_gc_dir_is_used_when_no_override() {
        let brew = Path::new("/some/brew/prefix/lib");
        assert_eq!(resolve_gc_lib_dir(None, Some(brew)).unwrap(), brew);
    }

    #[test]
    fn gc_lib_dir_is_discoverable_on_this_machine() {
        // The build already links Boehm GC, so it must be findable.
        let dir = gc_lib_dir().expect("bdw-gc should be installed");
        assert!(has_libgc(&dir), "{} has no libgc", dir.display());
    }

    #[test]
    fn clang_defaults_to_path_lookup() {
        // Without TYPHOON_CLANG this is a bare name resolved through PATH.
        if std::env::var_os("TYPHOON_CLANG").is_none() {
            assert_eq!(clang(), Path::new("clang"));
        }
    }
}
