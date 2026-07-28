#![allow(clippy::expect_used)]

use oikade_adapter_api as api;

use super::parse_structured_log;

#[test]
fn structured_log_requires_the_current_version() {
    let line = format!(
        "{}{}",
        api::LOG_RECORD_PREFIX,
        serde_json::to_string(&api::AdapterLogRecord {
            version: api::LOG_RECORD_VERSION,
            level: api::AdapterLogLevel::Warn,
            target: "rs_matter::transport".to_owned(),
            message: "line one\nline two".to_owned(),
        })
        .expect("log record must encode")
    );
    let parsed = parse_structured_log(&line).expect("log record must parse");
    assert_eq!(parsed.level, api::AdapterLogLevel::Warn);
    assert_eq!(parsed.message, "line one\nline two");

    let wrong_version = line.replace(
        &format!("\"version\":{}", api::LOG_RECORD_VERSION),
        "\"version\":99",
    );
    assert!(parse_structured_log(&wrong_version).is_none());
    assert!(parse_structured_log("ordinary child output").is_none());
}
