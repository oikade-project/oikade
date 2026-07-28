// Copyright 2026 The Oikade Authors
// SPDX-License-Identifier: Apache-2.0

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use oikade_adapter_api::{Capability, DeviceState, Value as WireValue};
use serde::{Deserialize, Serialize};

pub const MAX_DYNAMIC_ENDPOINTS: usize = 16;
pub const FIRST_DYNAMIC_ENDPOINT: u16 = 3;

pub const SWITCH_ON: &str = "oikade.switch.on";
pub const LIGHT_ON: &str = "oikade.light.on";
pub const LIGHT_LEVEL: &str = "oikade.light.level";
pub const OUTLET_ON: &str = "oikade.outlet.on";
pub const TEMPERATURE: &str = "oikade.sensor.temperature";
pub const HUMIDITY: &str = "oikade.sensor.relative-humidity";
pub const CONTACT_OPEN: &str = "oikade.sensor.contact-open";
pub const OCCUPANCY: &str = "oikade.sensor.occupancy-detected";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Profile {
    OnOffLight,
    DimmableLight,
    Outlet,
    Temperature,
    Humidity,
    Contact,
    Occupancy,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CanonicalValue {
    Bool(bool),
    Number(f64),
}

impl CanonicalValue {
    pub fn to_wire(&self) -> WireValue {
        match self {
            Self::Bool(value) => WireValue {
                kind: "bool".to_owned(),
                bool: Some(*value),
                integer: None,
                number: None,
                string: None,
            },
            Self::Number(value) => WireValue {
                kind: "number".to_owned(),
                bool: None,
                integer: None,
                number: Some(*value),
                string: None,
            },
        }
    }
}

#[derive(Clone, Debug)]
pub struct Projection {
    pub endpoint: u16,
    pub device_id: String,
    pub primary_capability_id: String,
    pub level_capability_id: Option<String>,
    pub name: String,
    pub unique_id: String,
    pub profile: Profile,
    pub on: Option<bool>,
    pub level: Option<f64>,
    pub sensor: Option<f64>,
    pub binary_sensor: Option<bool>,
}

impl Projection {
    pub fn capability_for_cluster(&self, level: bool) -> &str {
        if level {
            self.level_capability_id
                .as_deref()
                .unwrap_or(&self.primary_capability_id)
        } else {
            &self.primary_capability_id
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Diagnostic {
    pub severity: &'static str,
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ProjectionRef {
    pub device_id: String,
    pub capability_id: String,
}

pub type BuiltProjection = (Vec<Projection>, Vec<ProjectionRef>, Vec<Diagnostic>);

#[derive(Debug)]
pub enum SyncError {
    Capacity(usize),
    Persistence(io::Error),
}

impl From<io::Error> for SyncError {
    fn from(value: io::Error) -> Self {
        Self::Persistence(value)
    }
}

#[derive(Default)]
pub struct ProjectionSet {
    by_endpoint: BTreeMap<u16, Projection>,
    by_capability: HashMap<(String, String), u16>,
}

impl ProjectionSet {
    pub fn endpoint(&self, endpoint: u16) -> Option<&Projection> {
        self.by_endpoint.get(&endpoint)
    }

    pub fn endpoint_mut(&mut self, endpoint: u16) -> Option<&mut Projection> {
        self.by_endpoint.get_mut(&endpoint)
    }

    pub fn find_mut(&mut self, device_id: &str, capability_id: &str) -> Option<&mut Projection> {
        let endpoint = *self
            .by_capability
            .get(&(device_id.to_owned(), capability_id.to_owned()))?;
        self.by_endpoint.get_mut(&endpoint)
    }

    pub fn replace(&mut self, projections: Vec<Projection>) {
        self.by_endpoint.clear();
        self.by_capability.clear();

        for projection in projections {
            let endpoint = projection.endpoint;
            self.by_capability.insert(
                (
                    projection.device_id.clone(),
                    projection.primary_capability_id.clone(),
                ),
                endpoint,
            );
            if let Some(level) = &projection.level_capability_id {
                self.by_capability
                    .insert((projection.device_id.clone(), level.clone()), endpoint);
            }
            self.by_endpoint.insert(endpoint, projection);
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct EndpointMappings {
    version: u8,
    next_endpoint: u16,
    mappings: BTreeMap<String, u16>,
}

impl Default for EndpointMappings {
    fn default() -> Self {
        Self {
            version: 1,
            next_endpoint: FIRST_DYNAMIC_ENDPOINT,
            mappings: BTreeMap::new(),
        }
    }
}

impl EndpointMappings {
    fn load(path: &Path) -> Self {
        let Ok(bytes) = fs::read(path) else {
            return Self::default();
        };
        let Ok(mut state) = serde_json::from_slice::<Self>(&bytes) else {
            log::warn!(
                "ignoring invalid endpoint mapping state at {}",
                path.display()
            );
            return Self::default();
        };
        if state.version != 1 {
            log::warn!(
                "ignoring unsupported endpoint mapping state at {}",
                path.display()
            );
            return Self::default();
        }

        let mut seen = HashSet::new();
        state
            .mappings
            .retain(|_, endpoint| *endpoint >= FIRST_DYNAMIC_ENDPOINT && seen.insert(*endpoint));
        state.next_endpoint = state.next_endpoint.max(FIRST_DYNAMIC_ENDPOINT);
        state
    }

    fn allocate(&mut self, key: &str) -> io::Result<u16> {
        if let Some(endpoint) = self.mappings.get(key) {
            return Ok(*endpoint);
        }

        let used: HashSet<u16> = self.mappings.values().copied().collect();
        let mut candidate = self.next_endpoint.max(FIRST_DYNAMIC_ENDPOINT);
        while used.contains(&candidate) {
            candidate = candidate.checked_add(1).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "Matter endpoint IDs exhausted")
            })?;
        }
        self.next_endpoint = candidate.checked_add(1).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "Matter endpoint IDs exhausted")
        })?;
        self.mappings.insert(key.to_owned(), candidate);
        Ok(candidate)
    }

    fn save(&self, path: &Path) -> io::Result<()> {
        let temporary = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, path)
    }
}

pub struct ProjectionBuilder {
    mapping_path: PathBuf,
    mappings: EndpointMappings,
}

impl ProjectionBuilder {
    pub fn new(private_state_dir: &Path) -> Self {
        let mapping_path = private_state_dir.join("endpoints.json");
        let mappings = EndpointMappings::load(&mapping_path);
        Self {
            mapping_path,
            mappings,
        }
    }

    pub fn build(&mut self, devices: &[DeviceState]) -> Result<BuiltProjection, SyncError> {
        let mut mappings = self.mappings.clone();
        let mut result = Vec::new();
        let mut refs = Vec::new();
        let mut diagnostics = Vec::new();

        for state in devices {
            if state.device.id.is_empty() || state.device.name.is_empty() {
                diagnostics.push(diagnostic(
                    "error",
                    "invalid_device",
                    "device ID and name are required",
                    some(&state.device.id),
                    None,
                ));
                continue;
            }
            if state.device.capabilities.is_empty() {
                diagnostics.push(diagnostic(
                    "info",
                    "unsupported_device",
                    "device has no capabilities to project",
                    some(&state.device.id),
                    None,
                ));
                continue;
            }

            let values: HashMap<&str, &WireValue> = state
                .values
                .iter()
                .map(|entry| (entry.capability_id.as_str(), &entry.value))
                .collect();
            let light_on: Vec<&Capability> = state
                .device
                .capabilities
                .iter()
                .filter(|cap| cap.capability_type == LIGHT_ON)
                .collect();
            let levels: Vec<&Capability> = state
                .device
                .capabilities
                .iter()
                .filter(|cap| cap.capability_type == LIGHT_LEVEL)
                .collect();

            for capability in &state.device.capabilities {
                let id = &capability.id;
                let dev = &state.device.id;
                let can_actuate = capability.permissions.read
                    && capability.permissions.write
                    && capability.permissions.observe;
                let can_observe = capability.permissions.read
                    && !capability.permissions.write
                    && capability.permissions.observe;

                if capability.capability_type == LIGHT_LEVEL {
                    if light_on.is_empty() {
                        diagnostics.push(diagnostic(
                            "warning",
                            "missing_composed_light_on_off",
                            "light level requires one light on/off capability",
                            some(dev),
                            some(id),
                        ));
                    } else if light_on.len() > 1 {
                        diagnostics.push(diagnostic(
                            "warning",
                            "ambiguous_composed_light",
                            "light level has multiple light on/off capabilities",
                            some(dev),
                            some(id),
                        ));
                    }
                    continue;
                }

                let expected_kind = match capability.capability_type.as_str() {
                    SWITCH_ON | LIGHT_ON | OUTLET_ON | CONTACT_OPEN | OCCUPANCY => "bool",
                    TEMPERATURE | HUMIDITY => "number",
                    _ => {
                        diagnostics.push(diagnostic(
                            "info",
                            "unsupported_capability",
                            "capability has no Matter mapping",
                            some(dev),
                            some(id),
                        ));
                        continue;
                    }
                };
                if capability.kind != expected_kind {
                    diagnostics.push(diagnostic(
                        "error",
                        "invalid_capability_kind",
                        "capability kind does not match its Matter mapping",
                        some(dev),
                        some(id),
                    ));
                    continue;
                }
                let actuator = matches!(
                    capability.capability_type.as_str(),
                    SWITCH_ON | LIGHT_ON | OUTLET_ON
                );
                if (actuator && !can_actuate) || (!actuator && !can_observe) {
                    diagnostics.push(diagnostic(
                        "warning",
                        "unsupported_permissions",
                        "capability permissions cannot be represented safely",
                        some(dev),
                        some(id),
                    ));
                    continue;
                }
                let Some(value) = values.get(id.as_str()) else {
                    diagnostics.push(diagnostic(
                        "error",
                        "missing_capability_state",
                        "capability has no current value",
                        some(dev),
                        some(id),
                    ));
                    continue;
                };

                let (profile, on, level, sensor, binary_sensor, level_id) =
                    match capability.capability_type.as_str() {
                        SWITCH_ON => match wire_bool(value) {
                            Some(v) => (Profile::OnOffLight, Some(v), None, None, None, None),
                            None => {
                                diagnostics.push(diagnostic(
                                    "error",
                                    "invalid_on_off",
                                    "on/off state must be boolean",
                                    some(dev),
                                    some(id),
                                ));
                                continue;
                            }
                        },
                        LIGHT_ON => {
                            let Some(on) = wire_bool(value) else {
                                diagnostics.push(diagnostic(
                                    "error",
                                    "invalid_on_off",
                                    "on/off state must be boolean",
                                    some(dev),
                                    some(id),
                                ));
                                continue;
                            };
                            if levels.len() > 1 {
                                diagnostics.push(diagnostic(
                                    "warning",
                                    "ambiguous_light_level",
                                    "light on/off has multiple level capabilities",
                                    some(dev),
                                    some(id),
                                ));
                                (Profile::OnOffLight, Some(on), None, None, None, None)
                            } else if let Some(level_cap) = levels.first() {
                                if level_cap.kind != "number" {
                                    diagnostics.push(diagnostic(
                                        "error",
                                        "invalid_capability_kind",
                                        "composed light level must be numeric",
                                        some(dev),
                                        some(&level_cap.id),
                                    ));
                                    (Profile::OnOffLight, Some(on), None, None, None, None)
                                } else if !level_cap.permissions.read
                                    || !level_cap.permissions.write
                                    || !level_cap.permissions.observe
                                {
                                    diagnostics.push(diagnostic(
                                        "warning",
                                        "unsupported_permissions",
                                        "light level permissions cannot be represented safely",
                                        some(dev),
                                        some(&level_cap.id),
                                    ));
                                    (Profile::OnOffLight, Some(on), None, None, None, None)
                                } else if let Some(level) = values
                                    .get(level_cap.id.as_str())
                                    .and_then(|v| wire_number(v))
                                    .filter(|v| v.is_finite() && (0.0..=100.0).contains(v))
                                {
                                    (
                                        Profile::DimmableLight,
                                        Some(on),
                                        Some(level),
                                        None,
                                        None,
                                        Some(level_cap.id.clone()),
                                    )
                                } else {
                                    diagnostics.push(diagnostic(
                                        "error",
                                        "invalid_light_level",
                                        "light level must be a number from 0 through 100",
                                        some(dev),
                                        some(&level_cap.id),
                                    ));
                                    (Profile::OnOffLight, Some(on), None, None, None, None)
                                }
                            } else {
                                (Profile::OnOffLight, Some(on), None, None, None, None)
                            }
                        }
                        OUTLET_ON => match wire_bool(value) {
                            Some(v) => (Profile::Outlet, Some(v), None, None, None, None),
                            None => {
                                diagnostics.push(diagnostic(
                                    "error",
                                    "invalid_on_off",
                                    "on/off state must be boolean",
                                    some(dev),
                                    some(id),
                                ));
                                continue;
                            }
                        },
                        TEMPERATURE => match wire_number(value)
                            .filter(|v| v.is_finite() && (-273.15..=327.66).contains(v))
                        {
                            Some(v) => (Profile::Temperature, None, None, Some(v), None, None),
                            None => {
                                diagnostics.push(diagnostic(
                                    "error",
                                    "invalid_temperature",
                                    "temperature must be between -273.15 and 327.66 °C",
                                    some(dev),
                                    some(id),
                                ));
                                continue;
                            }
                        },
                        HUMIDITY => match wire_number(value)
                            .filter(|v| v.is_finite() && (0.0..=100.0).contains(v))
                        {
                            Some(v) => (Profile::Humidity, None, None, Some(v), None, None),
                            None => {
                                diagnostics.push(diagnostic(
                                    "error",
                                    "invalid_relative_humidity",
                                    "relative humidity must be between 0 and 100 percent",
                                    some(dev),
                                    some(id),
                                ));
                                continue;
                            }
                        },
                        CONTACT_OPEN => match wire_bool(value) {
                            Some(v) => (Profile::Contact, None, None, None, Some(v), None),
                            None => {
                                diagnostics.push(diagnostic(
                                    "error",
                                    "invalid_sensor_state",
                                    "contact state must be boolean",
                                    some(dev),
                                    some(id),
                                ));
                                continue;
                            }
                        },
                        OCCUPANCY => match wire_bool(value) {
                            Some(v) => {
                                diagnostics.push(diagnostic(
                                    "warning",
                                    "assumed_occupancy_modality",
                                    "occupancy is projected as a PIR sensor",
                                    some(dev),
                                    some(id),
                                ));
                                (Profile::Occupancy, None, None, None, Some(v), None)
                            }
                            None => {
                                diagnostics.push(diagnostic(
                                    "error",
                                    "invalid_sensor_state",
                                    "occupancy state must be boolean",
                                    some(dev),
                                    some(id),
                                ));
                                continue;
                            }
                        },
                        _ => unreachable!(),
                    };

                let mapping_key = format!("{dev}/{id}");
                let endpoint = mappings.allocate(&mapping_key)?;
                let projected_level_id = level_id.clone();
                result.push(Projection {
                    endpoint,
                    device_id: dev.clone(),
                    primary_capability_id: id.clone(),
                    level_capability_id: level_id,
                    name: truncate_utf8(&state.device.name, 32),
                    unique_id: unique_id(dev, id),
                    profile,
                    on,
                    level,
                    sensor,
                    binary_sensor,
                });
                refs.push(ProjectionRef {
                    device_id: dev.clone(),
                    capability_id: id.clone(),
                });
                if let Some(capability_id) = projected_level_id {
                    refs.push(ProjectionRef {
                        device_id: dev.clone(),
                        capability_id,
                    });
                }
            }
        }

        if result.len() > MAX_DYNAMIC_ENDPOINTS {
            return Err(SyncError::Capacity(result.len()));
        }
        mappings.save(&self.mapping_path)?;
        self.mappings = mappings;
        Ok((result, refs, diagnostics))
    }
}

fn some(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}

fn diagnostic(
    severity: &'static str,
    code: &'static str,
    message: &str,
    device_id: Option<String>,
    capability_id: Option<String>,
) -> Diagnostic {
    Diagnostic {
        severity,
        code,
        message: message.to_owned(),
        device_id,
        capability_id,
    }
}

pub fn wire_bool(value: &WireValue) -> Option<bool> {
    (value.kind == "bool").then_some(value.bool).flatten()
}

pub fn wire_number(value: &WireValue) -> Option<f64> {
    (value.kind == "number").then_some(value.number).flatten()
}

pub fn matter_level(percent: f64) -> u8 {
    (1.0 + percent.clamp(0.0, 100.0) * 253.0 / 100.0).round() as u8
}

pub fn canonical_level(level: u8) -> f64 {
    (f64::from(level.clamp(1, 254)) - 1.0) * 100.0 / 253.0
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn unique_id(device_id: &str, capability_id: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in format!("{device_id}/{capability_id}").bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("oikade-{hash:016x}")
}

#[cfg(test)]
mod tests;
