# 0004: Use strict, versioned YAML configuration

- Status: Accepted

## Context

Operators need readable configuration with precise diagnostics and predictable
validation.

## Decision

Use YAML with a required schema version. Reject unknown fields, duplicate keys,
invalid identifiers and unsupported versions. Resolve relative executable
paths against the configuration file and validate the complete document before
starting components.

Configuration changes must never be applied partially.

## Consequences

- Typographical errors fail early instead of being ignored.
- Schema evolution requires explicit compatibility rules.
- Configuration can be validated without starting the daemon.
- YAML parsing must remain strict despite permissive ecosystem conventions.
