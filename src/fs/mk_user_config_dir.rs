use std::path::{Path, PathBuf};

use super::Result;
use super::user_config_dir::config_root;
use super::user_dir::{CONFIG_DIR_MODE, mk_user_dir_at};

/// Creates an application's configuration directory, including any missing parents, and returns its path.
///
/// The path is [`user_config_dir`](super::user_config_dir) computes, so the same validation applies and the same root:
/// `%LOCALAPPDATA%` on Windows, which does not roam with the user's profile. Directories are
/// created with mode `0o700` on Unix, since configuration often holds credentials. Succeeds silently if the directory already exists, and leaves an existing
/// directory's mode alone.
///
/// # Errors
///
/// Returns [`FsError::EmptyName`](super::FsError::EmptyName) if `name` is empty,
/// [`FsError::NoConfigDir`](super::FsError::NoConfigDir) if the platform has no configuration directory,
/// [`FsError::IllegalPath`](super::FsError::IllegalPath) if `name` or `sub_path` could escape it, and
/// [`FsError::Io`](super::FsError::Io) if the directory could not be created.
pub fn mk_user_config_dir(name: &str, sub_path: impl AsRef<Path>) -> Result<PathBuf> {
    mk_user_dir_at(&config_root()?, name, sub_path.as_ref(), CONFIG_DIR_MODE)
}
