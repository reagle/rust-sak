use std::path::{Path, PathBuf};

use super::user_dir::user_path;
use super::{FsError, Result};

/// Computes the path of an application's configuration directory **without creating anything**.
///
/// The result is `<platform config dir>/<name>/<sub_path>` — `~/.config/my-app/themes` on Linux,
/// `~/Library/Application Support/my-app/themes` on macOS, `%APPDATA%\my-app\themes` on Windows. Pass `""` as
/// `sub_path` for the application's directory itself.
///
/// Both `name` and `sub_path` are validated the same way archive entries are, so a `sub_path` of
/// `../../.ssh/authorized_keys` is refused rather than quietly resolved — the trap in composing this path from
/// untrusted input.
///
/// ```
/// use rust_sak::fs::user_config_dir;
///
/// let dir = user_config_dir("my-app", "themes")?;
/// assert!(dir.ends_with("my-app/themes"));
///
/// assert!(user_config_dir("my-app", "../../escape").is_err());
/// # Ok::<(), rust_sak::fs::FsError>(())
/// ```
///
/// # Errors
///
/// Returns [`FsError::EmptyName`] if `name` is empty, [`FsError::NoConfigDir`] if the platform has no configuration
/// directory, and [`FsError::IllegalPath`] if `name` or `sub_path` could escape it.
pub fn user_config_dir(name: &str, sub_path: impl AsRef<Path>) -> Result<PathBuf> {
    user_path(&config_root()?, name, sub_path.as_ref())
}

/// Returns the platform's configuration directory.
///
/// # Errors
///
/// Returns [`FsError::NoConfigDir`] if the platform has none, or the environment does not say where it is.
pub(super) fn config_root() -> Result<PathBuf> {
    dirs::config_dir().ok_or(FsError::NoConfigDir)
}
