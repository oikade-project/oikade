// Copyright 2026 The Oikade Authors
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_channel::{Receiver, Sender};
use embassy_futures::select::{Either, select};
use oikade_adapter_api::{
    CommandRequest as WireCommandRequest, CommandResponse, CommissioningInfoRequest, Envelope,
    EventRequest, FrameKind, Hello, InitializeRequest, METHOD_COMMAND, METHOD_COMMISSIONING_INFO,
    METHOD_EVENT, METHOD_HEALTH, METHOD_HELLO, METHOD_INITIALIZE, METHOD_OPEN_COMMISSIONING_WINDOW,
    METHOD_REMOVE_RESOURCE, METHOD_SHUTDOWN, METHOD_SYNC, OpenCommissioningWindowRequest,
    ProtocolError, RemoveResourceRequest, SyncRequest, VERSION, Value as WireValue, decode_body,
    encode_body,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value as JsonValue, json};

use crate::bridge::{BridgeState, CommandRequest};
use crate::projection::{ProjectionBuilder, SyncError};
use crate::wire::Wire;

const MAX_PENDING_COMMANDS: usize = 1024;

#[derive(Debug)]
pub struct RpcFailure {
    pub code: &'static str,
    pub message: String,
}

impl RpcFailure {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub enum RuntimeRequest {
    TopologyChanged,
    AttributeChanged {
        endpoint: u16,
        cluster: u32,
        attribute: u32,
    },
    Health {
        response: Sender<Result<JsonValue, RpcFailure>>,
    },
    OpenCommissioningWindow {
        duration_seconds: u16,
        response: Sender<Result<JsonValue, RpcFailure>>,
    },
    CommissioningInfo {
        response: Sender<Result<JsonValue, RpcFailure>>,
    },
    RemoveResource {
        resource_type: String,
        id: String,
        response: Sender<Result<JsonValue, RpcFailure>>,
    },
}

pub enum ExitReason {
    Shutdown,
    Disconnected,
}

pub async fn run(
    wire: Wire,
    commands: Receiver<CommandRequest>,
    runtime: Sender<RuntimeRequest>,
    state: Arc<BridgeState>,
    private_state_dir: &Path,
) -> ExitReason {
    if !send_notification(&wire.outgoing, METHOD_HELLO, &hello()).await {
        return ExitReason::Disconnected;
    }

    let mut builder = ProjectionBuilder::new(private_state_dir);
    let mut pending: HashMap<u64, Sender<Result<WireValue, ()>>> = HashMap::new();
    let mut next_request_id = 1_u64;

    loop {
        match select(wire.incoming.recv(), commands.recv()).await {
            Either::First(Ok(envelope)) => match envelope.kind {
                FrameKind::Response => {
                    if envelope.method != METHOD_COMMAND {
                        log::error!("adapter response has unsupported method");
                        return ExitReason::Disconnected;
                    }
                    let Some(response) = pending.remove(&envelope.id) else {
                        log::error!("adapter response has unknown request ID");
                        return ExitReason::Disconnected;
                    };
                    let effective = if envelope.error.is_some() {
                        Err(())
                    } else {
                        envelope
                            .body
                            .as_deref()
                            .ok_or(())
                            .and_then(|body| decode_body::<CommandResponse>(body).map_err(|_| ()))
                            .map(|body| body.value)
                    };
                    let _ = response.send(effective).await;
                }
                FrameKind::Request => {
                    let method = envelope.method.clone();
                    let outcome = handle_request(
                        &method,
                        envelope.body.as_deref(),
                        &mut builder,
                        &runtime,
                        &state,
                    )
                    .await;
                    let sent = match outcome {
                        Ok(body) => {
                            send_response(&wire.outgoing, envelope.id, &method, &body).await
                        }
                        Err(error) => send_error(&wire.outgoing, envelope.id, &method, error).await,
                    };
                    if !sent {
                        return ExitReason::Disconnected;
                    }
                    if method == METHOD_SHUTDOWN {
                        return ExitReason::Shutdown;
                    }
                }
                FrameKind::Notification => {
                    log::error!("host sent an unsupported notification");
                    return ExitReason::Disconnected;
                }
            },
            Either::First(Err(_)) => return ExitReason::Disconnected,
            Either::Second(Ok(command)) => {
                if pending.len() >= MAX_PENDING_COMMANDS {
                    let _ = command.response.send(Err(())).await;
                    continue;
                }
                while next_request_id == 0 || pending.contains_key(&next_request_id) {
                    next_request_id = next_request_id.wrapping_add(1);
                }
                let id = next_request_id;
                next_request_id = next_request_id.wrapping_add(1);
                pending.insert(id, command.response.clone());
                let request = WireCommandRequest {
                    device_id: command.device_id,
                    capability_id: command.capability_id,
                    value: command.value.to_wire(),
                };
                if !send_request(&wire.outgoing, id, METHOD_COMMAND, &request).await {
                    if let Some(response) = pending.remove(&id) {
                        let _ = response.send(Err(())).await;
                    }
                    return ExitReason::Disconnected;
                }
            }
            Either::Second(Err(_)) => return ExitReason::Disconnected,
        }
    }
}

async fn handle_request(
    method: &str,
    body: Option<&serde_json::value::RawValue>,
    builder: &mut ProjectionBuilder,
    runtime: &Sender<RuntimeRequest>,
    state: &Arc<BridgeState>,
) -> Result<JsonValue, RpcFailure> {
    match method {
        METHOD_INITIALIZE => {
            let request: InitializeRequest = request_body(body)?;
            if request.api_version != VERSION || request.instance_id.is_empty() {
                return Err(RpcFailure::new(
                    "invalid_request",
                    "API version and instance ID are required",
                ));
            }
            Ok(json!({"ready": true}))
        }
        METHOD_SYNC => {
            let request: SyncRequest = request_body(body)?;
            let device_count = request.devices.len();
            let (projections, projected, diagnostics) = match builder.build(&request.devices) {
                Ok(result) => result,
                Err(SyncError::Capacity(required)) => {
                    return Err(RpcFailure::new(
                        "capacity_exceeded",
                        format!(
                            "Matter projection requires {required} endpoints; bridge capacity is {}",
                            crate::projection::MAX_DYNAMIC_ENDPOINTS
                        ),
                    ));
                }
                Err(SyncError::Persistence(error)) => {
                    return Err(RpcFailure::new(
                        "projection_failed",
                        format!("persist endpoint projection: {error}"),
                    ));
                }
            };
            state.replace(projections);
            runtime
                .send(RuntimeRequest::TopologyChanged)
                .await
                .map_err(|_| RpcFailure::new("unavailable", "Matter runtime is unavailable"))?;
            Ok(json!({
                "generation": request.generation,
                "devices": device_count,
                "projections": projected,
                "diagnostics": diagnostics,
            }))
        }
        METHOD_EVENT => {
            let request: EventRequest = request_body(body)?;
            if request.device_id.is_empty() || request.capability_id.is_empty() {
                return Err(RpcFailure::new(
                    "invalid_request",
                    "device and capability IDs are required",
                ));
            }
            match state.update_event(&request.device_id, &request.capability_id, &request.value) {
                Ok(Some((endpoint, cluster, attribute))) => runtime
                    .send(RuntimeRequest::AttributeChanged {
                        endpoint,
                        cluster,
                        attribute,
                    })
                    .await
                    .map_err(|_| RpcFailure::new("unavailable", "Matter runtime is unavailable"))?,
                Ok(None) => {}
                Err("not_found") => {
                    return Err(RpcFailure::new(
                        "not_found",
                        "projected capability was not found",
                    ));
                }
                Err(_) => {
                    return Err(RpcFailure::new(
                        "invalid_value",
                        "event value has the wrong kind or range",
                    ));
                }
            }
            Ok(json!({}))
        }
        METHOD_HEALTH => {
            let _: JsonValue = request_body(body)?;
            runtime_round_trip(runtime, |response| RuntimeRequest::Health { response }).await
        }
        METHOD_OPEN_COMMISSIONING_WINDOW => {
            let request: OpenCommissioningWindowRequest = request_body(body)?;
            if !(180..=900).contains(&request.duration_seconds) {
                return Err(RpcFailure::new(
                    "invalid_duration",
                    "commissioning duration must be between 180 and 900 seconds",
                ));
            }
            runtime_round_trip(runtime, |response| {
                RuntimeRequest::OpenCommissioningWindow {
                    duration_seconds: request.duration_seconds,
                    response,
                }
            })
            .await
        }
        METHOD_COMMISSIONING_INFO => {
            let _: CommissioningInfoRequest = request_body(body)?;
            runtime_round_trip(runtime, |response| RuntimeRequest::CommissioningInfo {
                response,
            })
            .await
        }
        METHOD_REMOVE_RESOURCE => {
            let request: RemoveResourceRequest = request_body(body)?;
            if request.resource_type.is_empty() || request.id.is_empty() {
                return Err(RpcFailure::new(
                    "invalid_request",
                    "resource type and ID are required",
                ));
            }
            runtime_round_trip(runtime, |response| RuntimeRequest::RemoveResource {
                resource_type: request.resource_type,
                id: request.id,
                response,
            })
            .await
        }
        METHOD_SHUTDOWN => {
            let _: JsonValue = request_body(body)?;
            Ok(json!({}))
        }
        _ => Err(RpcFailure::new(
            "unsupported_method",
            "unsupported host request",
        )),
    }
}

fn request_body<T: DeserializeOwned>(
    body: Option<&serde_json::value::RawValue>,
) -> Result<T, RpcFailure> {
    body.ok_or_else(|| RpcFailure::new("invalid_request", "request body is required"))
        .and_then(|body| {
            decode_body(body).map_err(|error| RpcFailure::new("invalid_request", error.to_string()))
        })
}

async fn runtime_round_trip(
    runtime: &Sender<RuntimeRequest>,
    request: impl FnOnce(Sender<Result<JsonValue, RpcFailure>>) -> RuntimeRequest,
) -> Result<JsonValue, RpcFailure> {
    let (tx, rx) = async_channel::bounded(1);
    runtime
        .send(request(tx))
        .await
        .map_err(|_| RpcFailure::new("unavailable", "Matter runtime is unavailable"))?;
    rx.recv()
        .await
        .map_err(|_| RpcFailure::new("unavailable", "Matter runtime is unavailable"))?
}

async fn send_notification<T: Serialize>(
    outgoing: &Sender<Envelope>,
    method: &str,
    body: &T,
) -> bool {
    send(outgoing, FrameKind::Notification, 0, method, body).await
}

async fn send_request<T: Serialize>(
    outgoing: &Sender<Envelope>,
    id: u64,
    method: &str,
    body: &T,
) -> bool {
    send(outgoing, FrameKind::Request, id, method, body).await
}

async fn send_response<T: Serialize>(
    outgoing: &Sender<Envelope>,
    id: u64,
    method: &str,
    body: &T,
) -> bool {
    send(outgoing, FrameKind::Response, id, method, body).await
}

async fn send<T: Serialize>(
    outgoing: &Sender<Envelope>,
    kind: FrameKind,
    id: u64,
    method: &str,
    body: &T,
) -> bool {
    let Ok(body) = encode_body(body) else {
        return false;
    };
    outgoing
        .send(Envelope {
            version: VERSION,
            kind,
            id,
            method: method.to_owned(),
            body: Some(body),
            error: None,
        })
        .await
        .is_ok()
}

async fn send_error(outgoing: &Sender<Envelope>, id: u64, method: &str, error: RpcFailure) -> bool {
    outgoing
        .send(Envelope {
            version: VERSION,
            kind: FrameKind::Response,
            id,
            method: method.to_owned(),
            body: None,
            error: Some(ProtocolError {
                code: error.code.to_owned(),
                message: error.message,
            }),
        })
        .await
        .is_ok()
}

fn hello() -> Hello {
    Hello {
        adapter_id: "oikade.matter".to_owned(),
        adapter_version: env!("CARGO_PKG_VERSION").to_owned(),
        min_api_version: VERSION,
        max_api_version: VERSION,
        protocols: vec!["matter".to_owned()],
    }
}

pub fn metadata() -> JsonValue {
    serde_json::to_value(hello()).unwrap_or_else(|_| json!({}))
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn metadata_uses_the_shared_contract_version() {
        let metadata: Hello = serde_json::from_value(metadata()).expect("metadata must decode");
        assert_eq!(metadata.adapter_id, "oikade.matter");
        assert_eq!(metadata.min_api_version, VERSION);
        assert_eq!(metadata.max_api_version, VERSION);
    }

    #[test]
    fn commissioning_info_forwards_a_read_only_runtime_request() {
        futures_lite::future::block_on(async {
            let root = tempfile::tempdir().expect("temporary directory");
            let mut builder = ProjectionBuilder::new(root.path());
            let state = Arc::new(BridgeState::new(async_channel::bounded(1).0));
            let (runtime_tx, runtime_rx) = async_channel::bounded(1);
            let body = serde_json::value::to_raw_value(&CommissioningInfoRequest {})
                .expect("request should encode");

            let request = handle_request(
                METHOD_COMMISSIONING_INFO,
                Some(&body),
                &mut builder,
                &runtime_tx,
                &state,
            );
            let response = async {
                match runtime_rx.recv().await.expect("runtime request") {
                    RuntimeRequest::CommissioningInfo { response } => response
                        .send(Ok(json!({"open": false})))
                        .await
                        .expect("response should be delivered"),
                    _ => panic!("commissioning info must not request a window open"),
                }
            };
            let (result, ()) = embassy_futures::join::join(request, response).await;

            assert_eq!(
                result.expect("request should succeed"),
                json!({"open": false})
            );
        });
    }
}
