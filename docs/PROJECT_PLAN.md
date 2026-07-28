# Project and implementation plan

## Positioning

Oikade is a lightweight, extensible smart-home integration runtime written in
Rust. It connects vendor devices and services to the ecosystems people already
use while keeping protocol and vendor concerns outside its canonical core.

> Everything, homeward.

**Current status:** the foundation, canonical runtime and Rust plugin host
are complete. M3 is in progress with Matter as the first outward protocol.

## Product goals

- Maintain one protocol-neutral `Device -> Capability -> Value` model.
- Run device integrations and protocol stacks as supervised child processes.
- Provide a first-class Rust plugin SDK over a stable process contract. Other
  language SDKs may be developed later in separate repositories.
- Expose supported canonical devices through Matter to Apple Home, Google Home
  and Amazon Alexa, subject to each ecosystem's feature support.
- Provide predictable local configuration, diagnostics, persistence, backup
  and recovery without an Oikade-hosted cloud dependency.
- Leave room for a separately maintained compatibility host for a documented
  subset of Homebridge plugins without adding its runtime to this repository
  or the core installation.
- Add future ecosystems and transports through adapters rather than changing
  the canonical model into a Matter-specific abstraction.

Matter is not a universal feature superset. Unsupported or ambiguous mappings
must produce explicit diagnostics rather than silently dropping behaviour.
Homebridge compatibility means translating useful devices into canonical
capabilities; it does not promise unsupported Homebridge API parity.

## v1 scope

Included:

- Rust daemon, local administration CLI and HTTP/JSON-over-Unix-socket API.
- Strict versioned YAML configuration and redb-backed canonical state.
- Out-of-process Rust plugins and versioned plugin RPC v1.
- Supervised Matter sidecar built from the same workspace and release.
- Stable Matter identity, fabrics, endpoint allocation and common device
  mappings with bidirectional commands and state reports.
- Multi-admin interoperability evidence for Apple Home, Google Home and Amazon
  Alexa where their supported device sets overlap.
- Native Linux x64, Linux ARM64 and macOS ARM64 packages without Docker.
- Operational health, bounded logs, backup/restore and upgrade/rollback paths.

Deferred beyond v1 unless a concrete requirement changes the decision:

- Additional outward protocols and Apple-specific features outside Matter.
- Complete compatibility with every Homebridge plugin or private API.
- Non-Rust plugin SDKs and a Homebridge compatibility host; these belong in
  separate repositories when pursued.
- Hosted cloud services, remote-access brokerage or a mobile application.
- A full graphical administration interface.
- Multi-node clustering and high availability.
- A public plugin marketplace and automated third-party signing service.
- Direct Zigbee or Thread radio-controller implementations.
- A claim of strong sandboxing on every operating system; process isolation is
  primarily a reliability and secret-reduction boundary.

## Design rules

1. Keep canonical state, lifecycle and routing in the core; keep vendor and
   ecosystem logic outside it.
2. Use versioned wire contracts instead of sharing internal implementation
   types with plugins.
3. Preserve device, capability and protocol identities across restart and
   rediscovery.
4. Bound queues, frames, logs, retries and shutdown time.
5. Make unhealthy components and unsupported mappings visible to operators.
6. Fail closed around ambiguous state, credentials and destructive controls.
7. Access only state explicitly owned by the selected runtime and components.
8. Keep non-Rust runtimes out of this repository and the core installation.
9. Stabilize public contracts only after fixtures and compatibility rules exist.

## Current architecture

The [architecture guide](ARCHITECTURE.md) defines component ownership. In
summary:

- The Rust core owns canonical topology, state, routing and persistence.
- Device plugins own hardware/vendor communication and run over plugin RPC.
- Protocol adapters consume canonical topology and run over adapter RPC.
- The workspace Matter sidecar owns Matter credentials, fabrics, endpoints,
  clusters and subscriptions while remaining process-isolated.
- The local admin API is the only supported operational control boundary.
- Rust is the only currently supported plugin implementation language.

## Milestones

| Milestone | Status | Release intent | Key result |
| --- | --- | --- | --- |
| M0 | Complete | Feasibility | Lifecycle, persistence, supervision and physical ecosystem feasibility established. |
| M1 | Complete | v0.1 core | Persistent canonical devices work through the runtime and admin API. |
| M2 | Complete | v0.2 SDK foundation | External Rust plugins run through the supervised API. |
| M3 | In progress | v0.3 Matter MVP | Canonical devices work reliably through Matter with persistent identity. |
| M4 | Planned | v0.5 alpha | Early adopters can install, diagnose, back up and recover deployments. |
| M5 | Planned | v0.6 plugin preview | Authors can build and package production Rust plugins. |
| M6 | Planned | v0.9 beta | Security, upgrades and sustained reliability meet beta gates. |
| M7 | Planned | v1.0 stable | Public contracts and supported deployment paths are stable. |

### Completed foundation: M0–M2

The active implementation now provides:

- A Cargo workspace on Rust 1.89 with one daemon/CLI binary.
- Strict YAML configuration and a marked redb state format.
- Typed canonical devices, serialized commands, durable effective values and
  bounded state subscriptions.
- A deterministic virtual integration for runtime and protocol testing.
- A local admin socket with status, device, plugin and adapter controls.
- Sanitized process supervision with readiness, graceful termination, restart
  backoff, quarantine and bounded stdout/stderr capture.
- Strict plugin artifacts, inherited-socket RPC, reconciliation, health,
  cancellation and crash/malformed-frame isolation.
- A Rust plugin API binding, strict host implementation and versioned golden
  wire fixtures.

### M3 — Matter bridge MVP

Outcome: supported canonical devices are reliable Matter bridge endpoints with
persistent identity, credentials and bidirectional state.

Implemented:

- Workspace-owned `oikade-matter-adapter` pinned to `rs-matter` 0.2.0.
- Dedicated typed adapter RPC with full topology sync, acknowledged events,
  controller command routing, health and graceful shutdown.
- Private Matter state, stable endpoint mappings and restart recovery.
- Explicit setup passcode/discriminator validation, automatic one-time-per-process-start
  15-minute commissioning for zero-fabric state, explicit status and onboarding
  retrieval, and explicit window reopening controls.
- Fabric inventory and fail-closed local single-fabric removal.
- Explicit full Matter reset through marked private-state rotation.
- Switch, dimmable light, outlet, temperature, humidity, contact and occupancy
  projections with exact diagnostics and 16-endpoint preflight.
- Shared workspace version, lockfile, CI and unified native release packaging.
- Real-process RPC, malformed-frame, restart, state-reset and active-event smoke
  tests.
- Separate host/sidecar CPU and RSS measurement plus a seven-day soak runner.

Remaining M3 gates:

- Validate commissioning, writes, subscriptions and restart recovery with a
  physical Apple Home controller using the `rs-matter` sidecar.
- Validate Matter multi-admin with Apple Home, Google Home and Amazon Alexa and
  document ecosystem-specific differences.
- Exercise physical dimming, sensors, reconnects and subscription recovery.
- Add or adopt a safe `rs-matter` application API that expires live sessions
  during local single-fabric removal; until then the operation stays disabled.
- Run and retain the seven-day active-event soak on the designated Linux ARM64
  reference host.
- Execute and verify the first tagged unified release workflow.

M3 exits when fresh commissioning, identity persistence, controller writes,
state reports, common mappings, multi-admin behaviour, package installation and
safe reset have physical or retained operational evidence.

### M4 — Usable alpha

- Status/doctor and explicit component restart controls.
- Atomic configuration reload with validation and rollback.
- Backup/restore for core and protocol-owned state with clear scope.
- systemd and launchd service definitions and clean-machine installation docs.
- Metrics and useful startup summaries.
- Multiple real device integrations and multi-day deployment tests.

### M5 — Rust plugin ecosystem preview

- Ergonomic Rust author SDK over `oikade-plugin-api`.
- Rust reference plugins covering commands, polling, spontaneous events,
  reconciliation, health and graceful shutdown.
- Plugin conformance suite, packaging contract and author documentation.
- Multiple production Rust integrations exercised in multi-day deployments.
- Explicit compatibility policy for Rust SDK and wire API releases.

### M6 — Security, packaging and beta hardening

- Threat model and child environment/filesystem audit.
- Dependency, license and release-provenance policy.
- Upgrade, rollback, backup and recovery tests.
- Resource budgets, fault injection and sustained multi-integration soak tests.
- No unresolved critical security defects.

### M7 — v1.0 stable

- Frozen plugin, adapter, admin and configuration compatibility policies.
- Documented Matter mappings and ecosystem limitations.
- Supported native packages and operational recovery procedures.
- Measured resource budgets and complete operator/plugin-author documentation.

## Performance policy

Host and sidecar costs are measured separately. Release budgets come from a
documented Linux ARM64 reference host with fixed build flags and retained raw
results; developer and hosted-CI measurements are comparative only.

## Beyond v1

Likely follow-on work includes MQTT, separately maintained language SDKs and
Homebridge compatibility, an optional web UI, MCP/agent integrations outside
the core trust boundary, broader Matter device coverage and radio protocol
controllers. Additional outward protocols require a concrete use case and an
explicit maintenance commitment.
