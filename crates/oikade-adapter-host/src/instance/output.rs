use oikade_adapter_api as api;
use oikade_supervisor::LogStream;

fn parse_structured_log(line: &str) -> Option<api::AdapterLogRecord> {
    let encoded = line.strip_prefix(api::LOG_RECORD_PREFIX)?;
    let record: api::AdapterLogRecord = serde_json::from_str(encoded).ok()?;
    (record.version == api::LOG_RECORD_VERSION).then_some(record)
}

pub(super) fn emit_adapter_output(
    instance_id: &str,
    adapter_id: &str,
    stream: LogStream,
    line: &str,
) {
    if line.trim().is_empty() {
        return;
    }
    let stream = match stream {
        LogStream::Stdout => "stdout",
        LogStream::Stderr => "stderr",
    };
    let Some(record) = parse_structured_log(line) else {
        tracing::info!(
            adapter_instance = instance_id,
            adapter_id,
            stream,
            line,
            "adapter output"
        );
        return;
    };
    match record.level {
        api::AdapterLogLevel::Debug => tracing::debug!(
            adapter_instance = instance_id,
            adapter_id,
            stream,
            adapter_target = record.target,
            message = record.message,
            "adapter output"
        ),
        api::AdapterLogLevel::Info => tracing::info!(
            adapter_instance = instance_id,
            adapter_id,
            stream,
            adapter_target = record.target,
            message = record.message,
            "adapter output"
        ),
        api::AdapterLogLevel::Warn => tracing::warn!(
            adapter_instance = instance_id,
            adapter_id,
            stream,
            adapter_target = record.target,
            message = record.message,
            "adapter output"
        ),
        api::AdapterLogLevel::Error => tracing::error!(
            adapter_instance = instance_id,
            adapter_id,
            stream,
            adapter_target = record.target,
            message = record.message,
            "adapter output"
        ),
    }
}

#[cfg(test)]
mod tests;
