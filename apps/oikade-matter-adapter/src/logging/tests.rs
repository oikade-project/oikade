// Copyright 2026 The Oikade Authors
// SPDX-License-Identifier: Apache-2.0

#![allow(clippy::expect_used)]

use oikade_adapter_api::{AdapterLogLevel, AdapterLogRecord};

use super::{encode, should_emit};
use crate::options::LogLevel;

#[test]
fn normal_logging_keeps_milestones_and_suppresses_protocol_chatter() {
    assert!(should_emit(
        LogLevel::Info,
        log::Level::Info,
        "rs_matter::fabric",
        "Added operational fabric with local index 1",
    ));
    assert!(!should_emit(
        LogLevel::Info,
        log::Level::Error,
        "rs_matter::im",
        "Error reading attribute: AttributeNotFound",
    ));
    assert!(!should_emit(
        LogLevel::Info,
        log::Level::Warn,
        "rs_matter::transport",
        "SC::MRPStandAloneAck => No valid exchange found, dropping",
    ));
    assert!(should_emit(
        LogLevel::Debug,
        log::Level::Warn,
        "rs_matter::transport",
        "SC::MRPStandAloneAck => No valid exchange found, dropping",
    ));
}

#[test]
fn structured_logs_escape_multiline_messages() {
    let encoded = encode(AdapterLogLevel::Warn, "rs_matter::transport", "one\ntwo")
        .expect("structured log must encode");
    assert!(!encoded.contains("one\ntwo"));
    let decoded: AdapterLogRecord =
        serde_json::from_str(&encoded).expect("structured log must decode");
    assert_eq!(decoded.message, "one\ntwo");
}
