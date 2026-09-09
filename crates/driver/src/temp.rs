//! A dependency-free scratch directory with RAII cleanup.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A uniquely named directory under the system temp directory.
///
/// The directory is removed when the value is dropped, unless [`TempDir::keep`]
/// has been called (which is what `--keep-temps` does).
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
    keep: bool,
}

impl TempDir {
    /// Creates a new scratch directory whose name starts with `prefix`.
    pub fn new(prefix: &str) -> std::io::Result<Self> {
        let unique = format!(
            "{prefix}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(unique);
        // A leftover directory from a crashed run would silently share state.
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path)?;
        Ok(Self { path, keep: false })
    }

    /// The directory's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Leaves the directory on disk when this value is dropped.
    pub fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
