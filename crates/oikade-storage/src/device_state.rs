use std::collections::BTreeMap;

use async_trait::async_trait;
use oikade_core::{BoxError, CapabilityId, DeviceId, StateStore, Value, validate_identifier};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Bucket, Namespace, Storage, StorageError};

const DEVICE_STATE_FORMAT: u32 = 1;
pub const MAX_DEVICE_STATE_SIZE: usize = 1 << 20;

#[derive(Debug, Error)]
pub enum DeviceStateError {
    #[error("invalid device state: {0}")]
    Invalid(String),
    #[error("unsupported device state format {found}; this build supports {supported}")]
    UnsupportedFormat { found: u32, supported: u32 },
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("decode device state: {0}")]
    Decode(#[from] serde_json::Error),
}

#[derive(Clone)]
pub struct DeviceStateStore {
    bucket: Bucket,
}

#[async_trait]
impl StateStore for DeviceStateStore {
    async fn load_device(
        &self,
        device_id: &DeviceId,
    ) -> Result<Option<BTreeMap<CapabilityId, Value>>, BoxError> {
        let store = self.clone();
        let device_id = device_id.clone();
        tokio::task::spawn_blocking(move || store.load(&device_id))
            .await
            .map_err(|error| Box::new(error) as BoxError)?
            .map_err(|error| Box::new(error) as BoxError)
    }

    async fn save_device(
        &self,
        device_id: &DeviceId,
        values: &BTreeMap<CapabilityId, Value>,
    ) -> Result<(), BoxError> {
        let store = self.clone();
        let device_id = device_id.clone();
        let values = values.clone();
        tokio::task::spawn_blocking(move || store.save(&device_id, &values))
            .await
            .map_err(|error| Box::new(error) as BoxError)?
            .map_err(|error| Box::new(error) as BoxError)
    }
}

impl DeviceStateStore {
    pub fn new(storage: &Storage) -> Self {
        Self {
            bucket: storage.bucket(Namespace::Devices),
        }
    }

    pub fn load(
        &self,
        device_id: &DeviceId,
    ) -> Result<Option<BTreeMap<CapabilityId, Value>>, DeviceStateError> {
        let encoded = match self.bucket.get(&state_key(device_id)) {
            Ok(encoded) => encoded,
            Err(StorageError::NotFound) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if encoded.len() > MAX_DEVICE_STATE_SIZE {
            return Err(DeviceStateError::Invalid(format!(
                "device state exceeds {MAX_DEVICE_STATE_SIZE} bytes"
            )));
        }
        let mut deserializer = serde_json::Deserializer::from_slice(&encoded);
        let record = PersistedDeviceState::deserialize(&mut deserializer)?;
        deserializer.end()?;
        if record.version != DEVICE_STATE_FORMAT {
            return Err(DeviceStateError::UnsupportedFormat {
                found: record.version,
                supported: DEVICE_STATE_FORMAT,
            });
        }
        let mut values = BTreeMap::new();
        for (raw_id, persisted) in record.values {
            validate_identifier(&raw_id)
                .map_err(|error| DeviceStateError::Invalid(error.to_string()))?;
            let capability_id = CapabilityId::new(raw_id)
                .map_err(|error| DeviceStateError::Invalid(error.to_string()))?;
            values.insert(capability_id, persisted.try_into()?);
        }
        Ok(Some(values))
    }

    pub fn save(
        &self,
        device_id: &DeviceId,
        values: &BTreeMap<CapabilityId, Value>,
    ) -> Result<(), DeviceStateError> {
        let mut persisted = BTreeMap::new();
        for (capability_id, value) in values {
            value
                .validate()
                .map_err(|error| DeviceStateError::Invalid(error.to_string()))?;
            persisted.insert(capability_id.as_str().to_owned(), value.clone().into());
        }
        let encoded = serde_json::to_vec(&PersistedDeviceState {
            version: DEVICE_STATE_FORMAT,
            values: persisted,
        })?;
        if encoded.len() > MAX_DEVICE_STATE_SIZE {
            return Err(DeviceStateError::Invalid(format!(
                "device state exceeds {MAX_DEVICE_STATE_SIZE} bytes"
            )));
        }
        self.bucket.set(&state_key(device_id), &encoded)?;
        Ok(())
    }
}

fn state_key(device_id: &DeviceId) -> String {
    format!("{}.state.json", device_id.as_str())
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedDeviceState {
    version: u32,
    values: BTreeMap<String, PersistedValue>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedValue {
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bool: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    integer: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    number: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    string: Option<String>,
}

impl From<Value> for PersistedValue {
    fn from(value: Value) -> Self {
        match value {
            Value::Bool(value) => Self {
                kind: "bool".to_owned(),
                bool: Some(value),
                integer: None,
                number: None,
                string: None,
            },
            Value::Integer(value) => Self {
                kind: "integer".to_owned(),
                bool: None,
                integer: Some(value),
                number: None,
                string: None,
            },
            Value::Number(value) => Self {
                kind: "number".to_owned(),
                bool: None,
                integer: None,
                number: Some(value),
                string: None,
            },
            Value::String(value) => Self {
                kind: "string".to_owned(),
                bool: None,
                integer: None,
                number: None,
                string: Some(value),
            },
        }
    }
}

impl TryFrom<PersistedValue> for Value {
    type Error = DeviceStateError;

    fn try_from(value: PersistedValue) -> Result<Self, Self::Error> {
        let payloads = usize::from(value.bool.is_some())
            + usize::from(value.integer.is_some())
            + usize::from(value.number.is_some())
            + usize::from(value.string.is_some());
        if payloads != 1 {
            return Err(DeviceStateError::Invalid(
                "persisted value must contain exactly one payload".to_owned(),
            ));
        }
        let decoded = match value.kind.as_str() {
            "bool" => Value::Bool(value.bool.ok_or_else(|| {
                DeviceStateError::Invalid("bool value has the wrong payload".to_owned())
            })?),
            "integer" => Value::Integer(value.integer.ok_or_else(|| {
                DeviceStateError::Invalid("integer value has the wrong payload".to_owned())
            })?),
            "number" => Value::Number(value.number.ok_or_else(|| {
                DeviceStateError::Invalid("number value has the wrong payload".to_owned())
            })?),
            "string" => Value::String(value.string.ok_or_else(|| {
                DeviceStateError::Invalid("string value has the wrong payload".to_owned())
            })?),
            kind => {
                return Err(DeviceStateError::Invalid(format!(
                    "unsupported value kind {kind:?}"
                )));
            }
        };
        decoded
            .validate()
            .map_err(|error| DeviceStateError::Invalid(error.to_string()))?;
        Ok(decoded)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use oikade_core::{
        CAPABILITY_SWITCH_ON, Capability, CapabilityType, Command, CommandHandler, Device,
        Permissions, Runtime, ValueKind,
    };

    #[test]
    fn round_trips_typed_values() {
        let root = tempfile::tempdir().unwrap();
        let storage = Storage::open(root.path().join("state")).unwrap();
        let store = DeviceStateStore::new(&storage);
        let device = DeviceId::new("test.device").unwrap();
        let values = BTreeMap::from([
            (CapabilityId::new("bool").unwrap(), Value::Bool(true)),
            (CapabilityId::new("integer").unwrap(), Value::Integer(-42)),
            (CapabilityId::new("number").unwrap(), Value::Number(12.5)),
            (
                CapabilityId::new("string").unwrap(),
                Value::String("homeward".to_owned()),
            ),
        ]);
        store.save(&device, &values).unwrap();
        assert_eq!(store.load(&device).unwrap(), Some(values));
        assert_eq!(
            store
                .load(&DeviceId::new("missing.device").unwrap())
                .unwrap(),
            None
        );
    }

    #[test]
    fn unsupported_record_is_not_rewritten() {
        let root = tempfile::tempdir().unwrap();
        let storage = Storage::open(root.path().join("state")).unwrap();
        let device = DeviceId::new("test.device").unwrap();
        let bucket = storage.bucket(Namespace::Devices);
        let original = br#"{"version":99,"values":{}}"#;
        bucket.set(&state_key(&device), original).unwrap();
        let store = DeviceStateStore::new(&storage);
        assert!(matches!(
            store.load(&device),
            Err(DeviceStateError::UnsupportedFormat { .. })
        ));
        assert_eq!(bucket.get(&state_key(&device)).unwrap(), original);
    }

    #[test]
    fn malformed_record_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let storage = Storage::open(root.path().join("state")).unwrap();
        let device = DeviceId::new("test.device").unwrap();
        storage
            .bucket(Namespace::Devices)
            .set(
                &state_key(&device),
                br#"{"version":1,"values":{"on":{"kind":"bool","string":"wrong"}}}"#,
            )
            .unwrap();
        assert!(DeviceStateStore::new(&storage).load(&device).is_err());
    }

    #[tokio::test]
    async fn runtime_restores_committed_state_and_retains_unknown_capabilities() {
        let root = tempfile::tempdir().unwrap();
        let storage = Storage::open(root.path().join("state")).unwrap();
        let store = Arc::new(DeviceStateStore::new(&storage));
        let device = Device {
            id: DeviceId::new("test.switch").unwrap(),
            name: "Test switch".to_owned(),
            manufacturer: String::new(),
            model: String::new(),
            capabilities: vec![Capability {
                id: CapabilityId::new("on").unwrap(),
                capability_type: CapabilityType::new(CAPABILITY_SWITCH_ON).unwrap(),
                name: "On".to_owned(),
                kind: ValueKind::Bool,
                permissions: Permissions {
                    read: true,
                    write: true,
                    observe: true,
                },
                initial_value: Value::Bool(false),
            }],
        };
        let handler: Arc<dyn CommandHandler> =
            Arc::new(|command: Command| async move { Ok::<Value, BoxError>(command.value) });

        let first = Runtime::new(Some(store.clone()));
        first.start().await.unwrap();
        first
            .register(device.clone(), Some(handler.clone()))
            .await
            .unwrap();
        first
            .write(Command {
                device_id: device.id.clone(),
                capability_id: CapabilityId::new("on").unwrap(),
                value: Value::Bool(true),
            })
            .await
            .unwrap();
        first.stop().await;

        let mut persisted = store.load(&device.id).unwrap().unwrap();
        persisted.insert(
            CapabilityId::new("unknown").unwrap(),
            Value::String("retained".to_owned()),
        );
        store.save(&device.id, &persisted).unwrap();

        let second = Runtime::new(Some(store.clone()));
        second.start().await.unwrap();
        second
            .register(device.clone(), Some(handler))
            .await
            .unwrap();
        assert_eq!(
            second
                .read(&device.id, &CapabilityId::new("on").unwrap())
                .await
                .unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            store
                .load(&device.id)
                .unwrap()
                .unwrap()
                .get(&CapabilityId::new("unknown").unwrap()),
            Some(&Value::String("retained".to_owned()))
        );
    }
}
