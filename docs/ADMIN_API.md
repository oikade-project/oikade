# Local administration API

Oikade exposes a versioned HTTP/JSON administration API only on a Unix domain
socket. The `oikade` CLI uses this API to inspect and control a running daemon
without reading the redb database directly.

## Socket and access

The default socket is `oikade.sock` inside the runtime state directory. It can
be changed with `runtime.admin_socket` in YAML or `oikade run --socket <path>`.
Client commands accept `--config`, `--state-dir` or `--socket` to locate the
same daemon.

The socket is created with mode `0600`; its owning operating-system user is the
local trust boundary. Oikade does not expose this API on TCP. It is not a remote
administration service and is separate from plugin and adapter RPC.

Unix socket paths have a small platform limit. Oikade rejects paths longer than
99 bytes with an actionable startup error. A custom path such as
`/run/oikade/oikade.sock` can be used when the state directory path is unusually
long.

At startup Oikade refuses to replace a regular file, directory or active socket.
It reclaims an existing socket only when a connection attempt explicitly reports
`connection refused`. At shutdown it removes only the socket inode created by
that server instance.

## CLI

```sh
oikade status --state-dir /var/lib/oikade
oikade devices list --state-dir /var/lib/oikade
oikade devices get --state-dir /var/lib/oikade \
  builtin.virtual.example-switch on
oikade devices set --state-dir /var/lib/oikade \
  builtin.virtual.example-switch on true
oikade devices watch --state-dir /var/lib/oikade
oikade plugins list --state-dir /var/lib/oikade
oikade plugins inspect --state-dir /var/lib/oikade garden-weather
oikade adapters list --state-dir /var/lib/oikade
oikade adapters inspect --state-dir /var/lib/oikade matter
oikade adapters commissioning-info --state-dir /var/lib/oikade matter
```

Values are parsed according to the capability's declared kind. Booleans accept
only `true` or `false`; integers and finite numbers use their normal decimal
forms; strings use the supplied argument verbatim.

## Version 1 endpoints

| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/v1/status` | Runtime health, build, uptime and counts. |
| `GET` | `/v1/devices` | Current device definitions, permissions and readable values. |
| `GET` | `/v1/plugins` | Configured Rust plugin instances and supervision state. |
| `GET` | `/v1/plugins/{instance}` | Manifest, process, restart and device details for one instance. |
| `GET` | `/v1/adapters` | Configured protocol adapters and supervision state. |
| `GET` | `/v1/adapters/{instance}` | Process, health, synchronization and projection diagnostics for one adapter. |
| `GET` | `/v1/adapters/{instance}/commissioning-window` | Inspect current commissioning status and retrieve active onboarding payloads. |
| `POST` | `/v1/adapters/{instance}/commissioning-window` | Explicitly request bounded onboarding payloads. |
| `POST` | `/v1/adapters/{instance}/reset` | Reset only the selected adapter's private protocol state. |
| `DELETE` | `/v1/adapters/{instance}/resources/{type}/{id}` | Remove one supported protocol-owned resource. |
| `GET` | `/v1/devices/{device}/capabilities/{capability}` | Capability metadata and its value when readable. |
| `PUT` | `/v1/devices/{device}/capabilities/{capability}` | Validate, dispatch and commit a typed value. |
| `GET` | `/v1/events` | Newline-delimited state-change stream. |

Values use a tagged representation with exactly one payload:

```json
{
  "value": {
    "kind": "bool",
    "bool": true
  }
}
```

Supported kinds are `bool`, `integer`, `number` and `string`. Errors return an
HTTP status plus a stable `code` and a human-readable `message`. Request bodies,
unary responses and individual stream records are bounded. Event subscriptions
use the core's bounded queue and cannot block command routing.

Runtime health includes configured plugin and protocol-adapter health. A plugin
or adapter in starting, backoff, stopped or quarantined state makes
`/v1/status` unhealthy and increments its corresponding unhealthy count;
details remain available from the subsystem endpoints. This does not stop
device inspection or healthy integrations.

A responsive plugin can also report its underlying integration as unhealthy,
with an optional detail shown by `plugins inspect`. This affects overall health
without restarting the process. A missing RPC health response is instead a
process failure handled by restart backoff and quarantine.

Adapter details include the external protocol, process state, restart count,
health response, current full-sync generation, canonical snapshot revision and
non-fatal projection diagnostics. Protocol-owned resources reported by the
adapter are included as typed IDs, display names and non-secret attributes.
The Matter sidecar currently reports each commissioned fabric with its local
index, label, vendor ID, fabric ID, node ID and compressed fabric ID. Setup
codes and operational credentials are never included in status or inventory.

Fresh Matter state automatically opens one 15-minute window when no fabrics
exist. `GET /v1/adapters/{instance}/commissioning-window` reports whether it is
open and returns timing and onboarding payloads only while Oikade owns an active
window. The request never changes commissioning state. The CLI equivalent is:

```sh
oikade adapters commissioning-info matter
```

`POST /v1/adapters/{instance}/commissioning-window` accepts a
`duration_seconds` from 180 through 900. It opens a window and returns its
original duration, remaining time, Matter manual code and QR payload to that
explicit caller. If an Oikade-owned window is already active, the operation is
idempotent: it returns that window without extending it. A controller-owned
window returns `window_conflict` without payloads. An expired Oikade window
that rs-matter is still closing returns the retryable `window_closing` error.
These secret-bearing values use a `no-store` response and are not retained in
adapter status or written to Oikade logs. The CLI command is:

```sh
oikade adapters open-commissioning-window --duration 10m matter
```

Each process start with zero fabrics opens a fresh automatic window. Restarts
with any existing fabric do not open it.

`DELETE /v1/adapters/{instance}/resources/{type}/{id}` removes one explicitly
selected protocol resource and returns the complete remaining inventory. The
current `rs-matter` adapter reports `matter.fabric` resources but fails local
single-fabric removal closed because the pinned SDK does not yet expose safe
live-session expiry. Controller-driven fabric removal remains available. A
future local implementation must not remove endpoint mappings, plugin state,
the Oikade database or the adapter state directory. The CLI requires
`--confirm` before issuing this request.

`POST /v1/adapters/{instance}/reset` permanently resets the selected adapter's
entire protocol-owned state. Its JSON confirmation must exactly equal the
instance ID. Oikade stops the adapter, verifies its private ownership marker,
rotates only that exact state directory, creates a new private directory and
waits for the adapter to become ready again. Canonical devices, plugins and the
Oikade database are outside that directory. The equivalent CLI requires the
instance ID twice to make the destructive scope explicit:

```sh
oikade adapters reset --confirm matter matter
```

The `/v1` API remains experimental until its compatibility policy and error
contract are frozen for a stable release.
