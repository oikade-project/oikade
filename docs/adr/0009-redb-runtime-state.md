# 0009: Use redb for runtime state

- Status: Accepted

## Context

The daemon needs a small embedded transactional store without a database
service or container dependency. Protocol-owned state must remain separate.

## Decision

Use the pure-Rust `redb` engine. Store canonical runtime data in
`runtime-v1.redb` with a strict `runtime-state.json` format marker. Use
separate tables for devices, plugins and discovery, and version persisted
records explicitly.

Reject ambiguous marker/database combinations, unsupported formats and newer
schemas. Commit durable values before publishing corresponding live state.
Matter keeps credentials, fabrics and mappings in its private adapter state.

## Consequences

- Oikade remains a service-free local installation.
- State ownership is explicit between the daemon and adapters.
- Backup and restore must operate on consistent, versioned logical state.
- Cache and storage costs must be measured on the ARM64 reference environment.
