use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use thiserror::Error;

pub const ADAPTER_STATE_MARKER: &str = ".oikade-adapter-state";
static RESET_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Error)]
pub enum StateError {
    #[error("adapter state directory is required")]
    Missing,
    #[error("adapter state I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("adapter state directory must be a private real directory")]
    UnsafeDirectory,
    #[error("adapter state marker is invalid or belongs to another instance")]
    InvalidMarker,
}

pub fn ensure_state_directory(
    directory: impl AsRef<Path>,
    instance_id: &str,
) -> Result<PathBuf, StateError> {
    let directory = directory.as_ref();
    if directory.as_os_str().is_empty() {
        return Err(StateError::Missing);
    }
    if !directory.exists() {
        if let Some(parent) = directory.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    let metadata = fs::symlink_metadata(directory)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(StateError::UnsafeDirectory);
    }
    let directory = directory.canonicalize()?;
    ensure_marker(&directory, instance_id)?;
    Ok(directory)
}

pub fn reset_state_directory(directory: &Path, instance_id: &str) -> Result<(), StateError> {
    let verified = ensure_state_directory(directory, instance_id)?;
    let directory = verified.as_path();
    let parent = directory.parent().ok_or(StateError::UnsafeDirectory)?;
    let base = directory
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(StateError::UnsafeDirectory)?;
    let quarantine = create_quarantine(parent, base)?;
    let backup = quarantine.join("state");
    if let Err(error) = fs::rename(directory, &backup) {
        let _ = fs::remove_dir(&quarantine);
        return Err(error.into());
    }
    let rollback = |cause: std::io::Error| -> StateError {
        let _ = fs::remove_dir_all(directory);
        let _ = fs::rename(&backup, directory);
        let _ = fs::remove_dir(&quarantine);
        StateError::Io(cause)
    };
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    if let Err(error) = builder.create(directory) {
        return Err(rollback(error));
    }
    if let Err(error) = ensure_marker(directory, instance_id) {
        let cause = match error {
            StateError::Io(error) => error,
            other => std::io::Error::other(other.to_string()),
        };
        return Err(rollback(cause));
    }
    let metadata = fs::symlink_metadata(&quarantine)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(StateError::UnsafeDirectory);
    }
    fs::remove_dir_all(quarantine)?;
    Ok(())
}

fn create_quarantine(parent: &Path, base: &str) -> Result<PathBuf, StateError> {
    for _ in 0..128 {
        let sequence = RESET_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".{base}.reset-{}-{sequence}", std::process::id()));
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(StateError::Io(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate adapter state quarantine",
    )))
}

fn ensure_marker(directory: &Path, instance_id: &str) -> Result<(), StateError> {
    let marker = directory.join(ADAPTER_STATE_MARKER);
    let expected = format!("{instance_id}\n");
    match fs::symlink_metadata(&marker) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(marker)?;
            file.write_all(expected.as_bytes())?;
            file.sync_all()?;
            Ok(())
        }
        Err(error) => Err(error.into()),
        Ok(metadata) => {
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.permissions().mode() & 0o077 != 0
                || metadata.len() > 256
            {
                return Err(StateError::InvalidMarker);
            }
            if fs::read_to_string(marker)? != expected {
                return Err(StateError::InvalidMarker);
            }
            Ok(())
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn marker_prevents_cross_instance_reuse() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("adapter");
        fs::create_dir(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        ensure_state_directory(&state, "matter").unwrap();
        assert!(ensure_state_directory(&state, "other").is_err());
    }

    #[test]
    fn reset_replaces_only_the_marked_private_directory() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("adapter");
        ensure_state_directory(&state, "matter").unwrap();
        fs::write(state.join("sentinel"), "remove me").unwrap();
        fs::write(root.path().join("keep"), "keep me").unwrap();

        reset_state_directory(&state, "matter").unwrap();

        assert!(!state.join("sentinel").exists());
        assert_eq!(
            fs::read_to_string(root.path().join("keep")).unwrap(),
            "keep me"
        );
        assert_eq!(
            fs::read_to_string(state.join(ADAPTER_STATE_MARKER)).unwrap(),
            "matter\n"
        );
    }
}
