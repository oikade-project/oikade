# 0008: Co-locate the Matter adapter in the workspace

- Status: Accepted

## Context

The daemon and Matter adapter are one Rust product and share contracts,
versions, validation and release provenance. Repository ownership and process
isolation solve different problems.

## Decision

Build `oikade` and `oikade-matter-adapter` in one Cargo virtual workspace
with one toolchain and lockfile. Retain the supervised child-process boundary
because Matter owns network-facing code, credentials, fabrics, endpoint
mappings and a distinct asynchronous runtime.

Native packages include both executables from the same commit. The daemon uses
an explicit path or fixed package location and never downloads executable code
or searches `PATH`.

## Consequences

- Host/adapter contract drift becomes a workspace compile or test failure.
- One release attestation covers both executables.
- Matter failures and state remain isolated from the daemon.
- `rs-matter` dependencies are linked only into the adapter executable.
