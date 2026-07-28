# Logging and secrets

The Oikade daemon uses Rust `tracing` and writes structured text logs to
standard error. `runtime.log_level` or `oikade run --log-level` supplies the
filter. The Matter sidecar uses its own bounded `none`, `error`, `info` or
`debug` setting because it is a separate process.

Matter `info` is the normal operational profile. It retains adapter startup,
commissioning, fabric, subscription and genuine warning/error records while
suppressing packet dumps, late acknowledgements, stale-subscription turnover
and expected unsupported optional-cluster probes. `debug` restores complete
SDK diagnostics. `error` keeps only non-probe errors; `none` disables SDK logs,
although a fatal adapter startup or runtime error is always emitted.

## Child output

Plugin and adapter RPC never uses standard output or standard error. Both
streams are captured by the supervisor and emitted through Oikade's logger
tagged with the component identity and source stream. Empty lines are ignored
and an individual line is truncated after 16 KiB, so a child cannot corrupt
protocol framing or create an unbounded in-memory log line.

The first-party Matter sidecar emits a versioned one-line JSON log envelope.
The host validates that envelope and preserves its debug, info, warning or
error severity. Newlines inside SDK messages are escaped, so one SDK record
becomes one Oikade event. Unstructured third-party child output remains tagged
plain text at info level.

Child output remains untrusted even when it uses the validated severity
envelope. Oikade does not attempt to interpret arbitrary messages or promise
that it can redact a secret after a child has interpolated it into text. Plugin
and adapter authors must never print credentials, authorization headers,
cookies, setup payloads or complete configuration documents.

## Secret handling rules

- Do not include secrets in log messages, errors or structured fields.
- Do not log complete configuration, RPC frames, request bodies or child
  environments.
- Prefer stable component, device and capability identifiers as diagnostic
  context.
- Treat errors returned by external libraries as potentially sensitive before
  forwarding them to logs.
- Keep plugin credentials in explicitly configured plugin data; the supervisor
  does not inherit the parent process environment by default.

The child environment begins with a small locale, time, temporary-directory,
certificate and executable-path allowlist. Parent tokens and cloud credentials
are not inherited implicitly. Additional plugin variables require an explicit
future configuration mechanism.

## Matter onboarding data

The configured Matter setup passcode is passed only in the sidecar's explicitly
constructed environment, never in its command line or ordinary initialization
frame. The sidecar validates it without logging it.

Manual and QR onboarding payloads are returned only through an explicit local
commissioning-info or commissioning-window request while the window is active.
They are not stored in adapter status or canonical state. Fresh state emits a
concise automatic-window milestone with duration, never its pairing data.
Admin responses carry
`Cache-Control: no-store`; neither the daemon nor sidecar logs the successful
response body.
The pinned mDNS backend cannot acknowledge its first published advertisement,
so automatic commissioning also emits one warning identifying that readiness
limitation without including onboarding data.

This design reduces accidental exposure but does not make process listings,
debuggers or a compromised same-user process harmless. Filesystem permissions,
the local administration socket and the operating-system account remain part
of the trust boundary.
