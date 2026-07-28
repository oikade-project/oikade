# 0010: Keep first-party tooling Rust-first

- Status: Accepted

## Context

Oikade needs one complete, well-tested plugin author path before maintaining
bindings across several language ecosystems.

## Decision

Build the daemon, first-party adapters, contract crates and current plugin
tooling in Rust. The repository and release use one Rust toolchain.

Keep process contracts language-neutral. Future non-Rust SDKs or compatibility
hosts belong in separate repositories with their own toolchain, CI, release
cadence and declared supported API range.

## Consequences

- First-party runtime and plugin development share types and tooling.
- Rust plugin ergonomics and conformance are the immediate SDK priority.
- Other languages remain possible without becoming runtime dependencies.
- External SDKs must prove compatibility independently.
