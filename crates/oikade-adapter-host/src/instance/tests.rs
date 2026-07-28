#![allow(clippy::unwrap_used)]

use oikade_adapter_api as api;

use super::{
    InstanceError, map_session_error, validate_commissioning_info_response,
    validate_commissioning_response,
};
use crate::session::SessionError;

fn active_info() -> api::CommissioningInfoResponse {
    api::CommissioningInfoResponse {
        open: true,
        duration_seconds: Some(900),
        remaining_seconds: Some(899),
        manual_code: Some("12345678901".to_owned()),
        qr_code: Some("MT:TEST".to_owned()),
    }
}

#[test]
fn accepts_active_and_closed_commissioning_status() {
    validate_commissioning_info_response(&active_info()).unwrap();
    validate_commissioning_info_response(&api::CommissioningInfoResponse {
        open: false,
        duration_seconds: None,
        remaining_seconds: None,
        manual_code: None,
        qr_code: None,
    })
    .unwrap();
}

#[test]
fn accepts_active_untracked_window_without_payload() {
    validate_commissioning_info_response(&api::CommissioningInfoResponse {
        open: true,
        duration_seconds: None,
        remaining_seconds: None,
        manual_code: None,
        qr_code: None,
    })
    .unwrap();
}

#[test]
fn rejects_closed_status_with_pairing_data() {
    let mut response = active_info();
    response.open = false;
    assert!(validate_commissioning_info_response(&response).is_err());
}

#[test]
fn rejects_partial_or_invalid_active_status() {
    let mut response = active_info();
    response.qr_code = None;
    assert!(validate_commissioning_info_response(&response).is_err());

    let mut response = active_info();
    response.remaining_seconds = Some(901);
    assert!(validate_commissioning_info_response(&response).is_err());

    let mut response = active_info();
    response.manual_code = None;
    response.qr_code = None;
    assert!(validate_commissioning_info_response(&response).is_err());

    let mut response = active_info();
    response.duration_seconds = None;
    response.remaining_seconds = None;
    assert!(validate_commissioning_info_response(&response).is_err());
}

#[test]
fn explicit_response_requires_valid_duration_remaining_time_and_payload() {
    let response = api::OpenCommissioningWindowResponse {
        duration_seconds: 900,
        remaining_seconds: Some(899),
        manual_code: "12345678901".to_owned(),
        qr_code: "MT:TEST".to_owned(),
    };
    validate_commissioning_response(&response).unwrap();

    let mut response = response.clone();
    response.remaining_seconds = None;
    assert!(validate_commissioning_response(&response).is_err());

    response.remaining_seconds = Some(901);
    assert!(validate_commissioning_response(&response).is_err());
}

#[test]
fn allowlisted_adapter_protocol_error_uses_host_owned_message() {
    let error = map_session_error(SessionError::Protocol {
        code: "window_conflict".to_owned(),
        message: "setup passcode is 12345678".to_owned(),
    });

    assert_eq!(
        error.protocol_error(),
        Some((
            "window_conflict",
            "a commissioning window is already active"
        ))
    );
}

#[test]
fn sanitizes_unknown_and_malformed_adapter_protocol_errors() {
    for code in ["internal_error", "Window Conflict"] {
        let error = map_session_error(SessionError::Protocol {
            code: code.to_owned(),
            message: "setup passcode is 12345678".to_owned(),
        });
        assert!(matches!(error, InstanceError::Operation(_)));
        assert_eq!(error.protocol_error(), None);
    }
}

#[test]
fn protocol_error_accessor_sanitizes_constructed_errors() {
    let error = InstanceError::AdapterProtocol {
        code: "internal_error".to_owned(),
        message: "setup passcode is 12345678".to_owned(),
    };

    assert_eq!(error.protocol_error(), None);
}
