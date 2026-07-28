# Protocol adapter API

Oikade protocol adapters project the canonical core model into an external
ecosystem. They are distinct from device plugins:

| Boundary | Owns devices | Primary direction | Example |
| --- | --- | --- | --- |
| Device plugin | Plugin | Plugin into Oikade | Vendor HTTP integration |
| Protocol adapter | Oikade core | Oikade into ecosystem | Matter bridge |

The initial adapter API is a language-neutral, versioned newline-delimited JSON
protocol over an inherited anonymous Unix socket. Oikade passes the descriptor
number in `OIKADE_ADAPTER_RPC_FD`. Standard output and standard error remain
diagnostic streams and cannot corrupt protocol traffic.

The host launches adapters with a small allowlist of inherited locale, time,
temporary-directory, certificate and executable-path variables. Additional
variables must be configured explicitly. Oikade reserves the RPC descriptor
and supplies a private adapter state directory in
`OIKADE_ADAPTER_STATE_DIR`; parent-process secrets are not inherited by
default.

Some native protocol stacks need bootstrap data before they can send `hello`.
The Matter adapter receives its validated setup passcode and discriminator in
the explicitly constructed `OIKADE_MATTER_SETUP_PASSCODE` and
`OIKADE_MATTER_DISCRIMINATOR` environment entries. They are never placed in the
child process argument list or general adapter configuration frames. The
sidecar validates them again before starting `rs-matter` and does not log them.
Arbitrary child log text is not treated as safely redactable; see
[logging and secrets](LOGGING.md).

## Lifecycle

1. Oikade launches the adapter and supervises its process.
2. The adapter sends one `hello` notification with its supported API range and
   external protocols.
3. Oikade sends an `initialize` request with the selected API version,
   instance ID and adapter-owned configuration.
4. Oikade sends a complete `sync` request containing every projected device,
   capability and current committed value.
5. Oikade sends acknowledged `event` requests for subsequent committed state
   changes. A topology change causes another complete `sync` generation.
6. The adapter sends `command` requests when an external controller writes a
   value. Oikade responds with the effective value committed by the owning
   integration.
7. Oikade uses `health` requests to inspect responsiveness and protocol-owned
   resources, then ends a graceful session with `shutdown`.

A complete sync replaces the adapter's outward projection atomically. It does
not import devices, rewrite configuration or overwrite core state. Protocol
identity, commissioning credentials, fabrics, pairings and endpoint mappings
remain adapter-owned state.

## Bidirectional framing

Every envelope contains an explicit `kind`:

- `request` has a non-zero ID and receives one response.
- `response` repeats the request ID and method.
- `notification` has ID zero and receives no response.

The explicit kind is required because Oikade and an adapter can initiate calls
at the same time and can independently use the same numeric request ID. A
malformed envelope, unsupported method, oversized frame or response for an
unknown request closes the session and lets supervision apply restart policy.
Version 1 limits each frame to 1 MiB and bounds pending, abandoned and
concurrent request state.

## State and command ordering

The core is authoritative for topology and committed values. A `sync`
generation establishes the baseline before incremental events are delivered.
Events include the core's monotonic process-local revision and UTC commit time.
The sync also carries the revision captured with its snapshot. State and
topology events at or below that revision are already represented and are
discarded, preventing an older queued event from regressing freshly synced
state. Device registration and removal use the same revision sequence and
trigger a replacement sync.

The sync response returns the exact `device_id`/`capability_id` pairs accepted
by the adapter. Oikade forwards incremental events only for that set. Events
for a diagnosed unsupported capability are deliberately ignored, while an
event for a capability absent from the synchronized canonical snapshot still
triggers topology resynchronization. In v1, a missing projection list means
every supplied capability was accepted; an explicit empty list means none were
projected.

Before replacing live dynamic endpoints, the Matter adapter runs the same
mapping validation in a side-effect-free preflight and counts the resulting
endpoints against the SDK build's capacity. The pinned bridge currently allows
16 dynamic endpoints. An over-capacity sync fails with the exact required and
available counts before clearing or adding any endpoint; an unexpected apply
failure clears the attempted projection instead of advertising a partial one.

For an external write, the adapter sends a `command` request. Oikade validates
the device, capability, permissions and value, serialises the command through
the owning integration, persists the effective result and responds with that
effective value. The resulting core event may repeat the same value back to the
adapter; adapters must treat applying an already-current value as idempotent.

## Matter sidecar ownership

The Matter sidecar owns:

- Matter SDK startup and event-loop threading.
- Commissioning, fabrics and operational credentials.
- Matter-specific persistent storage.
- Dynamic endpoint allocation and stable endpoint mappings.
- Cluster and attribute mapping.
- Matter subscriptions, reports and controller command callbacks.

Health responses may include a bounded inventory of protocol-owned resources.
The host validates unique type/ID pairs and bounded non-secret string
attributes before exposing them to the admin API. Matter uses resource type
`matter.fabric`; credentials and setup payloads are not resources.

The optional `remove_resource` request identifies one exact type/ID pair and
returns the complete remaining inventory. It is an explicit operator control,
not a reconciliation side effect. The current Matter implementation reports
`matter.fabric` resources but rejects local removal because `rs-matter` 0.2.0
does not expose the live-session expiry required to complete it safely.
Controller-driven removal remains available. Broad state-directory deletion is
intentionally outside the adapter RPC.

The optional `open_commissioning_window` request contains only a bounded
duration from 180 through 900 seconds. The Matter adapter uses its already
configured commissionable data, performs the operation on the Matter event
loop and returns manual and QR onboarding payloads only in the direct response.
The host validates but never stores or logs those payloads. An existing idle
window can be renewed; an active PASE exchange is never interrupted.

A full adapter reset is deliberately a host lifecycle operation rather than an
adapter RPC. The host first stops the sidecar so no native code retains open
state, verifies an instance-specific marker, replaces only the adapter's
private state directory, and waits for a fresh sidecar to synchronize. This
clears Matter fabrics, credentials, bridge identity and endpoint assignments
without touching canonical devices, plugin state or the Oikade database.

Oikade owns the canonical devices and state, launches and monitors the sidecar,
supplies an adapter-specific private state directory and routes commands to the
device-owning integration. The sidecar can be shipped as a native per-platform
binary; Docker is not part of the runtime design.

Native packages bundle the Matter sidecar built from the same workspace commit
and lockfile. The runtime uses an explicit configured path or a fixed package
location; it never searches `PATH` or downloads executable code. A
side-effect-free metadata probe must match the expected adapter ID, workspace
version, API range and protocol before the host creates or opens adapter state,
followed by normal hello validation after launch. See
[ADR 0008](adr/0008-colocated-matter-adapter.md).

The native v1 implementation has an explicit registry for generic switches,
On/Off lights, On/Off outlets, temperature and relative-humidity measurements,
contact state and occupancy detection. It reports each unknown or malformed
capability with device-scoped diagnostics instead of silently guessing. The
protocol-neutral contracts and active mapping table are documented in the
[capability registry](CAPABILITY_REGISTRY.md). Its
pinned `rs-matter` build, persisted endpoint mapping and real-stack smoke procedure are
documented in the workspace
[Matter sidecar package](../apps/oikade-matter-adapter/README.md).
This is an implementation proof, not yet a user-facing Matter support claim.
The `rs-matter` sidecar still requires physical Apple Home validation,
broader capability testing and multi-admin evidence.

The v1 Rust wire types and strict codec live in `crates/oikade-adapter-api` and
are consumed by both workspace executables. The supervised host, session and
canonical-model conversion live in `crates/oikade-adapter-host`.
