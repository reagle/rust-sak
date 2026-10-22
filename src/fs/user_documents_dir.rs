use std::path::{Path, PathBuf};

use super::user_dir::user_path;
use super::{FsError, Result};

/// Computes the path of an application's folder in the user's Documents **without creating anything**.
///
/// The result is `<platform documents dir>/<name>/<sub_path>` — `~/Documents/my-app/exports` on macOS,
/// `%USERPROFILE%\Documents\my-app\exports` on Windows, and the XDG documents directory on Linux. Pass `""` as
/// `sub_path` for the application's folder itself.
///
/// `name` and `sub_path` are validated exactly as [`user_config_dir`](super::user_config_dir) validates them, so a
/// `sub_path` of `../../.ssh` is refused rather than quietly resolved.
///
/// ```no_run
/// use rust_sak::fs::user_documents_dir;
///
/// let dir = user_documents_dir("my-app", "exports")?;
/// assert!(dir.ends_with("my-app/exports"));
/// # Ok::<(), rust_sak::fs::FsError>(())
/// ```
///
/// # Errors
///
/// Returns [`FsError::EmptyName`] if `name` is empty, [`FsError::NoDocumentsDir`] if the platform has no documents
/// directory, and [`FsError::IllegalPath`] if `name` or `sub_path` could escape it.
pub fn user_documents_dir(name: &str, sub_path: impl AsRef<Path>) -> Result<PathBuf> {
    user_path(&documents_root()?, name, sub_path.as_ref())
}

/// Returns the user's documents directory.
///
/// # Errors
///
/// Returns [`FsError::NoDocumentsDir`] if the platform has none — a Linux install without XDG user directories, or a
/// Windows profile without a Documents known folder.
pub(super) fn documents_root() -> Result<PathBuf> {
    dirs::document_dir().ok_or(FsError::NoDocumentsDir)
}
