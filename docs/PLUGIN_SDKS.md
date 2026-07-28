# Plugin SDK repository strategy

Oikade currently develops and supports only Rust plugin tooling in this
repository. The plugin boundary is still a process protocol rather than a Rust
ABI, so supporting more languages later does not require changing the core
architecture.

## Current scope

- `oikade-plugin-api` is the checked-in Rust API v1 wire binding and codec.
- A higher-level Rust author SDK should hide inherited-socket setup, handshake,
  lifecycle dispatch, cancellation, health and graceful shutdown.
- First-party plugin examples and conformance fixtures should be written in
  Rust once that author SDK is available.
- The Oikade workspace, CI and release support the Rust binding and tooling.

The normative contract is the versioned protocol documentation plus
`contracts/plugin/v1/frames.jsonl`. Rust types are a binding to that contract,
not permission to expose daemon-internal implementation types across the
process boundary.

## Future languages

Non-Rust SDKs are deferred. If one is created, it should live in a separate
repository with its own language-native package, CI, release cadence and
compatibility declaration. The main Oikade repository should consume only
language-neutral fixtures or released artifacts when a concrete integration
requires them.

This separation keeps optional runtimes out of Oikade's source, build and
installation while preserving a deliberate path to extensibility later.

## Packaging

A plugin is a local artifact directory with an `oikade-plugin.yaml` manifest
and one executable entry point. Oikade does not download plugin binaries,
language runtimes or packages at daemon startup. Third-party distribution,
signing and trust policy remain separate from the wire API.
