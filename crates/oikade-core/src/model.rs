use std::collections::HashSet;

use thiserror::Error;

const MAX_IDENTIFIER_LENGTH: usize = 128;

pub const CAPABILITY_SWITCH_ON: &str = "oikade.switch.on";
pub const CAPABILITY_LIGHT_ON: &str = "oikade.light.on";
pub const CAPABILITY_LIGHT_LEVEL: &str = "oikade.light.level";
pub const CAPABILITY_OUTLET_ON: &str = "oikade.outlet.on";
pub const CAPABILITY_TEMPERATURE: &str = "oikade.sensor.temperature";
pub const CAPABILITY_RELATIVE_HUMIDITY: &str = "oikade.sensor.relative-humidity";
pub const CAPABILITY_CONTACT_OPEN: &str = "oikade.sensor.contact-open";
pub const CAPABILITY_OCCUPANCY_DETECTED: &str = "oikade.sensor.occupancy-detected";

pub const LIGHT_LEVEL_MINIMUM: f64 = 0.0;
pub const LIGHT_LEVEL_MAXIMUM: f64 = 100.0;

#[derive(Debug, Error, Clone, PartialEq)]
#[error("invalid core model: {0}")]
pub struct ModelError(String);

impl ModelError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId(String);

impl DeviceId {
    pub fn new(value: impl Into<String>) -> Result<Self, ModelError> {
        let value = value.into();
        validate_identifier(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapabilityId(String);

impl CapabilityId {
    pub fn new(value: impl Into<String>) -> Result<Self, ModelError> {
        let value = value.into();
        validate_identifier(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapabilityType(String);

impl CapabilityType {
    pub fn new(value: impl Into<String>) -> Result<Self, ModelError> {
        let value = value.into();
        validate_identifier(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Bool,
    Integer,
    Number,
    String,
}

impl ValueKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Integer => "integer",
            Self::Number => "number",
            Self::String => "string",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Integer(i64),
    Number(f64),
    String(String),
}

impl Value {
    pub const fn kind(&self) -> ValueKind {
        match self {
            Self::Bool(_) => ValueKind::Bool,
            Self::Integer(_) => ValueKind::Integer,
            Self::Number(_) => ValueKind::Number,
            Self::String(_) => ValueKind::String,
        }
    }

    pub fn validate(&self) -> Result<(), ModelError> {
        if let Self::Number(number) = self
            && !number.is_finite()
        {
            return Err(ModelError::new("number must be finite"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    pub read: bool,
    pub write: bool,
    pub observe: bool,
}

impl Permissions {
    pub const fn any(self) -> bool {
        self.read || self.write || self.observe
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Capability {
    pub id: CapabilityId,
    pub capability_type: CapabilityType,
    pub name: String,
    pub kind: ValueKind,
    pub permissions: Permissions,
    pub initial_value: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub id: DeviceId,
    pub name: String,
    pub manufacturer: String,
    pub model: String,
    pub capabilities: Vec<Capability>,
}

impl Device {
    pub fn validate(&self) -> Result<(), ModelError> {
        if self.name.trim().is_empty() {
            return Err(ModelError::new(format!(
                "device {:?} name is required",
                self.id.as_str()
            )));
        }
        if self.capabilities.is_empty() {
            return Err(ModelError::new(format!(
                "device {:?} must have at least one capability",
                self.id.as_str()
            )));
        }

        let mut seen = HashSet::with_capacity(self.capabilities.len());
        for capability in &self.capabilities {
            capability.validate()?;
            if !seen.insert(capability.id.clone()) {
                return Err(ModelError::new(format!(
                    "device {:?} has duplicate capability ID {:?}",
                    self.id.as_str(),
                    capability.id.as_str()
                )));
            }
        }
        Ok(())
    }
}

impl Capability {
    pub fn validate(&self) -> Result<(), ModelError> {
        if self.name.trim().is_empty() {
            return Err(ModelError::new(format!(
                "capability {:?} name is required",
                self.id.as_str()
            )));
        }
        if let Some(builtin) = lookup_builtin_capability(&self.capability_type)
            && self.kind != builtin.kind
        {
            return Err(ModelError::new(format!(
                "capability {:?} kind is {:?}, want {:?} for type {:?}",
                self.id.as_str(),
                self.kind.as_str(),
                builtin.kind.as_str(),
                self.capability_type.as_str()
            )));
        }
        if !self.permissions.any() {
            return Err(ModelError::new(format!(
                "capability {:?} must permit at least one operation",
                self.id.as_str()
            )));
        }
        self.initial_value.validate()?;
        if self.initial_value.kind() != self.kind {
            return Err(ModelError::new(format!(
                "capability {:?} initial value is {:?}, want {:?}",
                self.id.as_str(),
                self.initial_value.kind().as_str(),
                self.kind.as_str()
            )));
        }
        if let Some(builtin) = lookup_builtin_capability(&self.capability_type) {
            builtin.validate_value(&self.initial_value)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityRole {
    Actuator,
    Sensor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltinCapability {
    pub capability_type: CapabilityType,
    pub kind: ValueKind,
    pub role: CapabilityRole,
}

impl BuiltinCapability {
    pub fn validate_value(&self, value: &Value) -> Result<(), ModelError> {
        value.validate()?;
        if value.kind() != self.kind {
            return Err(ModelError::new(format!(
                "capability type {:?} requires {:?}, got {:?}",
                self.capability_type.as_str(),
                self.kind.as_str(),
                value.kind().as_str()
            )));
        }
        if self.capability_type.as_str() == CAPABILITY_LIGHT_LEVEL
            && let Value::Number(level) = value
            && !(LIGHT_LEVEL_MINIMUM..=LIGHT_LEVEL_MAXIMUM).contains(level)
        {
            return Err(ModelError::new(format!(
                "capability type {:?} requires a percentage from 0 through 100, got {level}",
                self.capability_type.as_str()
            )));
        }
        Ok(())
    }
}

pub fn builtin_capabilities() -> Vec<BuiltinCapability> {
    [
        (
            CAPABILITY_SWITCH_ON,
            ValueKind::Bool,
            CapabilityRole::Actuator,
        ),
        (
            CAPABILITY_LIGHT_ON,
            ValueKind::Bool,
            CapabilityRole::Actuator,
        ),
        (
            CAPABILITY_LIGHT_LEVEL,
            ValueKind::Number,
            CapabilityRole::Actuator,
        ),
        (
            CAPABILITY_OUTLET_ON,
            ValueKind::Bool,
            CapabilityRole::Actuator,
        ),
        (
            CAPABILITY_TEMPERATURE,
            ValueKind::Number,
            CapabilityRole::Sensor,
        ),
        (
            CAPABILITY_RELATIVE_HUMIDITY,
            ValueKind::Number,
            CapabilityRole::Sensor,
        ),
        (
            CAPABILITY_CONTACT_OPEN,
            ValueKind::Bool,
            CapabilityRole::Sensor,
        ),
        (
            CAPABILITY_OCCUPANCY_DETECTED,
            ValueKind::Bool,
            CapabilityRole::Sensor,
        ),
    ]
    .into_iter()
    .map(|(capability_type, kind, role)| BuiltinCapability {
        capability_type: CapabilityType(capability_type.to_owned()),
        kind,
        role,
    })
    .collect()
}

pub fn lookup_builtin_capability(capability_type: &CapabilityType) -> Option<BuiltinCapability> {
    builtin_capabilities()
        .into_iter()
        .find(|builtin| builtin.capability_type == *capability_type)
}

pub fn validate_identifier(value: &str) -> Result<(), ModelError> {
    if value.is_empty() {
        return Err(ModelError::new("identifier is empty"));
    }
    if value.len() > MAX_IDENTIFIER_LENGTH {
        return Err(ModelError::new(format!(
            "identifier exceeds {MAX_IDENTIFIER_LENGTH} bytes"
        )));
    }
    let bytes = value.as_bytes();
    let valid_edge = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    let valid_inner = |byte: u8| valid_edge(byte) || matches!(byte, b'.' | b'_' | b'-');
    if !valid_edge(bytes[0])
        || !valid_edge(bytes[bytes.len() - 1])
        || !bytes.iter().copied().all(valid_inner)
    {
        return Err(ModelError::new(
            "identifier must contain lowercase letters, digits, dots, underscores, or hyphens",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn id(value: &str) -> DeviceId {
        DeviceId::new(value).unwrap()
    }

    fn capability_id(value: &str) -> CapabilityId {
        CapabilityId::new(value).unwrap()
    }

    fn capability_type(value: &str) -> CapabilityType {
        CapabilityType::new(value).unwrap()
    }

    fn switch() -> Device {
        Device {
            id: id("builtin.virtual.kitchen-switch"),
            name: "Kitchen switch".to_owned(),
            manufacturer: String::new(),
            model: String::new(),
            capabilities: vec![Capability {
                id: capability_id("on"),
                capability_type: capability_type(CAPABILITY_SWITCH_ON),
                name: "On".to_owned(),
                kind: ValueKind::Bool,
                permissions: Permissions {
                    read: true,
                    write: true,
                    observe: true,
                },
                initial_value: Value::Bool(false),
            }],
        }
    }

    #[test]
    fn values_are_typed_and_comparable() {
        assert_eq!(Value::Bool(true).kind(), ValueKind::Bool);
        assert_eq!(Value::Bool(true), Value::Bool(true));
        assert_ne!(Value::Bool(true), Value::Bool(false));
        assert_ne!(Value::Bool(true), Value::Integer(1));
        assert!(Value::Number(f64::INFINITY).validate().is_err());
    }

    #[test]
    fn accepts_protocol_neutral_switch() {
        switch().validate().unwrap();
    }

    #[test]
    fn builtin_registry_is_stable() {
        assert_eq!(builtin_capabilities().len(), 8);
        let switch = lookup_builtin_capability(&capability_type(CAPABILITY_SWITCH_ON)).unwrap();
        assert_eq!(switch.kind, ValueKind::Bool);
        assert_eq!(switch.role, CapabilityRole::Actuator);
        let temperature =
            lookup_builtin_capability(&capability_type(CAPABILITY_TEMPERATURE)).unwrap();
        assert_eq!(temperature.kind, ValueKind::Number);
        assert_eq!(temperature.role, CapabilityRole::Sensor);
    }

    #[test]
    fn light_level_validates_percentage_range() {
        let level = lookup_builtin_capability(&capability_type(CAPABILITY_LIGHT_LEVEL)).unwrap();
        for value in [0.0, 50.5, 100.0] {
            level.validate_value(&Value::Number(value)).unwrap();
        }
        for value in [-0.01, 100.01, f64::INFINITY] {
            assert!(level.validate_value(&Value::Number(value)).is_err());
        }
        assert!(level.validate_value(&Value::Integer(50)).is_err());
    }

    #[test]
    fn rejects_invalid_definitions() {
        let mut device = switch();
        device.name = " ".to_owned();
        assert!(
            device
                .validate()
                .unwrap_err()
                .to_string()
                .contains("name is required")
        );

        let mut device = switch();
        device.capabilities.clear();
        assert!(
            device
                .validate()
                .unwrap_err()
                .to_string()
                .contains("at least one")
        );

        let mut device = switch();
        device.capabilities.push(device.capabilities[0].clone());
        assert!(
            device
                .validate()
                .unwrap_err()
                .to_string()
                .contains("duplicate")
        );

        let mut device = switch();
        device.capabilities[0].kind = ValueKind::String;
        assert!(device.validate().unwrap_err().to_string().contains("want"));

        let mut device = switch();
        device.capabilities[0].permissions = Permissions {
            read: false,
            write: false,
            observe: false,
        };
        assert!(
            device
                .validate()
                .unwrap_err()
                .to_string()
                .contains("at least one operation")
        );
    }

    #[test]
    fn identifiers_match_the_existing_contract() {
        for valid in ["a", "a.b", "a_b", "a-b", "0", "a0"] {
            validate_identifier(valid).unwrap();
        }
        for invalid in ["", ".a", "a.", "A", "a b", "é"] {
            assert!(validate_identifier(invalid).is_err(), "{invalid:?}");
        }
    }
}
