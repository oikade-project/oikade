use std::collections::BTreeMap;

use oikade_core::{
    Capability as CoreCapability, DeviceState, Event as CoreEvent, Value as CoreValue,
};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub const API_VERSION: &str = "v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub api_version: String,
    pub build: String,
    pub healthy: bool,
    pub started_at: String,
    pub uptime_ms: u64,
    pub devices: usize,
    pub subscribers: usize,
    pub plugins: usize,
    pub unhealthy_plugins: usize,
    pub adapters: usize,
    pub unhealthy_adapters: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plugin {
    pub instance_id: String,
    pub plugin_id: String,
    pub name: String,
    pub version: String,
    pub api_version: u32,
    pub artifact: String,
    pub state: String,
    pub healthy: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub restarts: usize,
    pub devices: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub health_detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Adapter {
    pub instance_id: String,
    pub adapter_id: String,
    pub version: String,
    pub protocol: String,
    pub state: String,
    pub healthy: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub restarts: usize,
    pub generation: u64,
    pub snapshot_revision: u64,
    pub devices: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub health_detail: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<AdapterDiagnostic>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<AdapterResource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterResource {
    #[serde(rename = "type")]
    pub resource_type: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterDiagnostic {
    pub severity: String,
    pub code: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub device_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub capability_id: String,
    pub message: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
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
pub struct Event {
    pub device_id: String,
    pub capability_id: String,
    pub value: Value,
    pub revision: u64,
    pub occurred_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<Event>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorPayload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorPayload {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommissioningWindow {
    pub duration_seconds: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_seconds: Option<u16>,
    pub manual_code: String,
    pub qr_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommissioningInfo {
    pub open: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_seconds: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qr_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterReset {
    pub instance_id: String,
    pub state: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct DevicesResponse {
    pub devices: Vec<Device>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PluginsResponse {
    pub plugins: Vec<Plugin>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct AdaptersResponse {
    pub adapters: Vec<Adapter>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct AdapterResourcesResponse {
    pub resources: Vec<AdapterResource>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WriteRequest {
    pub value: Value,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommissioningRequest {
    pub duration_seconds: u16,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResetRequest {
    pub confirmation: String,
}

pub(crate) fn device_from_state(state: &DeviceState) -> Device {
    Device {
        id: state.device.id.as_str().to_owned(),
        name: state.device.name.clone(),
        manufacturer: state.device.manufacturer.clone(),
        model: state.device.model.clone(),
        capabilities: state
            .device
            .capabilities
            .iter()
            .map(|capability| capability_from_state(capability, state.values.get(&capability.id)))
            .collect(),
    }
}

pub(crate) fn capability_from_state(
    capability: &CoreCapability,
    value: Option<&CoreValue>,
) -> Capability {
    Capability {
        id: capability.id.as_str().to_owned(),
        capability_type: capability.capability_type.as_str().to_owned(),
        name: capability.name.clone(),
        kind: capability.kind.as_str().to_owned(),
        permissions: Permissions {
            read: capability.permissions.read,
            write: capability.permissions.write,
            observe: capability.permissions.observe,
        },
        value: capability
            .permissions
            .read
            .then(|| value.map(value_from_core))
            .flatten(),
    }
}

pub fn value_from_core(value: &CoreValue) -> Value {
    let mut converted = Value {
        kind: value.kind().as_str().to_owned(),
        bool: None,
        integer: None,
        number: None,
        string: None,
    };
    match value {
        CoreValue::Bool(value) => converted.bool = Some(*value),
        CoreValue::Integer(value) => converted.integer = Some(*value),
        CoreValue::Number(value) => converted.number = Some(*value),
        CoreValue::String(value) => converted.string = Some(value.clone()),
    }
    converted
}

pub fn value_to_core(value: &Value) -> Result<CoreValue, String> {
    let payloads = usize::from(value.bool.is_some())
        + usize::from(value.integer.is_some())
        + usize::from(value.number.is_some())
        + usize::from(value.string.is_some());
    if payloads != 1 {
        return Err("value must contain exactly one payload".to_owned());
    }
    let converted = match value.kind.as_str() {
        "bool" => CoreValue::Bool(value.bool.ok_or("bool value has the wrong payload")?),
        "integer" => {
            CoreValue::Integer(value.integer.ok_or("integer value has the wrong payload")?)
        }
        "number" => CoreValue::Number(value.number.ok_or("number value has the wrong payload")?),
        "string" => CoreValue::String(
            value
                .string
                .clone()
                .ok_or("string value has the wrong payload")?,
        ),
        other => return Err(format!("unsupported value kind {other:?}")),
    };
    converted.validate().map_err(|error| error.to_string())?;
    Ok(converted)
}

pub(crate) fn event_from_core(event: &CoreEvent) -> Event {
    Event {
        device_id: event.device_id.as_str().to_owned(),
        capability_id: event.capability_id.as_str().to_owned(),
        value: value_from_core(&event.value),
        revision: event.revision,
        occurred_at: OffsetDateTime::from(event.occurred_at)
            .format(&Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned()),
    }
}
