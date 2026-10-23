use std::fs::DirBuilder;
#[cfg(unix)]
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use super::safe_path::validate_entry_name;
use super::{FsError, Result};

/// The mode new per-application directories are created with: readable and traversable by all, writable by the owner.
#[cfg(unix)]
const DIR_MODE: u32 = 0o755;

/// Joins `root`, `name` and `sub_path` after validating the latter two.
///
/// This is the whole of [`user_config_dir`](super::user_config_dir) and
/// [`user_documents_dir`](super::user_documents_dir) except for looking up the platform directory, which is what lets
/// tests exercise the validation against a temporary root instead of the real `$HOME`.
///
/// # Errors
///
/// Returns [`FsError::EmptyName`] if `name` is empty, or [`FsError::IllegalPath`] if either part is not a contained
/// relative path.
pub(super) fn user_path(root: &Path, name: &str, sub_path: &Path) -> Result<PathBuf> {
    // An empty name, or one of only `.` and separators, passes validation but names no directory at all, which would
    // silently hand back the bare root.
    let name_components = validate_entry_name(name)?;
    if name_components.is_empty() {
        return Err(FsError::EmptyName);
    }

    let mut path = root.to_path_buf();
    for component in name_components {
        path.push(component);
    }

    let sub_path = sub_path.to_str().ok_or_else(|| FsError::IllegalPath {
        path: sub_path.to_string_lossy().into_owned(),
        reason: "is not valid UTF-8".to_string(),
    })?;
    for component in validate_entry_name(sub_path)? {
        path.push(component);
    }

    Ok(path)
}

/// Creates the directory [`user_path`] computes under an explicit root, including any missing parents.
///
/// The testable half of [`mk_user_config_dir`](super::mk_user_config_dir) and
/// [`mk_user_documents_dir`](super::mk_user_documents_dir), so the suite can create directories under a temporary
/// root rather than the real one. Directories are created with mode `0o755` on Unix; an existing one is left alone.
///
/// # Errors
///
/// As [`user_path`], plus [`FsError::Io`] if the directory could not be created.
pub(super) fn mk_user_dir_at(root: &Path, name: &str, sub_path: &Path) -> Result<PathBuf> {
    let path = user_path(root, name, sub_path)?;

    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(DIR_MODE);
    builder.create(&path)?;

    Ok(path)
}
