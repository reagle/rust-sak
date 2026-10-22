use std::path::{Path, PathBuf};

use super::Result;
use super::user_dir::mk_user_dir_at;
use super::user_documents_dir::documents_root;

/// Creates an application's folder in the user's Documents, including any missing parents, and returns its path.
///
/// The path is [`user_documents_dir`](super::user_documents_dir) computes, so the same validation applies. Directories
/// are created with mode `0o755` on Unix. Succeeds silently if the directory already exists, and leaves an existing
/// directory's mode alone.
///
/// # Errors
///
/// Returns [`FsError::EmptyName`](super::FsError::EmptyName) if `name` is empty,
/// [`FsError::NoDocumentsDir`](super::FsError::NoDocumentsDir) if the platform has no documents directory,
/// [`FsError::IllegalPath`](super::FsError::IllegalPath) if `name` or `sub_path` could escape it, and
/// [`FsError::Io`](super::FsError::Io) if the directory could not be created.
pub fn mk_user_documents_dir(name: &str, sub_path: impl AsRef<Path>) -> Result<PathBuf> {
    mk_user_dir_at(&documents_root()?, name, sub_path.as_ref())
}
