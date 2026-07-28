use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

pub const VERSION: u32 = 1;
pub const TRANSPORT_FD_ENVIRONMENT: &str = "OIKADE_PLUGIN_RPC_FD";

pub const METHOD_HELLO: &str = "hello";
pub const METHOD_INITIALIZE: &str = "initialize";
pub const METHOD_COMMAND: &str = "command";
pub const METHOD_CANCEL: &str = "cancel";
pub const METHOD_EVENT: &str = "event";
pub const METHOD_RECONCILE: &str = "reconcile";
pub const METHOD_HEALTH: &str = "health";

pub const CAPABILITY_SWITCH_ON: &str = "oikade.switch.on";
pub const CAPABILITY_LIGHT_ON: &str = "oikade.light.on";
pub const CAPABILITY_LIGHT_LEVEL: &str = "oikade.light.level";
pub const CAPABILITY_OUTLET_ON: &str = "oikade.outlet.on";
pub const CAPABILITY_TEMPERATURE: &str = "oikade.sensor.temperature";
pub const CAPABILITY_RELATIVE_HUMIDITY: &str = "oikade.sensor.relative-humidity";
pub const CAPABILITY_CONTACT_OPEN: &str = "oikade.sensor.contact-open";
pub const CAPABILITY_OCCUPANCY_DETECTED: &str = "oikade.sensor.occupancy-detected";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub version: u32,
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
    pub plugin_id: String,
    pub plugin_version: String,
    pub min_api_version: u32,
    pub max_api_version: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InitializeRequest {
    pub api_version: u32,
    pub instance_id: String,
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InitializeResponse {
    pub devices: Vec<Device>,
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
    pub initial_value: Value,
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
pub struct CancelRequest {
    pub request_id: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub device_id: String,
    pub capability_id: String,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reconcile {
    pub devices: Vec<Device>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthResponse {
    pub healthy: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn version_one_frames_match_the_shared_golden_fixture() {
        let on = Value {
            kind: "bool".to_owned(),
            bool: Some(true),
            integer: None,
            number: None,
            string: None,
        };
        let device = Device {
            id: "switch".to_owned(),
            name: "Switch".to_owned(),
            manufacturer: "Oikade".to_owned(),
            model: "Fixture".to_owned(),
            capabilities: vec![Capability {
                id: "on".to_owned(),
                capability_type: CAPABILITY_SWITCH_ON.to_owned(),
                name: "On".to_owned(),
                kind: "bool".to_owned(),
                permissions: Permissions {
                    read: true,
                    write: true,
                    observe: true,
                },
                initial_value: on.clone(),
            }],
        };
        let body = |value| Some(value);
        let frames = vec![
            Envelope {
                version: VERSION,
                id: 0,
                method: METHOD_HELLO.to_owned(),
                body: body(
                    serde_json::value::to_raw_value(&Hello {
                        plugin_id: "example.plugin".to_owned(),
                        plugin_version: "0.1.0".to_owned(),
                        min_api_version: 1,
                        max_api_version: 1,
                    })
                    .unwrap(),
                ),
                error: None,
            },
            Envelope {
                version: VERSION,
                id: 1,
                method: METHOD_INITIALIZE.to_owned(),
                body: body(
                    serde_json::value::to_raw_value(&InitializeRequest {
                        api_version: 1,
                        instance_id: "demo".to_owned(),
                        config: serde_json::json!({"address":"192.0.2.1"}),
                    })
                    .unwrap(),
                ),
                error: None,
            },
            Envelope {
                version: VERSION,
                id: 1,
                method: METHOD_INITIALIZE.to_owned(),
                body: body(
                    serde_json::value::to_raw_value(&InitializeResponse {
                        devices: vec![device.clone()],
                    })
                    .unwrap(),
                ),
                error: None,
            },
            Envelope {
                version: VERSION,
                id: 2,
                method: METHOD_COMMAND.to_owned(),
                body: body(
                    serde_json::value::to_raw_value(&CommandRequest {
                        device_id: "switch".to_owned(),
                        capability_id: "on".to_owned(),
                        value: on.clone(),
                    })
                    .unwrap(),
                ),
                error: None,
            },
            Envelope {
                version: VERSION,
                id: 2,
                method: METHOD_COMMAND.to_owned(),
                body: body(
                    serde_json::value::to_raw_value(&CommandResponse { value: on.clone() })
                        .unwrap(),
                ),
                error: None,
            },
            Envelope {
                version: VERSION,
                id: 0,
                method: METHOD_EVENT.to_owned(),
                body: body(
                    serde_json::value::to_raw_value(&Event {
                        device_id: "switch".to_owned(),
                        capability_id: "on".to_owned(),
                        value: on.clone(),
                    })
                    .unwrap(),
                ),
                error: None,
            },
            Envelope {
                version: VERSION,
                id: 0,
                method: METHOD_RECONCILE.to_owned(),
                body: body(
                    serde_json::value::to_raw_value(&Reconcile {
                        devices: vec![device],
                    })
                    .unwrap(),
                ),
                error: None,
            },
            Envelope {
                version: VERSION,
                id: 3,
                method: METHOD_HEALTH.to_owned(),
                body: body(serde_json::value::to_raw_value(&serde_json::json!({})).unwrap()),
                error: None,
            },
            Envelope {
                version: VERSION,
                id: 3,
                method: METHOD_HEALTH.to_owned(),
                body: body(
                    serde_json::value::to_raw_value(&HealthResponse {
                        healthy: false,
                        detail: "offline".to_owned(),
                    })
                    .unwrap(),
                ),
                error: None,
            },
            Envelope {
                version: VERSION,
                id: 4,
                method: METHOD_CANCEL.to_owned(),
                body: body(
                    serde_json::value::to_raw_value(&CancelRequest { request_id: 2 }).unwrap(),
                ),
                error: None,
            },
            Envelope {
                version: VERSION,
                id: 4,
                method: METHOD_CANCEL.to_owned(),
                body: body(serde_json::value::to_raw_value(&serde_json::json!({})).unwrap()),
                error: None,
            },
            Envelope {
                version: VERSION,
                id: 5,
                method: METHOD_COMMAND.to_owned(),
                body: None,
                error: Some(ProtocolError {
                    code: "command_failed".to_owned(),
                    message: "offline".to_owned(),
                }),
            },
        ];

        let expected: Vec<_> = include_str!("../../../contracts/plugin/v1/frames.jsonl")
            .trim()
            .lines()
            .collect();
        assert_eq!(frames.len(), expected.len());
        for (index, (frame, expected)) in frames.iter().zip(expected).enumerate() {
            let encoded = serde_json::to_string(frame).unwrap();
            assert_eq!(encoded, expected, "frame {index} changed");
        }
    }
}
