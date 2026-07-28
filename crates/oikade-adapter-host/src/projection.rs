use std::collections::{BTreeMap, BTreeSet};

use oikade_adapter_api as api;
use oikade_core::{CapabilityId, Command, DeviceId, Event, Runtime, RuntimeError, Snapshot, Value};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::session::CommandHandler;

pub type ProjectionKeys = BTreeMap<String, BTreeSet<String>>;

pub fn sync_request(snapshot: &Snapshot, generation: u64) -> Result<api::SyncRequest, String> {
    if !snapshot.started {
        return Err("core runtime is stopped".to_owned());
    }
    let mut devices = Vec::with_capacity(snapshot.devices.len());
    for state in &snapshot.devices {
        let capabilities = state
            .device
            .capabilities
            .iter()
            .map(|capability| api::Capability {
                id: capability.id.as_str().to_owned(),
                capability_type: capability.capability_type.as_str().to_owned(),
                name: capability.name.clone(),
                kind: capability.kind.as_str().to_owned(),
                permissions: api::Permissions {
                    read: capability.permissions.read,
                    write: capability.permissions.write,
                    observe: capability.permissions.observe,
                },
            })
            .collect::<Vec<_>>();
        let mut values = Vec::with_capacity(capabilities.len());
        for capability in &state.device.capabilities {
            let value = state.values.get(&capability.id).ok_or_else(|| {
                format!(
                    "snapshot device {:?} is missing capability {:?} state",
                    state.device.id.as_str(),
                    capability.id.as_str()
                )
            })?;
            values.push(api::CapabilityState {
                capability_id: capability.id.as_str().to_owned(),
                value: value_to_api(value),
            });
        }
        devices.push(api::DeviceState {
            device: api::Device {
                id: state.device.id.as_str().to_owned(),
                name: state.device.name.clone(),
                manufacturer: state.device.manufacturer.clone(),
                model: state.device.model.clone(),
                capabilities,
            },
            values,
        });
    }
    Ok(api::SyncRequest {
        generation,
        revision: snapshot.revision,
        devices,
    })
}

pub fn projection_keys(devices: &[api::DeviceState]) -> ProjectionKeys {
    devices
        .iter()
        .map(|state| {
            (
                state.device.id.clone(),
                state
                    .device
                    .capabilities
                    .iter()
                    .map(|capability| capability.id.clone())
                    .collect(),
            )
        })
        .collect()
}

pub fn accepted_projection_keys(
    projections: Option<&[api::Projection]>,
    available: &ProjectionKeys,
) -> Result<ProjectionKeys, String> {
    let Some(projections) = projections else {
        return Ok(available.clone());
    };
    let mut accepted = ProjectionKeys::new();
    for (index, projection) in projections.iter().enumerate() {
        let capabilities = available.get(&projection.device_id).ok_or_else(|| {
            format!(
                "projection {index} references unknown device {:?}",
                projection.device_id
            )
        })?;
        if !capabilities.contains(&projection.capability_id) {
            return Err(format!(
                "projection {index} references unknown capability {:?}/{:?}",
                projection.device_id, projection.capability_id
            ));
        }
        if !accepted
            .entry(projection.device_id.clone())
            .or_default()
            .insert(projection.capability_id.clone())
        {
            return Err(format!(
                "projection {index} duplicates capability {:?}/{:?}",
                projection.device_id, projection.capability_id
            ));
        }
    }
    Ok(accepted)
}

pub fn contains(keys: &ProjectionKeys, event: &Event) -> bool {
    keys.get(event.device_id.as_str())
        .is_some_and(|capabilities| capabilities.contains(event.capability_id.as_str()))
}

pub fn event_request(event: &Event) -> api::EventRequest {
    let occurred_at = OffsetDateTime::from(event.occurred_at)
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned());
    api::EventRequest {
        device_id: event.device_id.as_str().to_owned(),
        capability_id: event.capability_id.as_str().to_owned(),
        value: value_to_api(&event.value),
        revision: event.revision,
        occurred_at,
    }
}

pub fn value_from_api(value: &api::Value) -> Result<Value, String> {
    let payloads = usize::from(value.bool.is_some())
        + usize::from(value.integer.is_some())
        + usize::from(value.number.is_some())
        + usize::from(value.string.is_some());
    if payloads != 1 {
        return Err("value must contain exactly one payload".to_owned());
    }
    let value = match value.kind.as_str() {
        "bool" => Value::Bool(value.bool.ok_or("bool value has the wrong payload")?),
        "integer" => Value::Integer(value.integer.ok_or("integer value has the wrong payload")?),
        "number" => Value::Number(value.number.ok_or("number value has the wrong payload")?),
        "string" => Value::String(
            value
                .string
                .clone()
                .ok_or("string value has the wrong payload")?,
        ),
        other => return Err(format!("unsupported value kind {other:?}")),
    };
    value.validate().map_err(|error| error.to_string())?;
    Ok(value)
}

pub fn value_to_api(value: &Value) -> api::Value {
    let mut converted = api::Value {
        kind: value.kind().as_str().to_owned(),
        bool: None,
        integer: None,
        number: None,
        string: None,
    };
    match value {
        Value::Bool(value) => converted.bool = Some(*value),
        Value::Integer(value) => converted.integer = Some(*value),
        Value::Number(value) => converted.number = Some(*value),
        Value::String(value) => converted.string = Some(value.clone()),
    }
    converted
}

pub struct RuntimeCommandHandler {
    pub runtime: Runtime,
}

#[async_trait::async_trait]
impl CommandHandler for RuntimeCommandHandler {
    async fn handle(
        &self,
        request: api::CommandRequest,
    ) -> Result<api::CommandResponse, api::ProtocolError> {
        let value = value_from_api(&request.value).map_err(|message| api::ProtocolError {
            code: "invalid_value".to_owned(),
            message,
        })?;
        let device_id = DeviceId::new(request.device_id).map_err(|error| api::ProtocolError {
            code: "not_found".to_owned(),
            message: error.to_string(),
        })?;
        let capability_id =
            CapabilityId::new(request.capability_id).map_err(|error| api::ProtocolError {
                code: "not_found".to_owned(),
                message: error.to_string(),
            })?;
        let effective = self
            .runtime
            .write_value(Command {
                device_id,
                capability_id,
                value,
            })
            .await
            .map_err(runtime_error)?;
        Ok(api::CommandResponse {
            value: value_to_api(&effective),
        })
    }
}

fn runtime_error(error: RuntimeError) -> api::ProtocolError {
    let code = match error {
        RuntimeError::NotFound(_) => "not_found",
        RuntimeError::NotWritable(_) => "not_writable",
        RuntimeError::InvalidValue(_) => "invalid_value",
        RuntimeError::Stopped => "unavailable",
        _ => "command_failed",
    };
    api::ProtocolError {
        code: code.to_owned(),
        message: error.to_string(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn explicit_empty_projection_accepts_nothing() {
        let available = BTreeMap::from([("device".to_owned(), BTreeSet::from(["on".to_owned()]))]);
        assert_eq!(
            accepted_projection_keys(Some(&[]), &available).unwrap(),
            BTreeMap::new()
        );
        assert_eq!(
            accepted_projection_keys(None, &available).unwrap(),
            available
        );
    }
}
