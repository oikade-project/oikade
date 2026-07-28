use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

pub const VERSION: u32 = 1;
pub const MINIMUM_COMMISSIONING_WINDOW_SECONDS: u16 = 3 * 60;
pub const MAXIMUM_COMMISSIONING_WINDOW_SECONDS: u16 = 15 * 60;
pub const TRANSPORT_FD_ENVIRONMENT: &str = "OIKADE_ADAPTER_RPC_FD";
pub const STATE_DIRECTORY_ENVIRONMENT: &str = "OIKADE_ADAPTER_STATE_DIR";
pub const LOG_RECORD_PREFIX: &str = "@oikade-adapter-log ";
pub const LOG_RECORD_VERSION: u8 = 1;

pub const METHOD_HELLO: &str = "hello";
pub const METHOD_INITIALIZE: &str = "initialize";
pub const METHOD_SYNC: &str = "sync";
pub const METHOD_EVENT: &str = "event";
pub const METHOD_COMMAND: &str = "command";
pub const METHOD_HEALTH: &str = "health";
pub const METHOD_OPEN_COMMISSIONING_WINDOW: &str = "open_commissioning_window";
pub const METHOD_REMOVE_RESOURCE: &str = "remove_resource";
pub const METHOD_SHUTDOWN: &str = "shutdown";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdapterLogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterLogRecord {
    pub version: u8,
    pub level: AdapterLogLevel,
    pub target: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrameKind {
    Request,
    Response,
    Notification,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub version: u32,
    pub kind: FrameKind,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub id: u64,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Box<RawValue>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ProtocolError>,
}

impl PartialEq for Envelope {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
            && self.kind == other.kind
            && self.id == other.id
            && self.method == other.method
            && self.body.as_deref().map(RawValue::get) == other.body.as_deref().map(RawValue::get)
            && self.error == other.error
    }
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub adapter_id: String,
    pub adapter_version: String,
    pub min_api_version: u32,
    pub max_api_version: u32,
    pub protocols: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InitializeRequest {
    pub api_version: u32,
    pub instance_id: String,
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitializeResponse {
    pub ready: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub manufacturer: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capability {
    pub id: String,
    #[serde(rename = "type")]
    pub capability_type: String,
    pub name: String,
    pub kind: String,
    pub permissions: Permissions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Permissions {
    pub read: bool,
    pub write: bool,
    pub observe: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Value {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bool: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integer: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub string: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityState {
    pub capability_id: String,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceState {
    pub device: Device,
    pub values: Vec<CapabilityState>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncRequest {
    pub generation: u64,
    pub revision: u64,
    pub devices: Vec<DeviceState>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncResponse {
    pub generation: u64,
    pub devices: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projections: Option<Vec<Projection>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Projection {
    pub device_id: String,
    pub capability_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: String,
    pub code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub device_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub capability_id: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventRequest {
    pub device_id: String,
    pub capability_id: String,
    pub value: Value,
    pub revision: u64,
    pub occurred_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventResponse {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandRequest {
    pub device_id: String,
    pub capability_id: String,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandResponse {
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthResponse {
    pub healthy: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<Resource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resource {
    #[serde(rename = "type")]
    pub resource_type: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveResourceRequest {
    #[serde(rename = "type")]
    pub resource_type: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoveResourceResponse {
    pub resources: Vec<Resource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenCommissioningWindowRequest {
    pub duration_seconds: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenCommissioningWindowResponse {
    pub duration_seconds: u16,
    pub manual_code: String,
    pub qr_code: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShutdownResponse {}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{Decoder, Encoder};

    #[test]
    fn bidirectional_envelope_round_trips() {
        let frame = Envelope {
            version: VERSION,
            kind: FrameKind::Request,
            id: 7,
            method: METHOD_HEALTH.to_owned(),
            body: Some(serde_json::value::to_raw_value(&serde_json::json!({})).unwrap()),
            error: None,
        };
        let mut encoder = Encoder::new(Vec::new());
        encoder.encode(&frame).unwrap();
        let bytes = encoder.into_inner();
        let mut decoder = Decoder::new(bytes.as_slice());
        assert_eq!(decoder.decode().unwrap(), Some(frame));
    }

    #[test]
    fn projection_presence_preserves_v1_omission_semantics() {
        let missing: SyncResponse =
            serde_json::from_str(r#"{"generation":1,"devices":0,"diagnostics":[]}"#).unwrap();
        assert_eq!(missing.projections, None);

        let null: SyncResponse =
            serde_json::from_str(r#"{"generation":1,"devices":0,"projections":null}"#).unwrap();
        assert_eq!(null.projections, None);

        let empty: SyncResponse =
            serde_json::from_str(r#"{"generation":1,"devices":0,"projections":[]}"#).unwrap();
        assert_eq!(empty.projections, Some(Vec::new()));
    }
}
