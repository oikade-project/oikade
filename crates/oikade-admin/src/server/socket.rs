use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::Path;

use tokio::net::UnixStream;
use tokio::time::timeout;

use super::{STALE_DIAL_TIMEOUT, ServerError, SocketIdentity};

pub(super) fn prepare_parent(path: &Path) -> Result<(), ServerError> {
    let parent = path.parent().ok_or(ServerError::MissingSocket)?;
    fs::create_dir_all(parent)?;
    let metadata = fs::symlink_metadata(parent)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ServerError::Io(std::io::Error::other(
            "admin socket parent must be a real directory",
        )));
    }
    Ok(())
}

pub(super) async fn remove_stale_socket(path: &Path) -> Result<(), ServerError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_socket() {
        return Err(ServerError::Io(std::io::Error::other(
            "admin socket path exists and is not a Unix socket",
        )));
    }
    let expected = SocketIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    };
    match timeout(STALE_DIAL_TIMEOUT, UnixStream::connect(path)).await {
        Ok(Ok(_)) => {
            return Err(ServerError::Io(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                "admin socket is already accepting connections",
            )));
        }
        Err(_) => {
            return Err(ServerError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "cannot confirm that admin socket is stale",
            )));
        }
        Ok(Err(error)) if error.raw_os_error() == Some(nix::libc::ECONNREFUSED) => {}
        Ok(Err(error)) => return Err(error.into()),
    }
    if socket_identity(path)? != expected {
        return Err(ServerError::Io(std::io::Error::other(
            "admin socket changed during stale check",
        )));
    }
    fs::remove_file(path)?;
    Ok(())
}

pub(super) fn socket_identity(path: &Path) -> Result<SocketIdentity, ServerError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket() {
        return Err(ServerError::Io(std::io::Error::other(
            "admin socket path is not a Unix socket",
        )));
    }
    Ok(SocketIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

pub(super) fn remove_socket_if_same(
    path: &Path,
    expected: SocketIdentity,
) -> Result<(), ServerError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(metadata) => {
            if !metadata.file_type().is_socket()
                || metadata.dev() != expected.device
                || metadata.ino() != expected.inode
            {
                return Err(ServerError::Io(std::io::Error::other(
                    "refusing to remove replaced admin socket",
                )));
            }
            fs::remove_file(path)?;
            Ok(())
        }
    }
}
