use std::{collections::BTreeMap, sync::Arc};

use oikade_core::{
    CAPABILITY_SWITCH_ON, Capability as CoreCapability, CapabilityId, CapabilityType, Command,
    Device as CoreDevice, DeviceId, Permissions, Value as CoreValue, ValueKind,
};

use super::*;
use crate::{ApiError, Client, ClientError, Value};

#[tokio::test]
async fn local_api_reads_writes_and_removes_its_private_socket() {
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("admin.sock");
    let runtime = Runtime::new(None);
    runtime.start().await.unwrap();
    runtime
        .register(
            CoreDevice {
                id: DeviceId::new("test.switch").unwrap(),
                name: "Test Switch".to_owned(),
                manufacturer: "Oikade".to_owned(),
                model: "Test".to_owned(),
                capabilities: vec![CoreCapability {
                    id: CapabilityId::new("on").unwrap(),
                    capability_type: CapabilityType::new(CAPABILITY_SWITCH_ON).unwrap(),
                    name: "On".to_owned(),
                    kind: ValueKind::Bool,
                    permissions: Permissions {
                        read: true,
                        write: true,
                        observe: true,
                    },
                    initial_value: CoreValue::Bool(false),
                }],
            },
            Some(Arc::new(|command: Command| async move {
                Ok::<_, oikade_core::BoxError>(command.value)
            })),
        )
        .await
        .unwrap();
    let server = Server::new(runtime.clone(), &socket, Vec::new(), Vec::new()).unwrap();
    server.start().await.unwrap();

    assert_eq!(
        fs::symlink_metadata(&socket).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let client = Client::new(&socket).unwrap();
    assert_eq!(client.status().await.unwrap().devices, 1);
    assert_eq!(client.devices().await.unwrap()[0].id, "test.switch");
    let commissioning_error = client.commissioning_info("missing").await.unwrap_err();
    assert!(matches!(
        commissioning_error,
        ClientError::Api(ApiError {
            status: StatusCode::NOT_FOUND,
            ..
        })
    ));
    let malformed = reqwest::Client::builder()
        .unix_socket(socket.clone())
        .build()
        .unwrap()
        .put("http://oikade/v1/devices/test.switch/capabilities/on")
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        malformed.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-store"
    );
    assert_eq!(
        malformed.json::<ErrorPayload>().await.unwrap().code,
        "invalid_request"
    );
    let committed = client
        .set_capability(
            "test.switch",
            "on",
            Value {
                kind: "bool".to_owned(),
                bool: Some(true),
                integer: None,
                number: None,
                string: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(committed.value.unwrap().bool, Some(true));
    assert_eq!(
        runtime.snapshot().await.devices.first().unwrap().values,
        BTreeMap::from([(CapabilityId::new("on").unwrap(), CoreValue::Bool(true))])
    );

    server.stop().await.unwrap();
    assert!(!socket.exists());
    runtime.stop().await;
}

#[test]
fn adapter_protocol_errors_keep_only_allowlisted_code_and_fixed_message() {
    let failure = adapter_failure(AdapterInstanceError::AdapterProtocol {
        code: "window_closing".to_owned(),
        message: "setup passcode is 12345678".to_owned(),
    });

    assert_eq!(failure.status, StatusCode::BAD_GATEWAY);
    assert_eq!(failure.payload.code, "window_closing");
    assert_eq!(
        failure.payload.message,
        "the previous commissioning window is still closing"
    );
}

#[test]
fn unknown_adapter_protocol_errors_use_a_safe_generic_fallback() {
    let failure = adapter_failure(AdapterInstanceError::AdapterProtocol {
        code: "internal_error".to_owned(),
        message: "setup passcode is 12345678".to_owned(),
    });

    assert_eq!(failure.status, StatusCode::BAD_GATEWAY);
    assert_eq!(failure.payload.code, "adapter_error");
    assert_eq!(failure.payload.message, "adapter operation failed");
}

#[test]
fn non_protocol_adapter_errors_use_a_safe_generic_fallback() {
    let failure = adapter_failure(AdapterInstanceError::Operation(
        "internal detail that must not cross the admin boundary".to_owned(),
    ));

    assert_eq!(failure.status, StatusCode::BAD_GATEWAY);
    assert_eq!(failure.payload.code, "adapter_error");
    assert_eq!(failure.payload.message, "adapter operation failed");
}
