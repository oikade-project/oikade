// Copyright 2026 The Oikade Authors
// SPDX-License-Identifier: Apache-2.0

use std::io::{self, Write as _};

use oikade_adapter_api::{
    AdapterLogLevel, AdapterLogRecord, LOG_RECORD_PREFIX, LOG_RECORD_VERSION,
};

use crate::options::LogLevel;

pub(crate) fn init(level: LogLevel) {
    let _ = log::set_boxed_logger(Box::new(MatterLogger { level }));
    log::set_max_level(log::LevelFilter::Debug);
}

struct MatterLogger {
    level: LogLevel,
}

impl log::Log for MatterLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        level_enabled(self.level, metadata.level())
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let message = record.args().to_string();
        if should_emit(self.level, record.level(), record.target(), &message) {
            emit(log_level(record.level()), record.target(), &message);
        }
    }

    fn flush(&self) {}
}

fn level_enabled(configured: LogLevel, level: log::Level) -> bool {
    match configured {
        LogLevel::None => false,
        LogLevel::Error => level == log::Level::Error,
        LogLevel::Info => matches!(
            level,
            log::Level::Error | log::Level::Warn | log::Level::Info
        ),
        LogLevel::Debug => level != log::Level::Trace,
    }
}

fn should_emit(configured: LogLevel, level: log::Level, target: &str, message: &str) -> bool {
    if !level_enabled(configured, level) {
        return false;
    }
    if configured == LogLevel::Debug || target.starts_with("oikade_matter_adapter") {
        return true;
    }
    match level {
        log::Level::Error => !expected_optional_probe(message),
        log::Level::Warn => !expected_exchange_chatter(message),
        log::Level::Info => matter_milestone(message),
        log::Level::Debug | log::Level::Trace => false,
    }
}

fn expected_optional_probe(message: &str) -> bool {
    message.contains("AttributeNotFound")
        || ((message.contains("Error processing attribute read")
            || message.contains("Error processing attribute write"))
            && (message.contains("UnsupportedAttribute") || message.contains("UnsupportedCluster")))
}

fn expected_exchange_chatter(message: &str) -> bool {
    message.contains("No valid session found")
        || message.contains("No valid exchange found")
        || message.contains("MRPStandAloneAck")
        || message.contains("InvalidSubscription")
        || message.contains("removed during reporting")
        || message.contains("No PASE Commissioning Window to close")
}

fn matter_milestone(message: &str) -> bool {
    message.contains("PASE Basic Commissioning Window opened")
        || message.contains("PASE Commissioning Window closed")
        || message.contains("Commissioning complete")
        || message.contains("Added operational fabric")
        || message.contains("Added subscription")
        || (message.contains("Subscription ") && message.contains(" primed"))
}

fn log_level(level: log::Level) -> AdapterLogLevel {
    match level {
        log::Level::Error => AdapterLogLevel::Error,
        log::Level::Warn => AdapterLogLevel::Warn,
        log::Level::Info => AdapterLogLevel::Info,
        log::Level::Debug | log::Level::Trace => AdapterLogLevel::Debug,
    }
}

fn encode(level: AdapterLogLevel, target: &str, message: &str) -> Option<String> {
    serde_json::to_string(&AdapterLogRecord {
        version: LOG_RECORD_VERSION,
        level,
        target: target.to_owned(),
        message: message.to_owned(),
    })
    .ok()
}

pub(crate) fn emit(level: AdapterLogLevel, target: &str, message: &str) {
    let Some(encoded) = encode(level, target, message) else {
        return;
    };
    let mut stderr = io::stderr().lock();
    let _ = writeln!(stderr, "{LOG_RECORD_PREFIX}{encoded}");
}

#[cfg(test)]
mod tests;
