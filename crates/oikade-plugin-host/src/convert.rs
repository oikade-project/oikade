use oikade_core::{
    Capability, CapabilityId, CapabilityType, Device, DeviceId, Permissions, Value, ValueKind,
};
use oikade_plugin_api as api;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConversionError {
    #[error("invalid device or capability: {0}")]
    Model(#[from] oikade_core::ModelError),
    #[error("value must contain exactly one payload")]
    PayloadCount,
    #[error("{0} value has the wrong payload")]
    WrongPayload(String),
    #[error("unsupported value kind {0:?}")]
    UnsupportedKind(String),
}

pub fn device_from_api(instance_id: &str, device: &api::Device) -> Result<Device, ConversionError> {
    let converted = Device {
        id: DeviceId::new(format!("plugin.{instance_id}.{}", device.id))?,
        name: device.name.clone(),
        manufacturer: device.manufacturer.clone(),
        model: device.model.clone(),
        capabilities: device
            .capabilities
            .iter()
            .map(|capability| {
                let kind = kind_from_api(&capability.kind)?;
                Ok(Capability {
                    id: CapabilityId::new(capability.id.clone())?,
                    capability_type: CapabilityType::new(capability.capability_type.clone())?,
                    name: capability.name.clone(),
                    kind,
                    permissions: Permissions {
                        read: capability.permissions.read,
                        write: capability.permissions.write,
                        observe: capability.permissions.observe,
                    },
                    initial_value: value_from_api(&capability.initial_value)?,
                })
            })
            .collect::<Result<Vec<_>, ConversionError>>()?,
    };
    converted.validate()?;
    Ok(converted)
}

pub fn value_from_api(value: &api::Value) -> Result<Value, ConversionError> {
    let payloads = usize::from(value.bool.is_some())
        + usize::from(value.integer.is_some())
        + usize::from(value.number.is_some())
        + usize::from(value.string.is_some());
    if payloads != 1 {
        return Err(ConversionError::PayloadCount);
    }
    let converted = match value.kind.as_str() {
        "bool" => Value::Bool(
            value
                .bool
                .ok_or_else(|| ConversionError::WrongPayload("bool".to_owned()))?,
        ),
        "integer" => Value::Integer(
            value
                .integer
                .ok_or_else(|| ConversionError::WrongPayload("integer".to_owned()))?,
        ),
        "number" => Value::Number(
            value
                .number
                .ok_or_else(|| ConversionError::WrongPayload("number".to_owned()))?,
        ),
        "string" => Value::String(
            value
                .string
                .clone()
                .ok_or_else(|| ConversionError::WrongPayload("string".to_owned()))?,
        ),
        other => return Err(ConversionError::UnsupportedKind(other.to_owned())),
    };
    converted.validate()?;
    Ok(converted)
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

fn kind_from_api(kind: &str) -> Result<ValueKind, ConversionError> {
    match kind {
        "bool" => Ok(ValueKind::Bool),
        "integer" => Ok(ValueKind::Integer),
        "number" => Ok(ValueKind::Number),
        "string" => Ok(ValueKind::String),
        other => Err(ConversionError::UnsupportedKind(other.to_owned())),
    }
}
