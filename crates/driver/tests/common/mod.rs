//! Shared helpers for the driver's integration tests and the golden harness.
#![allow(dead_code)]

pub mod wasm;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// The workspace root (`crates/driver/../..`).
pub fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/driver always has two ancestors")
}

/// Directory holding the hand-written `.ll` fixtures.
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Reads a `.ll` fixture by file name.
pub fn fixture(name: &str) -> String {
    let path = fixtures_dir().join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("cannot read fixture {}: {err}", path.display()))
}

/// Makes sure `libtyphoon_runtime.a` exists and is up to date.
///
/// The runtime is a `staticlib`, so it cannot be a Cargo dependency of this
/// crate and Cargo will neither build nor rebuild it for us. We therefore run
/// `cargo build -p typhoon-runtime` once per test binary (a no-op when the
/// archive is already current). `TYPHOON_RUNTIME_LIB` short-circuits this.
pub fn ensure_runtime_lib() -> &'static Path {
    static LIB: OnceLock<PathBuf> = OnceLock::new();
    LIB.get_or_init(|| {
        if std::env::var_os("TYPHOON_RUNTIME_LIB").is_none() {
            let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
            let out = Command::new(cargo)
                .arg("build")
                .arg("-p")
                .arg("typhoon-runtime")
                .arg("--manifest-path")
                .arg(repo_root().join("Cargo.toml"))
                .output();
            match out {
                Ok(out) if !out.status.success() => panic!(
                    "`cargo build -p typhoon-runtime` failed:\n{}",
                    String::from_utf8_lossy(&out.stderr)
                ),
                // No cargo on PATH: fall back to whatever archive is present.
                Ok(_) | Err(_) => {}
            }
        }
        typhoon_driver::toolchain::runtime_lib().unwrap_or_else(|err| panic!("{err}"))
    })
}

static SCRATCH_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A per-test scratch directory, removed when dropped.
pub struct Scratch {
    path: PathBuf,
}

impl Scratch {
    pub fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "typhoon-it-{label}-{}-{}",
            std::process::id(),
            SCRATCH_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("cannot create scratch directory");
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Renders a minimal unified diff between two blocks of text.
///
/// Common leading and trailing lines are collapsed; the differing middle is
/// printed with `-` for expected and `+` for actual lines.
pub fn unified_diff(expected: &str, actual: &str) -> String {
    let exp: Vec<&str> = expected.lines().collect();
    let act: Vec<&str> = actual.lines().collect();

    let prefix = exp.iter().zip(&act).take_while(|(a, b)| a == b).count();
    let max_suffix = exp.len().min(act.len()) - prefix;
    let suffix = (0..max_suffix)
        .take_while(|i| exp[exp.len() - 1 - i] == act[act.len() - 1 - i])
        .count();

    let mut out = String::new();
    out.push_str("--- expected\n+++ actual\n");
    out.push_str(&format!(
        "@@ -{},{} +{},{} @@\n",
        prefix + 1,
        exp.len() - prefix - suffix,
        prefix + 1,
        act.len() - prefix - suffix
    ));
    let context = 2;
    for line in &exp[prefix.saturating_sub(context)..prefix] {
        out.push_str(&format!(" {line}\n"));
    }
    for line in &exp[prefix..exp.len() - suffix] {
        out.push_str(&format!("-{line}\n"));
    }
    for line in &act[prefix..act.len() - suffix] {
        out.push_str(&format!("+{line}\n"));
    }
    for line in &exp[exp.len() - suffix..(exp.len() - suffix + context).min(exp.len())] {
        out.push_str(&format!(" {line}\n"));
    }
    out
}
