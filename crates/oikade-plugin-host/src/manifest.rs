use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use oikade_config::parse_yaml_document;
use oikade_core::validate_identifier;
use oikade_plugin_api::VERSION;
use serde::Deserialize;
use thiserror::Error;

pub const MANIFEST_FILENAME: &str = "oikade-plugin.yaml";
const MAX_MANIFEST_SIZE: u64 = 64 << 10;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub api_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    pub executable: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("plugin artifact path must be non-empty")]
    EmptyArtifact,
    #[error("inspect plugin artifact: {0}")]
    Io(#[from] std::io::Error),
    #[error("plugin manifest exceeds {MAX_MANIFEST_SIZE} bytes")]
    TooLarge,
    #[error("invalid plugin manifest: {0}")]
    Invalid(String),
}

pub fn load_manifest(artifact: impl AsRef<Path>) -> Result<(Manifest, PathBuf), ManifestError> {
    let artifact = artifact.as_ref();
    if artifact.as_os_str().is_empty() {
        return Err(ManifestError::EmptyArtifact);
    }
    let artifact = artifact.canonicalize()?;
    if !artifact.is_dir() {
        return Err(ManifestError::Invalid(
            "artifact is not a directory".to_owned(),
        ));
    }
    let path = artifact.join(MANIFEST_FILENAME);
    let metadata = fs::metadata(&path)?;
    if metadata.len() > MAX_MANIFEST_SIZE {
        return Err(ManifestError::TooLarge);
    }
    let encoded = fs::read(&path)?;
    let manifest: Manifest = serde_json::from_value(
        parse_yaml_document(&encoded).map_err(|error| ManifestError::Invalid(error.to_string()))?,
    )
    .map_err(|error| ManifestError::Invalid(error.to_string()))?;
    validate(&manifest)?;

    if manifest.executable.is_absolute()
        || manifest.executable.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::RootDir
            )
        })
    {
        return Err(ManifestError::Invalid(
            "executable must stay within the artifact directory".to_owned(),
        ));
    }
    let executable = artifact.join(&manifest.executable);
    let executable_metadata = fs::metadata(&executable)?;
    if !executable_metadata.is_file() {
        return Err(ManifestError::Invalid(
            "plugin executable is not a regular file".to_owned(),
        ));
    }
    if executable_metadata.permissions().mode() & 0o111 == 0 {
        return Err(ManifestError::Invalid(
            "plugin executable is not executable".to_owned(),
        ));
    }
    let executable = executable.canonicalize()?;
    if !executable.starts_with(&artifact) {
        return Err(ManifestError::Invalid(
            "executable symlink must stay within the artifact directory".to_owned(),
        ));
    }
    Ok((manifest, executable))
}

fn validate(manifest: &Manifest) -> Result<(), ManifestError> {
    if manifest.api_version != VERSION {
        return Err(ManifestError::Invalid(format!(
            "api_version must be {VERSION}, got {}",
            manifest.api_version
        )));
    }
    validate_identifier(&manifest.id)
        .map_err(|error| ManifestError::Invalid(format!("id: {error}")))?;
    for (name, value) in [
        ("name", manifest.name.as_str()),
        ("version", manifest.version.as_str()),
    ] {
        if value.is_empty() || value != value.trim() {
            return Err(ManifestError::Invalid(format!(
                "{name} must be non-empty without surrounding whitespace"
            )));
        }
    }
    if manifest.executable.as_os_str().is_empty() {
        return Err(ManifestError::Invalid("executable is required".to_owned()));
    }
    if manifest.args.iter().any(|arg| arg.contains('\0')) {
        return Err(ManifestError::Invalid(
            "arguments must not contain NUL bytes".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    #[test]
    fn loads_strict_local_manifest() {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("plugin");
        fs::write(&executable, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(
            root.path().join(MANIFEST_FILENAME),
            "api_version: 1\nid: example.plugin\nname: Example\nversion: 0.1.0\nexecutable: plugin\n",
        )
        .unwrap();
        let (manifest, resolved) = load_manifest(root.path()).unwrap();
        assert_eq!(manifest.id, "example.plugin");
        assert_eq!(resolved, executable.canonicalize().unwrap());
    }

    #[test]
    fn rejects_escape_and_unknown_fields() {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join(MANIFEST_FILENAME),
            "api_version: 1\nid: example.plugin\nname: Example\nversion: 1\nexecutable: ../plugin\nsurprise: true\n",
        )
        .unwrap();
        assert!(load_manifest(root.path()).is_err());
    }

    #[test]
    fn rejects_executable_symlink_escape() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        fs::set_permissions(outside.path(), fs::Permissions::from_mode(0o700)).unwrap();
        symlink(outside.path(), root.path().join("plugin")).unwrap();
        fs::write(
            root.path().join(MANIFEST_FILENAME),
            "api_version: 1\nid: example.plugin\nname: Example\nversion: 1.0.0\nexecutable: plugin\n",
        )
        .unwrap();
        assert!(load_manifest(root.path()).is_err());
    }
}
