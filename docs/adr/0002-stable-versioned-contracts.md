# 0002: Use stable, versioned process contracts

- Status: Accepted

## Context

Plugins and protocol adapters must evolve independently from the daemon without
sharing internal Rust types or memory.

## Decision

Define strict, versioned, language-neutral wire contracts for plugin and
adapter communication. Frames use bounded newline-delimited JSON with explicit
handshakes, request identifiers, compatibility ranges and validation.

The canonical core remains protocol-neutral: devices publish typed
capabilities and values, while adapters map those concepts into external
ecosystems.

## Consequences

- Contract compatibility can be tested with shared fixtures.
- Future SDKs can target other languages without changing the daemon.
- Changes to framing or semantics require explicit compatibility policy.
- Internal refactoring does not alter the public process boundary.
