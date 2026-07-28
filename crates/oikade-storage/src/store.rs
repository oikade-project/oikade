use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const ENGINE: &str = "redb";
const STATE_FORMAT: u32 = 1;
const SCHEMA_VERSION: u64 = 1;
const CACHE_SIZE_BYTES: usize = 16 * 1024 * 1024;

const MARKER_FILENAME: &str = "runtime-state.json";
const DATABASE_FILENAME: &str = "runtime-v1.redb";

const META: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");
const DEVICES: TableDefinition<&[u8], &[u8]> = TableDefinition::new("devices");
const PLUGINS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("plugins");
const DISCOVERY: TableDefinition<&[u8], &[u8]> = TableDefinition::new("discovery");
const SCHEMA_VERSION_KEY: &str = "schema_version";

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("state directory is required")]
    MissingStateDirectory,
    #[error("ambiguous runtime state: {0}")]
    AmbiguousState(String),
    #[error("unsupported runtime state engine {0:?}")]
    UnsupportedEngine(String),
    #[error("unsupported runtime state format {found}; this build supports {supported}")]
    UnsupportedFormat { found: u32, supported: u32 },
    #[error("runtime database schema {found} is newer than supported schema {supported}")]
    SchemaTooNew { found: u64, supported: u64 },
    #[error("invalid storage name: {0}")]
    InvalidName(String),
    #[error("storage key was not found")]
    NotFound,
    #[error("state path must not be a symbolic link: {0}")]
    SymbolicLink(PathBuf),
    #[error("state path is not a directory: {0}")]
    NotDirectory(PathBuf),
    #[error("state I/O: {0}")]
    Io(#[from] io::Error),
    #[error("state marker: {0}")]
    Marker(#[from] serde_json::Error),
    #[error("runtime database: {0}")]
    Database(#[from] redb::Error),
    #[error("runtime database open: {0}")]
    DatabaseOpen(#[from] redb::DatabaseError),
    #[error("runtime database transaction: {0}")]
    Transaction(#[from] redb::TransactionError),
    #[error("runtime database table: {0}")]
    Table(#[from] redb::TableError),
    #[error("runtime database storage: {0}")]
    Storage(#[from] redb::StorageError),
    #[error("runtime database commit: {0}")]
    Commit(#[from] redb::CommitError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Namespace {
    Devices,
    Plugins,
    Discovery,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StateMarker {
    engine: String,
    format: u32,
}

pub struct Storage {
    database: Arc<Database>,
    path: PathBuf,
}

impl Storage {
    pub fn open(state_directory: impl AsRef<Path>) -> Result<Self, StorageError> {
        let supplied = state_directory.as_ref();
        if supplied.as_os_str().is_empty() {
            return Err(StorageError::MissingStateDirectory);
        }
        let state_directory = absolute_path(supplied)?;
        prepare_state_directory(&state_directory)?;

        let marker_path = state_directory.join(MARKER_FILENAME);
        let database_path = state_directory.join(DATABASE_FILENAME);
        reject_symlink_if_present(&marker_path)?;
        reject_symlink_if_present(&database_path)?;

        let marker_exists = marker_path.try_exists()?;
        let database_exists = database_path.try_exists()?;

        let fresh = match (marker_exists, database_exists) {
            (false, false) => true,
            (true, true) => false,
            (true, false) => {
                return Err(StorageError::AmbiguousState(format!(
                    "{} exists but {} does not",
                    marker_path.display(),
                    database_path.display()
                )));
            }
            (false, true) => {
                return Err(StorageError::AmbiguousState(format!(
                    "{} exists without {}",
                    database_path.display(),
                    marker_path.display()
                )));
            }
        };

        let marker = if fresh {
            None
        } else {
            Some(read_marker(&marker_path)?)
        };
        if let Some(marker) = &marker {
            validate_marker(marker)?;
        }

        let database = Database::builder()
            .set_cache_size(CACHE_SIZE_BYTES)
            .create(&database_path)?;
        secure_file(&database_path)?;
        initialize_schema(&database)?;

        if fresh {
            write_marker(
                &marker_path,
                &StateMarker {
                    engine: ENGINE.to_owned(),
                    format: STATE_FORMAT,
                },
            )?;
        }

        Ok(Self {
            database: Arc::new(database),
            path: database_path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn bucket(&self, namespace: Namespace) -> Bucket {
        Bucket {
            database: Arc::clone(&self.database),
            namespace,
            scopes: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct Bucket {
    database: Arc<Database>,
    namespace: Namespace,
    scopes: Vec<String>,
}

impl Bucket {
    pub fn scope(&self, name: impl Into<String>) -> Result<Self, StorageError> {
        let name = name.into();
        validate_name(&name)?;
        let mut scopes = self.scopes.clone();
        scopes.push(name);
        Ok(Self {
            database: Arc::clone(&self.database),
            namespace: self.namespace,
            scopes,
        })
    }

    pub fn set(&self, key: &str, value: &[u8]) -> Result<(), StorageError> {
        validate_name(key)?;
        let encoded_key = encode_key(&self.scopes, key)?;
        let transaction = self.database.begin_write()?;
        {
            let mut table = transaction.open_table(table_for(self.namespace))?;
            table.insert(encoded_key.as_slice(), value)?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> Result<Vec<u8>, StorageError> {
        validate_name(key)?;
        let encoded_key = encode_key(&self.scopes, key)?;
        let transaction = self.database.begin_read()?;
        let table = transaction.open_table(table_for(self.namespace))?;
        let value = table
            .get(encoded_key.as_slice())?
            .ok_or(StorageError::NotFound)?;
        Ok(value.value().to_vec())
    }

    pub fn delete(&self, key: &str) -> Result<(), StorageError> {
        validate_name(key)?;
        let encoded_key = encode_key(&self.scopes, key)?;
        let transaction = self.database.begin_write()?;
        {
            let mut table = transaction.open_table(table_for(self.namespace))?;
            if table.remove(encoded_key.as_slice())?.is_none() {
                return Err(StorageError::NotFound);
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn keys_with_suffix(&self, suffix: &str) -> Result<Vec<String>, StorageError> {
        let transaction = self.database.begin_read()?;
        let table = transaction.open_table(table_for(self.namespace))?;
        let mut keys = Vec::new();
        for entry in table.iter()? {
            let (key, _) = entry?;
            let (scopes, name) = decode_key(key.value())?;
            if scopes == self.scopes && name.ends_with(suffix) {
                keys.push(name);
            }
        }
        keys.sort();
        Ok(keys)
    }
}

fn table_for(namespace: Namespace) -> TableDefinition<'static, &'static [u8], &'static [u8]> {
    match namespace {
        Namespace::Devices => DEVICES,
        Namespace::Plugins => PLUGINS,
        Namespace::Discovery => DISCOVERY,
    }
}

fn initialize_schema(database: &Database) -> Result<(), StorageError> {
    let transaction = database.begin_write()?;
    let current = {
        let mut meta = transaction.open_table(META)?;
        let current = meta
            .get(SCHEMA_VERSION_KEY)?
            .map(|value| decode_schema_version(value.value()))
            .transpose()?
            .unwrap_or(0);
        if current > SCHEMA_VERSION {
            return Err(StorageError::SchemaTooNew {
                found: current,
                supported: SCHEMA_VERSION,
            });
        }
        if current < 1 {
            transaction.open_table(DEVICES)?;
            transaction.open_table(PLUGINS)?;
            transaction.open_table(DISCOVERY)?;
            meta.insert(SCHEMA_VERSION_KEY, SCHEMA_VERSION.to_be_bytes().as_slice())?;
        }
        current
    };
    if current <= SCHEMA_VERSION {
        transaction.commit()?;
    }
    Ok(())
}

fn decode_schema_version(encoded: &[u8]) -> Result<u64, StorageError> {
    let encoded: [u8; 8] = encoded.try_into().map_err(|_| {
        StorageError::AmbiguousState("database schema version has invalid encoding".to_owned())
    })?;
    Ok(u64::from_be_bytes(encoded))
}

fn validate_marker(marker: &StateMarker) -> Result<(), StorageError> {
    if marker.engine != ENGINE {
        return Err(StorageError::UnsupportedEngine(marker.engine.clone()));
    }
    if marker.format != STATE_FORMAT {
        return Err(StorageError::UnsupportedFormat {
            found: marker.format,
            supported: STATE_FORMAT,
        });
    }
    Ok(())
}

fn read_marker(path: &Path) -> Result<StateMarker, StorageError> {
    let encoded = fs::read(path)?;
    let mut deserializer = serde_json::Deserializer::from_slice(&encoded);
    let marker = StateMarker::deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(marker)
}

fn write_marker(path: &Path, marker: &StateMarker) -> Result<(), StorageError> {
    let encoded = serde_json::to_vec(marker)?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    reject_symlink_if_present(&temporary)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    secure_open_file(&file)?;
    file.write_all(&encoded)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, path)?;
    sync_directory(path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "state marker has no parent")
    })?)?;
    Ok(())
}

fn absolute_path(path: &Path) -> Result<PathBuf, io::Error> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn prepare_state_directory(path: &Path) -> Result<(), StorageError> {
    reject_symlink_if_present(path)?;
    fs::create_dir_all(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(StorageError::SymbolicLink(path.to_path_buf()));
    }
    if !metadata.is_dir() {
        return Err(StorageError::NotDirectory(path.to_path_buf()));
    }
    secure_directory(path)?;
    Ok(())
}

fn reject_symlink_if_present(path: &Path) -> Result<(), StorageError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(StorageError::SymbolicLink(path.to_path_buf()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn secure_directory(path: &Path) -> Result<(), io::Error> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn secure_directory(_path: &Path) -> Result<(), io::Error> {
    Ok(())
}

#[cfg(unix)]
fn secure_file(path: &Path) -> Result<(), io::Error> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn secure_file(_path: &Path) -> Result<(), io::Error> {
    Ok(())
}

#[cfg(unix)]
fn secure_open_file(file: &fs::File) -> Result<(), io::Error> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn secure_open_file(_file: &fs::File) -> Result<(), io::Error> {
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), io::Error> {
    fs::File::open(path)?.sync_all()
}

fn validate_name(name: &str) -> Result<(), StorageError> {
    if name.is_empty() {
        return Err(StorageError::InvalidName("name is empty".to_owned()));
    }
    if name.contains(['/', '\\', '\0']) {
        return Err(StorageError::InvalidName(
            "name contains a path separator or NUL byte".to_owned(),
        ));
    }
    Ok(())
}

fn encode_key(scopes: &[String], key: &str) -> Result<Vec<u8>, StorageError> {
    let mut encoded = Vec::new();
    push_length(&mut encoded, scopes.len())?;
    for scope in scopes {
        push_bytes(&mut encoded, scope.as_bytes())?;
    }
    push_bytes(&mut encoded, key.as_bytes())?;
    Ok(encoded)
}

fn push_bytes(target: &mut Vec<u8>, value: &[u8]) -> Result<(), StorageError> {
    push_length(target, value.len())?;
    target.extend_from_slice(value);
    Ok(())
}

fn push_length(target: &mut Vec<u8>, value: usize) -> Result<(), StorageError> {
    let value = u32::try_from(value)
        .map_err(|_| StorageError::InvalidName("name is too large".to_owned()))?;
    target.extend_from_slice(&value.to_be_bytes());
    Ok(())
}

fn decode_key(mut encoded: &[u8]) -> Result<(Vec<String>, String), StorageError> {
    let scope_count = take_length(&mut encoded)?;
    let mut scopes = Vec::with_capacity(scope_count);
    for _ in 0..scope_count {
        scopes.push(take_string(&mut encoded)?);
    }
    let key = take_string(&mut encoded)?;
    if !encoded.is_empty() {
        return Err(StorageError::AmbiguousState(
            "database key has trailing bytes".to_owned(),
        ));
    }
    Ok((scopes, key))
}

fn take_length(encoded: &mut &[u8]) -> Result<usize, StorageError> {
    if encoded.len() < 4 {
        return Err(StorageError::AmbiguousState(
            "database key has a truncated length".to_owned(),
        ));
    }
    let (length, remaining) = encoded.split_at(4);
    *encoded = remaining;
    let length: [u8; 4] = length.try_into().map_err(|_| {
        StorageError::AmbiguousState("database key has an invalid length".to_owned())
    })?;
    Ok(u32::from_be_bytes(length) as usize)
}

fn take_string(encoded: &mut &[u8]) -> Result<String, StorageError> {
    let length = take_length(encoded)?;
    if encoded.len() < length {
        return Err(StorageError::AmbiguousState(
            "database key is truncated".to_owned(),
        ));
    }
    let (value, remaining) = encoded.split_at(length);
    *encoded = remaining;
    String::from_utf8(value.to_vec())
        .map_err(|_| StorageError::AmbiguousState("database key is not valid UTF-8".to_owned()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
